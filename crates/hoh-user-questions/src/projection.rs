//! Pure fold of timed `ask_user_question` calls and its Session projection.

use crate::types::{
    AskUserQuestionAnswerItem, AskUserQuestionItem, AskUserQuestionOption, PendingUserQuestion,
    SettledUserQuestion, UserQuestionProjectionView, UserQuestionState,
};
use hoh_error::BoxError;
use hoh_llm::{ToolCallId, ToolSchema};
use hoh_session::{MAX_SAFE_INTEGER, SessionEvent, SessionLogOffset, TOOL_OUTCOME_UNKNOWN};
use hoh_session_projection::{ProjectionDefinition, ProjectionWire};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{collections::HashSet, sync::Arc};

pub const ASK_USER_QUESTION_TOOL: &str = "ask_user_question";
pub const TIMED_WAIT_PARAMETER: &str = "timeout";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserQuestionFold {
    pub timed: bool,
    pub questions: UserQuestionProjectionView,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserQuestionProjectionState {
    pub inherited_event_count: SessionLogOffset,
    pub timed: bool,
    pub questions: UserQuestionProjectionView,
}

pub fn is_timed_ask_user_question_schema(tool: &ToolSchema) -> bool {
    is_timed_schema_parts(&tool.name, &tool.parameters)
}

fn is_timed_schema_parts(name: &str, parameters: &Map<String, Value>) -> bool {
    name == ASK_USER_QUESTION_TOOL
        && parameters
            .get("properties")
            .and_then(Value::as_object)
            .is_some_and(|properties| properties.contains_key(TIMED_WAIT_PARAMETER))
}

fn string(object: &Map<String, Value>, key: &str) -> Option<String> {
    object.get(key)?.as_str().map(ToOwned::to_owned)
}

fn tool_option(value: &Value) -> Option<AskUserQuestionOption> {
    let object = value.as_object()?;
    Some(AskUserQuestionOption {
        label: string(object, "label")?,
        description: match object.get("description") {
            Some(value) => Some(value.as_str()?.to_owned()),
            None => None,
        },
    })
}

fn tool_question(value: &Value) -> Option<AskUserQuestionItem> {
    let object = value.as_object()?;
    let options = match object.get("options") {
        Some(value) => Some(
            value
                .as_array()?
                .iter()
                .map(tool_option)
                .collect::<Option<Vec<_>>>()?,
        ),
        None => None,
    };
    let multi_select = match object.get("multi_select") {
        Some(value) => Some(value.as_bool()?),
        None => None,
    };
    Some(AskUserQuestionItem {
        id: string(object, "id")?,
        question: string(object, "question")?,
        detail: None,
        header: match object.get("header") {
            Some(value) => Some(value.as_str()?.to_owned()),
            None => None,
        },
        options,
        multi_select,
        intent: None,
    })
}

/// Parse the model-facing snake-case argument shape. Unknown keys are ignored,
/// matching the source's loose tool schemas.
pub fn questions_of(arguments_text: &str) -> Option<Vec<AskUserQuestionItem>> {
    let value: Value = serde_json::from_str(arguments_text).ok()?;
    let questions = value.as_object()?.get("questions")?.as_array()?;
    if questions.is_empty() {
        return None;
    }
    questions.iter().map(tool_question).collect()
}

fn first_text(content: &Value) -> Option<&str> {
    content.as_array()?.iter().find_map(|block| {
        (block.get("type")?.as_str()? == "text")
            .then(|| block.get("text")?.as_str())
            .flatten()
    })
}

fn is_pending_result(content: &Value) -> bool {
    first_text(content)
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .and_then(|value| value.get("pending").and_then(Value::as_bool))
        == Some(true)
}

fn answer_batch_of(content: &Value) -> Option<Vec<AskUserQuestionAnswerItem>> {
    let text = first_text(content)?;
    let value: Value = serde_json::from_str(text).ok()?;
    let answers = value.get("answers")?.as_array()?;
    for answer in answers {
        let answer = answer.as_object()?;
        if answer.get("id").and_then(Value::as_str).is_none()
            || !answer
                .get("selected")
                .and_then(Value::as_array)
                .is_some_and(|selected| selected.iter().all(Value::is_string))
            || answer
                .get("custom")
                .is_some_and(|custom| !custom.is_string())
        {
            return None;
        }
    }
    serde_json::from_value(Value::Array(answers.clone())).ok()
}

fn settle_question(
    view: &mut UserQuestionProjectionView,
    call_id: &str,
    answers: Vec<AskUserQuestionAnswerItem>,
) -> bool {
    let Some(index) = view
        .active
        .iter()
        .position(|question| question.call_id == call_id)
    else {
        return false;
    };
    let question = view.active.remove(index);
    view.settled.push(SettledUserQuestion {
        call_id: question.call_id,
        answers,
    });
    true
}

fn timed_header(data: &Value) -> bool {
    data.pointer("/header/tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| {
            tools.iter().any(|value| {
                let Some(tool) = value.as_object() else {
                    return false;
                };
                let (Some(name), Some(parameters)) = (
                    tool.get("name").and_then(Value::as_str),
                    tool.get("parameters").and_then(Value::as_object),
                ) else {
                    return false;
                };
                is_timed_schema_parts(name, parameters)
            })
        })
}

