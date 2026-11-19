//! Port of request-error.spec.ts: `agent/request-error` recovery of failed model requests.
mod support_s17;

use futures_util::stream;
use hoh_agent::{
    Agent, AgentRequest, AgentRequestError, AgentStatusChanged, CancelOptions, RequestErrorAction,
};
use hoh_error::BoxError;
use hoh_llm::{
    GenerateOptions, LlmAdapter, LlmCallConfig, LlmError, LlmFailure, LlmStream,
    ResolvedRetryPolicy, StreamChunk,
};
use hoh_session::AgentCancelCause;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use support_s17::*;

/// One scripted model call: chunks, or a thrown `LlmError.fromFailure({ message, code })`.
enum Scripted {
    Chunks(Vec<StreamChunk>),
    Fail(&'static str, &'static str),
}

struct ScriptAdapter {
    script: Mutex<VecDeque<Scripted>>,
    requests: Mutex<usize>,
}

impl ScriptAdapter {
    fn new(script: Vec<Scripted>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into()),
            requests: Mutex::new(0),
        })
    }
    fn request_count(&self) -> usize {
        *lock(&self.requests)
    }
}

fn failure(message: &str, code: &str) -> BoxError {
    let failure: LlmFailure =
        serde_json::from_value(json!({ "message": message, "code": code })).expect("failure");
    Box::new(LlmError::from_failure(&failure, None).expect("llm error"))
}

impl LlmAdapter for ScriptAdapter {
    fn stream(&self, _options: GenerateOptions) -> Result<LlmStream, BoxError> {
        *lock(&self.requests) += 1;
        match lock(&self.script).pop_front() {
            None => Err("MockAdapter: script exhausted".into()),
            Some(Scripted::Chunks(chunks)) => Ok(LlmStream::chunks(chunks)),
            Some(Scripted::Fail(message, code)) => Ok(LlmStream::new(stream::once(async move {
                Err::<StreamChunk, BoxError>(failure(message, code))
            }))),
        }
    }
}

async fn harness(adapter: Arc<ScriptAdapter>) -> Stack {
    stack(adapter, StackOptions::default()).await
}

fn go(agent: &Arc<dyn Agent>) {
    agent.followup(user_message("go")).expect("followup");
}

#[tokio::test]
async fn does_not_offer_middleware_failures_to_request_recovery() {
    let adapter = ScriptAdapter::new(vec![Scripted::Chunks(text_response("unused"))]);
    let stack = harness(adapter.clone()).await;
    let agent = stack.create_agent("request-error-narrow", "mock").await;
    let recoveries = Arc::new(AtomicUsize::new(0));
    stack
        .ctx
        .on_waterfall::<AgentRequest, LlmCallConfig, _>(|_, _| async {
            Err(failure("middleware failed", "MIDDLEWARE"))
        })
        .expect("request listener");
    let counted = recoveries.clone();
    stack
        .ctx
        .on_waterfall::<AgentRequestError, Option<RequestErrorAction>, _>(move |_, _| {
            counted.fetch_add(1, Ordering::SeqCst);
            async { Ok(None) }
        })
        .expect("request-error listener");

    go(&agent);
    agent.when_idle().await;

    assert_eq!(recoveries.load(Ordering::SeqCst), 0);
    assert_eq!(adapter.request_count(), 0);
    stack.close().await;
}

