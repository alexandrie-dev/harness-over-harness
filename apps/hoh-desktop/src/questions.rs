//! Questions the engine asks the person (`user-question/ask`, from the ask-user tool).
//! They are not approvals: the answer is text for the model and grants nothing. The main window
//! shows them; Rust keeps the original question and validates the answer against it, so the page
//! cannot answer an id it was not asked or pick an option that does not exist.
use hoh_protocol::{
    AskUserQuestionAnswerItem, AskUserQuestionItem, SdkUserQuestionAnswer, SdkUserQuestionRequest,
    SdkUserQuestionResult,
};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Mutex};

const MAX_PENDING: usize = 8;
const MAX_QUESTIONS: usize = 16;
const MAX_CUSTOM_CHARS: usize = 8_000;

struct Pending {
    generation: u64,
    rpc_id: Value,
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
        let request: SdkUserQuestionRequest = serde_json::from_value(params)
            .map_err(|_| "Движок прислал некорректный вопрос".to_owned())?;
        let mut ids = std::collections::HashSet::new();
        if request.questions.is_empty()
            || request.questions.len() > MAX_QUESTIONS
            || request.questions.iter().any(|q| !ids.insert(q.id.as_str()))
        {
            return Err("Движок прислал некорректный вопрос".into());
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

    /// The engine withdrew the question (`user-question.cancelled`).
    pub fn cancel(&self, request_id: &str) -> bool {
        self.lock().remove(request_id).is_some()
    }

    /// The engine went away: nobody is waiting for these answers any more.
    pub fn clear(&self) {
        self.lock().clear();
    }
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
