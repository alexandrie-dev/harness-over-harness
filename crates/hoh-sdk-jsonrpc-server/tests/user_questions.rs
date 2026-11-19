mod support;
use hoh_agent::{Agents, CreateAgentOptions};
use hoh_error::HarnessError;
use hoh_protocol::{AskUserQuestionItem, SdkUserQuestionAnswer};
use hoh_sdk_jsonrpc_server::{
    user_question_helpers::UserQuestionError, user_questions::AskUserQuestionRequest,
};
use hoh_timeout::AbortController;
use serde_json::{Value, json};
use std::sync::Arc;
use support::*;
fn questions() -> Vec<AskUserQuestionItem> {
    serde_json::from_value(json!([{"id":"format","question":"Which format?","options":[{"label":"CSV"},{"label":"JSON"}]}])).unwrap()
}
fn answer() -> Value {
    json!({"answers":[{"id":"format","selected":["CSV"],"custom":"include a header"}]})
}
async fn setup(enabled: bool) -> Env {
    let h = Env::new().await;
    let mut p = h.params();
    p.capabilities = Some(serde_json::from_value(json!({"userQuestions":enabled})).unwrap());
    h.server.initialize(p).await.unwrap();
    h.open("main").await;
    h
}
async fn ask(
    h: &Env,
    agent: Option<Arc<dyn hoh_agent::Agent>>,
    signal: Option<hoh_timeout::AbortSignal>,
) -> Result<SdkUserQuestionAnswer, hoh_error::BoxError> {
    h.ctx
        .waterfall(
            AskUserQuestionRequest {
                agent,
                questions: questions(),
                signal,
            },
            |_| async { Err(UserQuestionError::new("no provider", "NO_PROVIDER").into()) },
        )
        .await
}
fn code(error: &hoh_error::BoxError) -> String {
    HarnessError::of(error.as_ref()).unwrap().code().into()
}
#[tokio::test]
async fn forwards_the_original_questions_and_maps_the_host_answer_to_the_sdk_owned_root() {
    let h = setup(true).await;
    h.peer.answer(answer());
    assert_eq!(
        serde_json::to_value(ask(&h, Some(h.agent("main")), None).await.unwrap()).unwrap(),
        answer()
    );
    {
        let r = lock(&h.peer.requests);
        assert_eq!(r[0].0, "user-question/ask");
        assert!(r[0].1["requestId"].as_str().is_some_and(|s| s.len() == 36));
        assert_eq!(r[0].1["sessionId"], "main");
        assert_eq!(
            r[0].1["questions"],
            serde_json::to_value(questions()).unwrap()
        );
    }
    assert!(h.peer.events("user-question.cancelled").is_empty());
    h.close().await;
}
#[tokio::test]
async fn keeps_native_answerers_when_the_sdk_host_did_not_advertise_question_support() {
    let h = setup(false).await;
    assert_eq!(
        code(&ask(&h, Some(h.agent("main")), None).await.unwrap_err()),
        "NO_PROVIDER"
    );
    h.ctx
        .on_waterfall::<AskUserQuestionRequest, SdkUserQuestionAnswer, _>(|_, _| async {
            Ok(serde_json::from_value(answer()).unwrap())
        })
        .unwrap();
    assert_eq!(
        serde_json::to_value(ask(&h, Some(h.agent("main")), None).await.unwrap()).unwrap(),
        answer()
    );
    assert!(lock(&h.peer.requests).is_empty());
    h.close().await;
}
#[tokio::test]
async fn does_not_forward_questions_belonging_to_another_runtime_root() {
    let h = setup(true).await;
    let foreign = h
        .ctx
        .get::<Agents>()
        .unwrap()
        .create(&h.ctx, CreateAgentOptions::new("foreign".into()))
        .await
        .unwrap();
    assert_eq!(
        code(
            &ask(&h, Some(foreign.agent.clone()), None)
                .await
                .unwrap_err()
        ),
        "NO_PROVIDER"
    );
    assert!(lock(&h.peer.requests).is_empty());
    foreign.dispose().await.unwrap();
    h.close().await;
}
#[tokio::test]
async fn rejects_an_invalid_host_answer_without_treating_it_as_approval() {
    let h = setup(true).await;
    for value in [
        Value::Null,
        json!(42),
        json!({}),
        json!({"answers":false}),
        json!({"answers":[null]}),
        json!({"answers":["wrong"]}),
        json!({"answers":[{"selected":[]}]}),
        json!({"answers":[{"id":42,"selected":[]}]}),
        json!({"answers":[{"id":"format"}]}),
        json!({"answers":[{"id":"format","selected":false}]}),
        json!({"answers":[]}),
        json!({"answers":[{"id":"wrong","selected":[]}]}),
        json!({"answers":[{"id":"format","selected":[]},{"id":"format","selected":[]}]}),
        json!({"answers":[{"id":"format","selected":[1]}]}),
        json!({"answers":[{"id":"format","selected":[],"custom":false}]}),
        json!({"cancelled":false}),
        json!({"cancelled":true,"answers":[]}),
    ] {
        h.peer.answer(value.clone());
        assert_eq!(
            code(&ask(&h, Some(h.agent("main")), None).await.unwrap_err()),
            "BAD_ANSWER",
            "{value}"
        );
    }
    h.close().await;
}
#[tokio::test]
async fn maps_an_explicit_user_dismissal_to_native_ask_cancelled() {
    let h = setup(true).await;
    h.peer.answer(json!({"cancelled":true}));
    assert_eq!(
        code(&ask(&h, Some(h.agent("main")), None).await.unwrap_err()),
        "ASK_CANCELLED"
    );
    assert!(h.peer.events("user-question.cancelled").is_empty());
    h.close().await;
}
async fn cancelled(shutdown: bool, broken_notify: bool) {
    let h = setup(true).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let notified = entered.clone();
    *lock(&h.peer.host) = Some(Arc::new(move |_, _, signal| {
        let entered = notified.clone();
        Box::pin(async move {
            entered.notify_one();
            Err(signal.unwrap().cancelled().await.into())
        })
    }));
    if broken_notify {
        *lock(&h.peer.notify_failure) = Some("broken output".into());
    }
    let control = AbortController::new();
    let signal = control.signal();
    let ctx = h.ctx.clone();
    let agent = h.agent("main");
    let pending = tokio::spawn(async move {
        ctx.waterfall(
            AskUserQuestionRequest {
                agent: Some(agent),
                questions: questions(),
                signal: Some(signal),
            },
            |_| async { Err("must not delegate".into()) },
        )
        .await
    });
    entered.notified().await;
    if shutdown {
        h.server.shutdown().await.unwrap();
    } else {
        control.abort_with("user cancelled the tool");
    }
    let e = pending.await.unwrap().unwrap_err();
    assert_eq!(code(&e), "ASK_ABORTED");
    if broken_notify {
        assert!(
            h.ctx
                .logger()
                .buffer()
                .iter()
                .any(|r| r.message.contains("cancellation could not reach the host"))
        );
    } else {
        let notices = h.peer.events("user-question.cancelled");
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0]["sessionId"], "main");
        assert!(notices[0]["requestId"].is_string());
    }
    h.close().await;
}
#[tokio::test]
async fn cancels_an_outstanding_host_request_on_tool_cancellation() {
    cancelled(false, false).await;
}
#[tokio::test]
async fn cancels_an_outstanding_host_request_on_shutdown() {
    cancelled(true, false).await;
}
#[tokio::test]
async fn contains_cancellation_notification_write_failure_while_aborting_the_pending_question() {
    cancelled(false, true).await;
}
#[tokio::test]
async fn reports_a_host_or_transport_failure_and_does_not_synthesize_an_answer() {
    let h = setup(true).await;
    assert_eq!(
        code(&ask(&h, Some(h.agent("main")), None).await.unwrap_err()),
        "PROVIDER_ERROR"
    );
    h.close().await;
}
#[tokio::test]
async fn source_constructor_returns_shared_harness_error_with_cause() {
    let error = UserQuestionError::new("message", "ASK_ABORTED").with_cause("cause");
    assert_eq!(error.name(), "UserQuestionError");
    assert_eq!(error.code(), "ASK_ABORTED");
    assert_eq!(error.cause().unwrap().to_string(), "cause");
}
#[tokio::test]
async fn response_sanitizes_unknown_keys_and_keeps_selected_order() {
    let result=hoh_sdk_jsonrpc_server::user_questions::parse_host_answer(json!({"ignored":true,"answers":[{"id":"format","selected":["JSON","CSV"],"custom":"","extra":true}]}),&["format"]).unwrap();
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({"answers":[{"id":"format","selected":["JSON","CSV"],"custom":""}]})
    );
}
