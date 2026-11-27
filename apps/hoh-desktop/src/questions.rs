//! Questions the engine asks the person (`user-question/ask`, from the ask-user tool).
//! The main window shows them. Rust validates each answer against the original request.
//! A credential request has a separate answer path that sends the engine only a private file path.
use crate::chat_secrets::ChatSecrets;
use hoh_protocol::{
    AskUserQuestionAnswerItem, AskUserQuestionIntent, AskUserQuestionItem, SdkUserQuestionAnswer,
    SdkUserQuestionRequest, SdkUserQuestionResult, SecretString,
};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Mutex};

const MAX_PENDING: usize = 8;
const MAX_QUESTIONS: usize = 16;
const MAX_CUSTOM_CHARS: usize = 8_000;

struct Pending {
    generation: u64,
    rpc_id: Value,
    session_id: String,
    questions: Vec<AskUserQuestionItem>,
}

#[derive(Default)]
pub struct QuestionBroker {
    pending: Mutex<HashMap<String, Pending>>,
}

/// What the main window needs to show: no engine ids beyond the request id.
pub type QuestionView = SdkUserQuestionRequest;

impl QuestionBroker {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Pending>> {
        self.pending.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn begin(
        &self,
        generation: u64,
        rpc_id: Value,
        params: Value,
    ) -> Result<QuestionView, String> {
        let request: SdkUserQuestionRequest =
            serde_json::from_value(params).map_err(|_| "Получен некорректный вопрос".to_owned())?;
        let mut ids = std::collections::HashSet::new();
        if request.questions.is_empty()
            || request.questions.len() > MAX_QUESTIONS
            || request.questions.iter().any(|q| !ids.insert(q.id.as_str()))
        {
            return Err("Получен некорректный вопрос".into());
        }
        if request.questions.iter().any(is_secret)
            && !matches!(request.questions.as_slice(), [question] if is_secret(question)
               && question.options.as_ref().is_none_or(Vec::is_empty) && question.multi_select != Some(true))
        {
            return Err("Invalid credential request.".into());
        }
        let mut pending = self.lock();
        if pending.len() >= MAX_PENDING || pending.contains_key(&request.request_id) {
            return Err("Слишком много неотвеченных вопросов".into());
        }
        pending.insert(
            request.request_id.clone(),
            Pending {
                generation,
                rpc_id,
                session_id: request.session_id.clone(),
                questions: request.questions.clone(),
            },
        );
        Ok(request)
    }

    /// Validates the page's answer and, only then, consumes the question. Returns the generation
    /// and the JSON-RPC response frame to send to the engine.
    pub fn answer(&self, request_id: &str, answer: &Value) -> Result<(u64, Value), String> {
        let mut pending = self.lock();
        let entry = pending
            .get(request_id)
            .ok_or_else(|| "Вопрос уже закрыт".to_owned())?;
        if entry.questions.iter().any(is_secret) {
            return Err("Use the private credential form for this request.".into());
        }
        let result = if answer.get("cancelled") == Some(&Value::Bool(true))
            && answer.as_object().is_some_and(|o| o.len() == 1)
        {
            json!({"cancelled":true})
        } else {
            let answers = validate(&entry.questions, answer)?;
            serde_json::to_value(SdkUserQuestionResult::Answer(SdkUserQuestionAnswer {
                answers,
            }))
            .map_err(|_| "Ответ не удалось подготовить".to_owned())?
        };
        let entry = pending.remove(request_id).expect("checked above");
        Ok((
            entry.generation,
            hoh_protocol::framing::success(entry.rpc_id, result),
        ))
    }

    pub fn answer_secret(
        &self,
        store: &ChatSecrets,
        session_id: &str,
        request_id: &str,
        secret: Option<&SecretString>,
    ) -> Result<(u64, Value, Option<String>), String> {
        let mut pending = self.lock();
        let entry = pending
            .get(request_id)
            .ok_or("The request is already closed.")?;
        let [question] = entry.questions.as_slice() else {
            return Err("The request is not for a credential.".into());
        };
        if entry.session_id != session_id || !is_secret(question) {
            return Err("The request is not for a credential in this chat.".into());
        }
        let path = secret
            .map(|value| store.store_for_turn(session_id, value))
            .transpose()?;
        let result = match &path {
            Some(path) => json!({"answers":[{"id":question.id,"selected":[],"custom":path}]}),
            None => json!({"cancelled":true}),
        };
        let entry = pending.remove(request_id).expect("checked above");
        Ok((
            entry.generation,
            hoh_protocol::framing::success(entry.rpc_id, result),
            path,
        ))
    }