/// Apply one immutable Session event. Unrelated and malformed events preserve
/// the input value exactly at the semantic level.
pub fn apply_user_question_event(
    fold: &UserQuestionFold,
    event: &SessionEvent,
) -> UserQuestionFold {
    let data = event.data();
    match event.event_type() {
        "request/header" => {
            let timed = timed_header(data);
            if timed == fold.timed {
                fold.clone()
            } else {
                UserQuestionFold {
                    timed,
                    questions: fold.questions.clone(),
                }
            }
        }
        "tool/call" => {
            if !fold.timed
                || data.get("name").and_then(Value::as_str) != Some(ASK_USER_QUESTION_TOOL)
            {
                return fold.clone();
            }
            let Some(call_id) = data.get("callId").and_then(Value::as_str) else {
                return fold.clone();
            };
            let Some(questions) = data
                .get("arguments")
                .and_then(Value::as_str)
                .and_then(questions_of)
            else {
                return fold.clone();
            };
            let mut next = fold.clone();
            next.questions
                .active
                .retain(|question| question.call_id != call_id);
            next.questions.active.push(PendingUserQuestion {
                call_id: ToolCallId::from(call_id),
                questions,
                state: UserQuestionState::Open,
            });
            next
        }
        "tool/result" => {
            let message = &data["message"];
            let Some(call_id) = message.get("toolCallId").and_then(Value::as_str) else {
                return fold.clone();
            };
            if !fold
                .questions
                .active
                .iter()
                .any(|question| question.call_id == call_id)
            {
                return fold.clone();
            }
            let mut next = fold.clone();
            if is_pending_result(&message["content"])
                || data.pointer("/error/code").and_then(Value::as_str) == Some(TOOL_OUTCOME_UNKNOWN)
            {
                for question in &mut next.questions.active {
                    if question.call_id == call_id {
                        question.state = UserQuestionState::Continued;
                    }
                }
                return next;
            }
            let answers = (data.get("error").is_none()
                && message.get("isError").and_then(Value::as_bool) != Some(true))
            .then(|| answer_batch_of(&message["content"]))
            .flatten();
            if let Some(answers) = answers {
                settle_question(&mut next.questions, call_id, answers);
            } else {
                next.questions
                    .active
                    .retain(|question| question.call_id != call_id);
            }
            next
        }
        "tool/ptc-dispatch" => {
            if data.get("name").and_then(Value::as_str) != Some(ASK_USER_QUESTION_TOOL)
                || data.get("isError").and_then(Value::as_bool) == Some(true)
                || !is_pending_result(&data["content"])
            {
                return fold.clone();
            }
            let (Some(call_id), Some(arguments)) = (
                data.get("subCallId").and_then(Value::as_str),
                data.get("arguments"),
            ) else {
                return fold.clone();
            };
            let Some(questions) = serde_json::to_string(arguments)
                .ok()
                .and_then(|arguments| questions_of(&arguments))
            else {
                return fold.clone();
            };
            let mut next = fold.clone();
            next.questions
                .active
                .retain(|question| question.call_id != call_id);
            next.questions.active.push(PendingUserQuestion {
                call_id: ToolCallId::from(call_id),
                questions,
                state: UserQuestionState::Continued,
            });
            next
        }
        "user/message" => {
            if data.pointer("/source/kind").and_then(Value::as_str) != Some("user-question-reply") {
                return fold.clone();
            }
            let Some(call_id) = data.pointer("/source/callId").and_then(Value::as_str) else {
                return fold.clone();
            };
            let mut next = fold.clone();
            let answers = answer_batch_of(&data["content"]).unwrap_or_default();
            if settle_question(&mut next.questions, call_id, answers) {
                next
            } else {
                fold.clone()
            }
        }
        _ => fold.clone(),
    }
}