#[tokio::test]
async fn lets_each_failed_request_return_a_retry_action_before_its_turn_closes() {
    let adapter = ScriptAdapter::new(vec![
        Scripted::Fail("busy", "RATE_LIMIT"),
        Scripted::Fail("unavailable", "SERVICE_UNAVAILABLE"),
        Scripted::Chunks(text_response("ok")),
    ]);
    let stack = harness(adapter.clone()).await;
    let agent = stack.create_agent("request-error-retry", "mock").await;
    let seen = Arc::new(Mutex::new(Vec::<(u64, u64, String, bool)>::new()));
    let statuses = Arc::new(Mutex::new(Vec::<String>::new()));
    let (watched, log) = (agent.clone(), statuses.clone());
    stack
        .ctx
        .on_contained::<AgentStatusChanged>(move |event| {
            if event.agent.id() == watched.id() {
                lock(&log).push(event.status.as_str().to_owned());
            }
            Ok(())
        })
        .expect("status listener");
    let (expected, record) = (agent.clone(), seen.clone());
    stack
        .ctx
        .on_waterfall::<AgentRequestError, Option<RequestErrorAction>, _>(move |event, _| {
            assert_eq!(event.agent.id(), expected.id());
            lock(&record).push((
                event.turn,
                event.step,
                event.failure.code.clone(),
                matches!(event.retry_policy, Some(ResolvedRetryPolicy::Normal { .. })),
            ));
            async { Ok(Some(RequestErrorAction::Retry)) }
        })
        .expect("request-error listener");

    go(&agent);
    agent.when_idle().await;

    assert_eq!(
        *lock(&seen),
        [
            (1, 1, "RATE_LIMIT".to_owned(), true),
            (1, 1, "SERVICE_UNAVAILABLE".to_owned(), true),
        ]
    );
    assert_eq!(events_of(&agent, "turn/start").len(), 1);
    assert_eq!(*lock(&statuses), ["running", "idle"]);
    let reasons: Vec<Value> = events_of(&agent, "request/header")
        .iter()
        .map(|event| event["data"]["reason"].clone())
        .collect();
    assert_eq!(reasons, [json!("initial")]);
    stack.close().await;
}

#[tokio::test]
async fn lets_cancellation_win_over_a_retry_action() {
    let adapter = ScriptAdapter::new(vec![
        Scripted::Fail("busy", "RATE_LIMIT"),
        Scripted::Chunks(text_response("unused")),
    ]);
    let stack = harness(adapter.clone()).await;
    let agent = stack.create_agent("request-error-cancel", "mock").await;
    stack
        .ctx
        .on_waterfall::<AgentRequestError, Option<RequestErrorAction>, _>(|event, _| async move {
            event
                .agent
                .cancel(AgentCancelCause::User, CancelOptions::default())?;
            Ok(Some(RequestErrorAction::Retry))
        })
        .expect("request-error listener");

    go(&agent);
    agent.when_idle().await;

    assert_eq!(adapter.request_count(), 1);
    assert_eq!(events_of(&agent, "turn/start").len(), 1);
    let end = events_of(&agent, "turn/end")
        .into_iter()
        .next()
        .expect("turn end");
    assert_match_object(
        &end,
        &json!({ "type": "turn/end", "data": { "reason": { "kind": "aborted", "reason": { "kind": "user" } } } }),
    );
    stack.close().await;
}

#[tokio::test]
async fn does_not_retry_when_the_recovery_listener_fails_before_returning_its_action() {
    let adapter = ScriptAdapter::new(vec![
        Scripted::Fail("busy", "RATE_LIMIT"),
        Scripted::Chunks(text_response("unused")),
    ]);
    let stack = harness(adapter.clone()).await;
    let agent = stack
        .create_agent("request-error-recovery-failed", "mock")
        .await;
    stack
        .ctx
        .on_waterfall::<AgentRequestError, Option<RequestErrorAction>, _>(|_, _| async {
            Err::<Option<RequestErrorAction>, BoxError>("recovery failed".into())
        })
        .expect("request-error listener");

    go(&agent);
    agent.when_idle().await;

    assert_eq!(adapter.request_count(), 1);
    assert_eq!(events_of(&agent, "turn/start").len(), 1);
    let end = events_of(&agent, "turn/end")
        .into_iter()
        .next()
        .expect("turn end");
    assert_match_object(
        &end,
        &json!({ "type": "turn/end", "data": { "reason": { "kind": "error" } } }),
    );
    stack.close().await;
}