    pub fn close_session_secrets(&self, session_id: &str) {
        self.lock().retain(|_, entry| {
            entry.session_id != session_id || !entry.questions.iter().any(is_secret)
        });
    }

    /// The engine withdrew the question (`user-question.cancelled`).
    pub fn cancel(&self, request_id: &str) -> bool {
        self.lock().remove(request_id).is_some()
    }

    /// The engine went away: nobody is waiting for these answers any more.
    pub fn clear(&self) {
        self.lock().clear();
    }
}

pub fn is_secret(question: &AskUserQuestionItem) -> bool {
    matches!(question.intent, Some(AskUserQuestionIntent::SecretFile))
}

fn validate(
    questions: &[AskUserQuestionItem],
    answer: &Value,
) -> Result<Vec<AskUserQuestionAnswerItem>, String> {
    let bad = || "Ответ не подходит к вопросу".to_owned();
    let answers: SdkUserQuestionAnswer =
        serde_json::from_value(answer.clone()).map_err(|_| bad())?;
    if answers.answers.len() != questions.len() {
        return Err(bad());
    }
    for question in questions {
        let given = answers
            .answers
            .iter()
            .filter(|a| a.id == question.id)
            .collect::<Vec<_>>();
        let [given] = given.as_slice() else {
            return Err(bad());
        };
        if given
            .custom
            .as_ref()
            .is_some_and(|c| c.chars().count() > MAX_CUSTOM_CHARS)
        {
            return Err(bad());
        }
        if let Some(options) = &question.options {
            let known = |label: &String| options.iter().any(|o| &o.label == label);
            if !given.selected.iter().all(known) {
                return Err(bad());
            }
        } else if !given.selected.is_empty() {
            return Err(bad());
        }
        if question.multi_select != Some(true) && given.selected.len() > 1 {
            return Err(bad());
        }
        let mut distinct = std::collections::HashSet::new();
        if !given.selected.iter().all(|s| distinct.insert(s)) {
            return Err(bad());
        }
    }
    Ok(answers.answers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn ask(broker: &QuestionBroker, request_id: &str) -> QuestionView {
        broker
            .begin(
                3,
                json!(7),
                json!({"requestId":request_id,"sessionId":"s","questions":[
                    {"id":"q1","question":"Какой формат?","options":[{"label":"PDF"},{"label":"Word"}]},
                    {"id":"q2","question":"Свободный ответ","multiSelect":true,"options":[{"label":"а"},{"label":"б"}]}
                ]}),
            )
            .unwrap()
    }

    #[test]
    fn a_valid_answer_goes_to_the_asking_generation_once() {
        let broker = QuestionBroker::default();
        ask(&broker, "r1");
        let answer = json!({"answers":[
            {"id":"q1","selected":["PDF"]},
            {"id":"q2","selected":["а","б"],"custom":"и ещё"}
        ]});
        let (generation, frame) = broker.answer("r1", &answer).unwrap();
        assert_eq!(generation, 3);
        assert_eq!(frame["id"], 7);
        assert_eq!(frame["result"]["answers"][1]["custom"], "и ещё");
        assert!(broker.answer("r1", &answer).is_err());
    }

    #[test]
    fn a_wrong_answer_is_refused_and_the_question_stays_open() {
        let broker = QuestionBroker::default();
        ask(&broker, "r1");
        for bad in [
            json!({"answers":[{"id":"q1","selected":["PDF"]}]}),
            json!({"answers":[{"id":"q1","selected":["Excel"]},{"id":"q2","selected":[]}]}),
            json!({"answers":[{"id":"q1","selected":["PDF","Word"]},{"id":"q2","selected":[]}]}),
            json!({"answers":[{"id":"q1","selected":["PDF"]},{"id":"q9","selected":[]}]}),
            json!({"answers":[{"id":"q1","selected":["PDF"]},{"id":"q1","selected":["PDF"]}]}),
            json!({"cancelled":true,"x":1}),
            json!("PDF"),
        ] {
            assert!(broker.answer("r1", &bad).is_err(), "{bad}");
        }
        let good = json!({"answers":[{"id":"q1","selected":["Word"]},{"id":"q2","selected":[]}]});
        assert!(broker.answer("r1", &good).is_ok());
    }

    #[test]
    fn dismissing_answers_with_cancelled() {
        let broker = QuestionBroker::default();
        ask(&broker, "r1");
        let (_, frame) = broker.answer("r1", &json!({"cancelled":true})).unwrap();
        assert_eq!(frame["result"], json!({"cancelled":true}));
    }

    #[test]
    fn protected_answers_validate_the_chat_and_only_deliver_a_private_file_path() {
        let broker = QuestionBroker::default();
        let directory =
            std::env::temp_dir().join(format!("hoh-credential-card-test-{}", std::process::id()));
        let store = ChatSecrets::new(directory.clone());
        let request = |id: &str| {
            json!({"requestId":id,"sessionId":"s","questions":[
                {"id":"credential","question":"Try the test","intent":{"kind":"secret-file"}}
            ]})
        };
        broker.begin(3, json!(7), request("r1")).unwrap();
        let dummy = "synthetic-card-token";
        let secret = SecretString::from(dummy);
        assert!(
            broker
                .answer(
                    "r1",
                    &json!({"answers":[{"id":"credential","selected":[],"custom":dummy}]})
                )
                .is_err()
        );
        assert!(
            broker
                .answer_secret(&store, "other", "r1", Some(&secret))
                .is_err()
        );
        assert!(
            broker
                .answer_secret(&store, "s", "r1", Some(&"  ".into()))
                .is_err()
        );
        assert!(!directory.exists());

        let manual = store.store(&secret).unwrap();
        let (generation, frame, path) = broker
            .answer_secret(&store, "s", "r1", Some(&secret))
            .unwrap();
        let path = path.unwrap();
        assert_eq!(generation, 3);
        assert_eq!(frame["id"], 7);
        assert_eq!(frame["result"]["answers"][0]["custom"], path);
        assert!(!frame.to_string().contains(dummy));
        assert_eq!(fs::read_to_string(&path).unwrap(), dummy);
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(
            broker
                .answer_secret(&store, "s", "r1", Some(&secret))
                .is_err()
        );

        broker.begin(3, json!(8), request("r2")).unwrap();
        let (_, frame, path_on_decline) = broker.answer_secret(&store, "s", "r2", None).unwrap();
        assert_eq!(frame["result"], json!({"cancelled":true}));
        assert!(path_on_decline.is_none());

        broker.begin(3, json!(9), request("r3")).unwrap();
        broker.close_session_secrets("s");
        store.close_session("s");
        assert!(
            broker
                .answer_secret(&store, "s", "r3", Some(&secret))
                .is_err()
        );
        assert!(!std::path::Path::new(&path).exists());
        assert!(std::path::Path::new(&manual).is_file());
        store.close();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn engine_withdrawal_and_restart_close_questions() {
        let broker = QuestionBroker::default();
        ask(&broker, "r1");
        ask(&broker, "r2");
        assert!(broker.cancel("r1"));
        assert!(!broker.cancel("r1"));
        broker.clear();
        assert!(broker.answer("r2", &json!({"cancelled":true})).is_err());
    }

    #[test]
    fn malformed_duplicate_and_excess_questions_are_refused() {
        let broker = QuestionBroker::default();
        assert!(broker.begin(1, json!(1), json!({"requestId":"a"})).is_err());
        assert!(
            broker
                .begin(
                    1,
                    json!(1),
                    json!({"requestId":"a","sessionId":"s","questions":[]})
                )
                .is_err()
        );
        ask(&broker, "r1");
        assert!(
            broker
                .begin(1, json!(2), json!({"requestId":"r1","sessionId":"s","questions":[{"id":"q","question":"?"}]}))
                .is_err()
        );
        for n in 0..10 {
            let _ = broker.begin(1, json!(n), json!({"requestId":format!("x{n}"),"sessionId":"s","questions":[{"id":"q","question":"?"}]}));
        }
        assert!(
            broker
                .begin(1, json!(99), json!({"requestId":"late","sessionId":"s","questions":[{"id":"q","question":"?"}]}))
                .is_err()
        );
    }
}