pub fn fold_user_questions<'a>(
    events: impl IntoIterator<Item = &'a SessionEvent>,
) -> UserQuestionProjectionView {
    events
        .into_iter()
        .fold(UserQuestionFold::default(), |fold, event| {
            apply_user_question_event(&fold, event)
        })
        .questions
}

fn validate_view(view: &UserQuestionProjectionView) -> Result<(), BoxError> {
    let mut ids = HashSet::new();
    for question in &view.active {
        if question.call_id.as_str().is_empty()
            || question.questions.is_empty()
            || !ids.insert(question.call_id.as_str())
        {
            return Err("active callIds must be non-empty and unique, with questions".into());
        }
        if question
            .questions
            .iter()
            .any(|question| question.intent.is_some())
        {
            return Err("projection questions do not carry presentation intents".into());
        }
    }
    if view
        .settled
        .iter()
        .any(|question| question.call_id.as_str().is_empty())
    {
        return Err("settled callIds must be non-empty".into());
    }
    Ok(())
}

pub fn user_question_projection_definition() -> ProjectionDefinition<UserQuestionProjectionState> {
    let state_schema = Arc::new(|value: Option<&Value>| {
        let value = value.ok_or_else(|| -> BoxError { "missing userQuestions state".into() })?;
        let state: UserQuestionProjectionState = serde_json::from_value(value.clone())?;
        if state.inherited_event_count.get() > MAX_SAFE_INTEGER {
            return Err("inheritedEventCount exceeds Number.MAX_SAFE_INTEGER".into());
        }
        validate_view(&state.questions)?;
        Ok(Arc::new(state))
    });
    let view_schema = Arc::new(|value: Option<&Value>| {
        let value = value.ok_or_else(|| -> BoxError { "missing userQuestions view".into() })?;
        let view: UserQuestionProjectionView = serde_json::from_value(value.clone())?;
        validate_view(&view)?;
        Ok(Arc::new(serde_json::to_value(view)?))
    });
    ProjectionDefinition {
        key: "userQuestions".into(),
        state_schema,
        init: Arc::new(|_, inherited_event_count| {
            Ok(Arc::new(UserQuestionProjectionState {
                inherited_event_count,
                timed: false,
                questions: UserQuestionProjectionView::default(),
            }))
        }),
        apply: Arc::new(|state, event| {
            if event.seq().get() < state.inherited_event_count.get() {
                return Ok(state.clone());
            }
            let fold = UserQuestionFold {
                timed: state.timed,
                questions: state.questions.clone(),
            };
            let next = apply_user_question_event(&fold, event);
            if next == fold {
                Ok(state.clone())
            } else {
                Ok(Arc::new(UserQuestionProjectionState {
                    inherited_event_count: state.inherited_event_count,
                    timed: next.timed,
                    questions: next.questions,
                }))
            }
        }),
        wire: Some(ProjectionWire::new(
            view_schema,
            Arc::new(|state| Ok(Some(Arc::new(serde_json::to_value(&state.questions)?)))),
        )),
        state_version: 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(kind: &str, data: Value, seq: u64) -> SessionEvent {
        SessionEvent::from_value(json!({"type":kind,"seq":seq,"time":0,"data":data}))
    }
    fn timed_header_event(seq: u64) -> SessionEvent {
        event(
            "request/header",
            json!({"header":{"tools":[{"name":"ask_user_question","description":"ask","parameters":{"type":"object","properties":{"timeout":{"type":"integer"}}}}]}}),
            seq,
        )
    }
    fn call(id: &str, arguments: &str, seq: u64) -> SessionEvent {
        event(
            "tool/call",
            json!({"callId":id,"name":ASK_USER_QUESTION_TOOL,"arguments":arguments}),
            seq,
        )
    }
    fn result(id: &str, text: &str, seq: u64) -> SessionEvent {
        event(
            "tool/result",
            json!({"message":{"toolCallId":id,"content":[{"type":"text","text":text}]}}),
            seq,
        )
    }
    fn args() -> &'static str {
        r#"{"questions":[{"id":"format","question":"Which?","header":"Output","options":[{"label":"CSV","description":"table"}],"multi_select":true}]}"#
    }

    #[test]
    fn timed_schema_requires_exact_name_and_timeout_property() {
        let tool: ToolSchema = serde_json::from_value(json!({"name":ASK_USER_QUESTION_TOOL,"description":"","parameters":{"properties":{"timeout":{}}}})).unwrap();
        assert!(is_timed_ask_user_question_schema(&tool));
        let mut legacy = tool.clone();
        legacy.parameters["properties"]
            .as_object_mut()
            .unwrap()
            .remove("timeout");
        assert!(!is_timed_ask_user_question_schema(&legacy));
        legacy.name = "other".into();
        legacy.parameters["properties"]["timeout"] = json!({});
        assert!(!is_timed_ask_user_question_schema(&legacy));
    }

    #[test]
    fn questions_of_maps_loose_snake_case_arguments() {
        let questions = questions_of(args()).unwrap();
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].id, "format");
        assert_eq!(questions[0].multi_select, Some(true));
        assert_eq!(
            questions[0].options.as_ref().unwrap()[0]
                .description
                .as_deref(),
            Some("table")
        );
        assert!(questions[0].intent.is_none());
    }

    #[test]
    fn questions_of_rejects_bad_json_empty_and_wrong_fields() {
        for value in [
            "no",
            "{}",
            r#"{"questions":[]}"#,
            r#"{"questions":[{"id":1,"question":"q"}]}"#,
            r#"{"questions":[{"id":"x","question":"q","multi_select":"yes"}]}"#,
        ] {
            assert!(questions_of(value).is_none(), "{value}");
        }
    }

    #[test]
    fn legacy_calls_are_not_tracked() {
        let view = fold_user_questions([&call("c", args(), 0)]);
        assert!(view.active.is_empty());
    }

    #[test]
    fn timed_call_opens_and_pending_result_continues() {
        let events = [
            timed_header_event(0),
            call("c", args(), 1),
            result("c", r#"{"pending":true,"callId":"c"}"#, 2),
        ];
        let view = fold_user_questions(events.iter());
        assert_eq!(view.active.len(), 1);
        assert_eq!(view.active[0].state, UserQuestionState::Continued);
    }

    #[test]
    fn answer_result_settles_with_batch() {
        let events = [
            timed_header_event(0),
            call("c", args(), 1),
            result(
                "c",
                r#"{"answers":[{"id":"format","selected":["CSV"]}]}"#,
                2,
            ),
        ];
        let view = fold_user_questions(events.iter());
        assert!(view.active.is_empty());
        assert_eq!(view.settled[0].answers[0].selected, ["CSV"]);
    }

    #[test]
    fn null_custom_answer_is_rejected_instead_of_settling() {
        let events = [
            timed_header_event(0),
            call("c", args(), 1),
            result(
                "c",
                r#"{"answers":[{"id":"format","selected":[],"custom":null}]}"#,
                2,
            ),
        ];
        let view = fold_user_questions(events.iter());
        assert!(view.active.is_empty());
        assert!(view.settled.is_empty());
    }

    #[test]
    fn tool_result_requires_its_canonical_tool_call_id() {
        let events = [timed_header_event(0), call("c", args(), 1)];
        let open = fold_user_questions(events.iter());
        let source_only = event(
            "tool/result",
            json!({"message":{"source":{"callId":"c"},"content":[]}}),
            2,
        );
        let fold = UserQuestionFold {
            timed: true,
            questions: open,
        };
        assert_eq!(apply_user_question_event(&fold, &source_only), fold);
    }

    #[test]
    fn successful_result_with_empty_content_closes_the_call() {
        let events = [
            timed_header_event(0),
            call("c", args(), 1),
            event(
                "tool/result",
                json!({"message":{"toolCallId":"c","content":[],"isError":false}}),
                2,
            ),
        ];
        assert_eq!(
            fold_user_questions(events.iter()),
            UserQuestionProjectionView::default()
        );
    }

    #[test]
    fn malformed_and_error_results_drop_the_call() {
        for data in [
            json!({"message":{"toolCallId":"c","content":[{"type":"text","text":"bad"}]}}),
            json!({"message":{"toolCallId":"c","content":[{"type":"text","text":"{\"answers\":[]}"}]},"error":{"code":"FAILED"}}),
            json!({"message":{"toolCallId":"c","content":[{"type":"text","text":"{\"answers\":[]}"}],"isError":true}}),
        ] {
            let events = [
                timed_header_event(0),
                call("c", args(), 1),
                event("tool/result", data, 2),
            ];
            let view = fold_user_questions(events.iter());
            assert!(view.active.is_empty());
            assert!(view.settled.is_empty());
        }
    }

    #[test]
    fn unknown_recovery_outcome_continues() {
        let events = [
            timed_header_event(0),
            call("c", args(), 1),
            event(
                "tool/result",
                json!({"message":{"toolCallId":"c","content":[]},"error":{"code":TOOL_OUTCOME_UNKNOWN}}),
                2,
            ),
        ];
        assert_eq!(
            fold_user_questions(events.iter()).active[0].state,
            UserQuestionState::Continued
        );
    }

    #[test]
    fn changing_header_back_to_legacy_only_affects_future_calls() {
        let events = [
            timed_header_event(0),
            call("first", args(), 1),
            event("request/header", json!({"header":{"tools":[]}}), 2),
            call("second", args(), 3),
        ];
        let view = fold_user_questions(events.iter());
        assert_eq!(
            view.active
                .iter()
                .map(|q| q.call_id.as_str())
                .collect::<Vec<_>>(),
            ["first"]
        );
    }

    #[test]
    fn repeated_call_id_replaces_and_moves_to_the_end() {
        let events = [
            timed_header_event(0),
            call("same", args(), 1),
            call("other", args(), 2),
            call(
                "same",
                r#"{"questions":[{"id":"new","question":"New?"}]}"#,
                3,
            ),
        ];
        let view = fold_user_questions(events.iter());
        assert_eq!(
            view.active
                .iter()
                .map(|q| q.call_id.as_str())
                .collect::<Vec<_>>(),
            ["other", "same"]
        );
        assert_eq!(view.active[1].questions[0].id, "new");
    }

    #[test]
    fn ptc_pending_dispatch_enters_as_continued() {
        let dispatch = event(
            "tool/ptc-dispatch",
            json!({"subCallId":"sub","name":ASK_USER_QUESTION_TOOL,"arguments":{"questions":[{"id":"x","question":"X?"}]},"content":[{"type":"text","text":"{\"pending\":true}"}]}),
            0,
        );
        let view = fold_user_questions([&dispatch]);
        assert_eq!(view.active[0].state, UserQuestionState::Continued);
    }

    #[test]
    fn failed_or_non_pending_ptc_is_ignored() {
        for data in [
            json!({"subCallId":"s","name":ASK_USER_QUESTION_TOOL,"arguments":{"questions":[{"id":"x","question":"X"}]},"isError":true,"content":[{"type":"text","text":"{\"pending\":true}"}]}),
            json!({"subCallId":"s","name":ASK_USER_QUESTION_TOOL,"arguments":{"questions":[{"id":"x","question":"X"}]},"content":[{"type":"text","text":"done"}]}),
            json!({"subCallId":"s","name":"other","arguments":{"questions":[{"id":"x","question":"X"}]},"content":[{"type":"text","text":"{\"pending\":true}"}]}),
        ] {
            assert!(
                fold_user_questions([&event("tool/ptc-dispatch", data, 0)])
                    .active
                    .is_empty()
            );
        }
    }

    #[test]
    fn late_reply_settles_and_missing_batch_records_empty_answers() {
        let base = [
            timed_header_event(0),
            call("c", args(), 1),
            result("c", r#"{"pending":true}"#, 2),
        ];
        let reply = event(
            "user/message",
            json!({"id":"m","source":{"kind":"user-question-reply","callId":"c","outcome":"answered"},"content":[{"type":"text","text":"not-json"}]}),
            3,
        );
        let view = fold_user_questions(base.iter().chain([&reply]));
        assert!(view.active.is_empty());
        assert!(view.settled[0].answers.is_empty());
    }

    #[test]
    fn late_reply_reads_answers_from_the_full_loose_service_payload() {
        let base = [
            timed_header_event(0),
            call("c", args(), 1),
            result("c", r#"{"pending":true}"#, 2),
        ];
        let reply = event(
            "user/message",
            json!({"id":"m","source":{"kind":"user-question-reply","callId":"c","outcome":"answered"},"content":[{"type":"text","text":"{\"kind\":\"answer_to_pending_question\",\"tool\":\"ask_user_question\",\"callId\":\"c\",\"questions\":[],\"answers\":[{\"id\":\"format\",\"selected\":[\"CSV\"]}]}"}]}),
            3,
        );
        let view = fold_user_questions(base.iter().chain([&reply]));
        assert_eq!(view.settled[0].answers[0].selected, ["CSV"]);
    }

    #[test]
    fn header_detection_needs_only_name_and_parameters() {
        let minimal = event(
            "request/header",
            json!({"header":{"tools":[{"name":"ask_user_question","parameters":{"properties":{"timeout":{}}}}]}}),
            0,
        );
        assert!(apply_user_question_event(&UserQuestionFold::default(), &minimal).timed);
    }

    #[test]
    fn unrelated_reply_and_events_preserve_state() {
        let open = apply_user_question_event(
            &apply_user_question_event(&UserQuestionFold::default(), &timed_header_event(0)),
            &call("c", args(), 1),
        );
        let unrelated = event(
            "user/message",
            json!({"source":{"kind":"user"},"content":[]}),
            2,
        );
        assert_eq!(apply_user_question_event(&open, &unrelated), open);
        assert_eq!(
            apply_user_question_event(&open, &event("turn/start", json!({}), 2)),
            open
        );
    }

    #[test]
    fn projection_has_key_version_and_wire_view() {
        let definition = user_question_projection_definition();
        assert_eq!(definition.key, "userQuestions");
        assert_eq!(definition.state_version, 2);
        assert!(definition.wire.is_some());
    }

    #[test]
    fn projection_ignores_inherited_prefix() {
        let definition = user_question_projection_definition();
        let header = hoh_session::SessionHeader::new("s".into(), 0);
        let state = (definition.init)(&header, SessionLogOffset::new(2).unwrap()).unwrap();
        let initial_view = (definition.wire.as_ref().unwrap().view)(&state)
            .unwrap()
            .unwrap();
        let state = (definition.apply)(&state, &timed_header_event(0)).unwrap();
        assert!(!state.timed);
        let state = (definition.apply)(&state, &timed_header_event(2)).unwrap();
        assert!(state.timed);
        assert_eq!(
            (definition.wire.as_ref().unwrap().view)(&state)
                .unwrap()
                .unwrap(),
            initial_view
        );
        let unchanged = (definition.apply)(&state, &event("turn/start", json!({}), 3)).unwrap();
        assert!(Arc::ptr_eq(&unchanged, &state));
    }

    #[test]
    fn checkpoint_schema_is_strict_and_rejects_duplicate_active_ids() {
        let definition = user_question_projection_definition();
        let duplicate = json!({"inheritedEventCount":0,"timed":true,"questions":{"active":[
            {"callId":"c","questions":[{"id":"a","question":"A"}],"state":"open"},
            {"callId":"c","questions":[{"id":"b","question":"B"}],"state":"continued"}
        ],"settled":[]}});
        assert!((definition.state_schema)(Some(&duplicate)).is_err());
        let extra = json!({"inheritedEventCount":0,"timed":false,"questions":{"active":[],"settled":[]},"future":true});
        assert!((definition.state_schema)(Some(&extra)).is_err());
    }

    #[test]
    fn checkpoint_round_trips_full_question_detail_and_wire_view() {
        let definition = user_question_projection_definition();
        let checkpoint = json!({
            "inheritedEventCount":3,
            "timed":true,
            "questions":{
                "active":[{"callId":"c","questions":[
                    {"id":"plan","question":"Approve?","detail":"# Plan","header":"Review","options":[{"label":"Approve","description":"Ship it."},{"label":"Refuse"}],"multiSelect":false},
                    {"id":"bare","question":"Anything else?"}
                ],"state":"continued"}],
                "settled":[{"callId":"old","answers":[{"id":"x","selected":["yes"]}]}]
            }
        });
        let state = (definition.state_schema)(Some(&checkpoint)).unwrap();
        assert_eq!(
            state.questions.active[0].questions[0].detail.as_deref(),
            Some("# Plan")
        );
        assert!(state.questions.active[0].questions[1].options.is_none());
        let wire = definition.wire.unwrap();
        assert_eq!(
            (wire.view)(&state).unwrap().unwrap().as_ref(),
            &checkpoint["questions"]
        );
        assert!((wire.view_schema)(Some(&checkpoint["questions"])).is_ok());
    }
}
