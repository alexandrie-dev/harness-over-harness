//! Port of image-offload.spec.ts: an adapter's `IMAGE_OFFLOAD_REQUIRED` failure records exact
//! image occurrences and retries without replacing messages.
//!
//! Not ported (they need `hoh-compaction-basic` and `hoh-token-meter`, still `todo`): the six
//! `summary image offload` cases that drive `compactNow`/`compactRegion`. See PORT_NOTES.md.
#[path = "support/loop_harness.rs"]
mod loop_harness;
mod support;

use hoh_agent::{Agent, AgentRequestError, CancelOptions, RequestErrorAction};
use hoh_compaction_image_offload::shim::CompactionSummaryError;
use hoh_error::SharedError;
use hoh_llm::{BuiltinLlmFailureCode, IMAGE_OFFLOAD_REQUIRED_CODE, LlmError, LlmErrorOptions};
use hoh_session::{AgentCancelCause, SessionEventEmitted, SessionSeq, SurfaceIntent};
use hoh_timeout::{AbortController, AbortSignal};
use loop_harness::*;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use support::*;

fn followup(agent: &Arc<dyn Agent>, content: Vec<Value>) {
    agent
        .followup(serde_json::from_value(user_message(content)).expect("user message"))
        .expect("followup");
}

/// `ctx.on('agent/request-error', ...)` that records `failure.code` and delegates.
fn delegating_listener(env: &Env) -> Arc<Mutex<Vec<String>>> {
    let delegated = Arc::new(Mutex::new(Vec::new()));
    let sink = delegated.clone();
    env.ctx
        .on_waterfall::<AgentRequestError, Option<RequestErrorAction>, _>(move |event, next| {
            lock(&sink).push(event.failure.code.clone());
            next.run()
        })
        .expect("listener");
    delegated
}

fn turn_end_reason(session: &hoh_session::Session) -> Value {
    session
        .snapshot_events(None, None)
        .iter()
        .find(|event| event.event_type() == "turn/end")
        .expect("turn/end")
        .data()["reason"]
        .clone()
}

/// `ctx.on('session/event', ...)`: cancel the agent when `event_type` is emitted on its session.
fn cancel_on(env: &Env, agent: &Arc<dyn Agent>, event_type: &'static str) {
    let agent = agent.clone();
    env.ctx
        .on_contained::<SessionEventEmitted>(move |emitted| {
            if &emitted.session == agent.session() && emitted.event.event_type() == event_type {
                agent
                    .cancel(AgentCancelCause::User, CancelOptions::default())
                    .expect("cancel");
            }
            Ok(())
        })
        .expect("listener");
}

// ---- summary image offload (the cases that do not need the compaction engine) ----

async fn summary_error(
    env: &Env,
    session: &hoh_session::Session,
    source_event_seqs: Vec<SessionSeq>,
    error: SharedError,
    signal: Option<AbortSignal>,
) -> Result<bool, hoh_kernel::Error> {
    env.ctx
        .waterfall::<CompactionSummaryError, bool, _>(
            CompactionSummaryError {
                session: session.clone(),
                source_event_seqs,
                error,
                signal,
            },
            |_| async { Ok(false) },
        )
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn delegates_unhandled_summary_errors_plain_error() {
    let env = harness(vec![]).await;
    let agent = env.create_agent("summary-offload").await;
    let error = SharedError::new(Box::<dyn std::error::Error + Send + Sync>::from(
        "summary failed",
    ));
    assert!(
        !summary_error(&env, agent.session(), vec![], error, None)
            .await
            .unwrap()
    );
    assert!(decisions(agent.session()).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn delegates_unhandled_summary_errors_llm_error_without_a_count() {
    let env = harness(vec![]).await;
    let agent = env.create_agent("summary-offload").await;
    let error = SharedError::new(
        LlmError::new("no count", BuiltinLlmFailureCode::ImageOffloadRequired).unwrap(),
    );
    assert!(
        !summary_error(&env, agent.session(), vec![], error, None)
            .await
            .unwrap()
    );
    assert!(decisions(agent.session()).is_empty());
}

// ---- compaction-image-offload ----

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_at_failed_attempt_settlement_preserves_images_for_later_turns() {
    let env = harness(vec![offload_required(1), text_response("continued")]).await;
    let agent = env.create_agent("offload-cancel-before-record").await;
    cancel_on(&env, &agent, "assistant/attempt");
    followup(&agent, vec![image_named("original")]);
    agent.when_idle().await;
    assert_eq!(turn_end_reason(agent.session())["kind"], "aborted");
    assert_eq!(env.adapter.request_count(), 1);
    assert!(decisions(agent.session()).is_empty());

    followup(&agent, vec![text("continue")]);
    agent.when_idle().await;
    assert_eq!(env.adapter.request_count(), 2);
    assert!(offloaded_names(&env.adapter.request(1)).is_empty());
    let messages = agent.session().derive_messages().unwrap();
    assert!(
        messages
            .iter()
            .flat_map(|message| message["content"].as_array().unwrap().clone())
            .any(|block| block == image_named("original"))
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "ждёт hoh-agent-loop: запрос без сообщения пользователя (s6 исправляет)"]
async fn cancelling_after_an_image_omission_stops_retry_and_retains_the_committed_selection() {
    let env = harness(vec![offload_required(1), text_response("continued")]).await;
    let agent = env.create_agent("offload-cancel-after-record").await;
    cancel_on(&env, &agent, "image/offload");
    followup(&agent, vec![image_named("original")]);
    agent.when_idle().await;
    assert_eq!(turn_end_reason(agent.session())["kind"], "aborted");
    assert_eq!(env.adapter.request_count(), 1);
    assert_eq!(decisions(agent.session()).len(), 1);

    followup(&agent, vec![text("continue")]);
    agent.when_idle().await;
    assert_eq!(env.adapter.request_count(), 2);
    assert_eq!(offloaded_names(&env.adapter.request(1)), ["original"]);
    assert_eq!(decisions(agent.session()).len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "ждёт hoh-agent-loop: запрос без сообщения пользователя (s6 исправляет)"]
async fn logs_one_exact_image_selection_and_retries_without_replacing_the_message() {
    let env = harness(vec![offload_required(2), text_response("sent")]).await;
    let agent = env.create_agent("offload-required").await;
    let delegated = delegating_listener(&env);

    followup(
        &agent,
        vec![image_named("a"), image_named("b"), image_named("c")],
    );
    agent.when_idle().await;

    assert_eq!(env.adapter.request_count(), 2);
    assert!(offloaded_names(&env.adapter.request(0)).is_empty());
    assert_eq!(offloaded_names(&env.adapter.request(1)), ["a", "b"]);
    assert!(lock(&delegated).is_empty());
    let session = agent.session();
    let events = session.snapshot_events(None, None);
    let types = event_types(session);
    let count = |name: &str| types.iter().filter(|t| *t == name).count();
    assert_eq!(count("llm/retry"), 0);
    assert_eq!(count("assistant/attempt"), 1);
    assert!(replacements(session).is_empty());
    assert!(!types.contains(&"compaction/prune".to_owned()));
    assert_eq!(decisions(session).len(), 1);
    let decision = &decisions(session)[0];
    let original = events
        .iter()
        .find(|event| event.event_type() == "user/message")
        .unwrap()
        .seq();
    assert_eq!(
        *decision.data(),
        json!({ "targets": [{ "seq": original, "imageIndexes": [0, 1] }] })
    );
    let user = &events[u64::from(original) as usize];
    assert_eq!(user.event_type(), "user/message");
    assert_eq!(user.surface_op(), Some(&json!("append")));
    let position = |name: &str| types.iter().position(|t| t == name).unwrap() as u64;
    assert!(position("assistant/attempt") < u64::from(decision.seq()));
    assert!(u64::from(decision.seq()) < position("assistant/message"));
    let header = &events[u64::from(decision.seq()) as usize + 1];
    assert_eq!(header.event_type(), "request/header");
    assert_eq!(header.data()["reason"], "series");
    assert!(user.data()["content"][0].get("offloaded").is_none());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "ждёт hoh-agent-loop: запрос без сообщения пользователя (s6 исправляет)"]
async fn counts_the_adapter_prefix_in_request_order_after_a_surface_replacement() {
    let env = harness(vec![offload_required(2), text_response("sent")]).await;
    let agent = env.create_agent("offload-reordered").await;
    let session = agent.session();
    // An empty-content assistant node derives no message and carries no occurrence.
    session
        .append(
            "assistant/message",
            json!({ "turn": 0, "step": 0, "message": assistant_message(vec![]), "stream": [] }),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    let first = session
        .append(
            "user/message",
            user_message(vec![image_named("first")]),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    session
        .append(
            "user/message",
            user_message(vec![image_named("second")]),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    session
        .append(
            "user/message",
            user_message(vec![image_named("replacement")]),
            Some(SurfaceIntent::replace(
                first.seq(),
                first.seq(),
                vec![first.seq()],
            )),
        )
        .unwrap();

    followup(&agent, vec![text("send")]);
    agent.when_idle().await;

    assert_eq!(env.adapter.request_count(), 2);
    assert_eq!(
        offloaded_names(&env.adapter.request(1)),
        ["replacement", "second"]
    );
    assert_eq!(replacements(session).len(), 1);
    let targets: Vec<Value> = decisions(session)
        .iter()
        .map(|event| event.data()["targets"].clone())
        .collect();
    assert_eq!(
        targets,
        [json!([
            { "seq": 3, "imageIndexes": [0] },
            { "seq": 2, "imageIndexes": [0] },
        ])]
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "ждёт hoh-agent-loop: запрос без сообщения пользователя (s6 исправляет)"]
async fn offloads_a_tool_result_image_and_leaves_later_images_untouched() {
    let env = harness(vec![offload_required(1), text_response("sent")]).await;
    let agent = env.create_agent("offload-tool-result").await;
    let session = agent.session();
    let call = |id: &str| json!({ "type": "tool-call", "id": id, "name": "read_image", "arguments": "{}" });
    let tool_call = |id: &str| json!({ "turn": 0, "step": 1, "callId": id, "name": "read_image", "arguments": "{}" });
    let tool_result = |id: &str, content: Vec<Value>| json!({ "turn": 0, "step": 1, "message": tool_result_message(id, content) });
    session
        .append("turn/start", json!({ "turn": 0 }), None)
        .unwrap();
    session
        .append(
            "assistant/message",
            json!({
                "turn": 0, "step": 1,
                "message": assistant_message(vec![call("empty"), call("shot"), call("inner")]),
                "stream": [],
            }),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    session
        .append("tool/call", tool_call("empty"), None)
        .unwrap();
    session
        .append(
            "tool/result",
            tool_result("empty", vec![text("no image")]),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    session
        .append("tool/call", tool_call("shot"), None)
        .unwrap();
    let result = session
        .append(
            "tool/result",
            tool_result("shot", vec![image_named("first")]),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    session
        .append("tool/call", tool_call("inner"), None)
        .unwrap();
    session
        .append(
            "tool/result",
            tool_result("inner", vec![image_named("second")]),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    session
        .append(
            "turn/end",
            json!({ "turn": 0, "reason": { "kind": "completed" } }),
            None,
        )
        .unwrap();

    followup(&agent, vec![text("send")]);
    agent.when_idle().await;

    assert_eq!(offloaded_names(&env.adapter.request(1)), ["first"]);
    assert!(replacements(session).is_empty());
    assert_eq!(
        *decisions(session)[0].data(),
        json!({ "targets": [{ "seq": result.seq(), "imageIndexes": [0] }] })
    );
    assert_eq!(
        session.derive_event_message(&result).unwrap().unwrap()["source"],
        result.data()["message"]["source"]
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "ждёт hoh-agent-loop: запрос без сообщения пользователя (s6 исправляет)"]
async fn advances_across_consecutive_failures_and_preserves_the_first_request_snapshot() {
    let env = harness(vec![
        offload_required(1),
        offload_required(1),
        text_response("sent"),
    ])
    .await;
    let agent = env.create_agent("offload-repeat").await;
    followup(
        &agent,
        vec![
            image_named("first"),
            image_named("second"),
            image_named("third"),
        ],
    );
    agent.when_idle().await;
    assert_eq!(
        env.adapter.offloaded_names_per_request(),
        [
            vec![],
            vec!["first".to_owned()],
            vec!["first".to_owned(), "second".to_owned()]
        ]
    );
    let indexes: Vec<Value> = decisions(agent.session())
        .iter()
        .map(|event| event.data()["targets"][0]["imageIndexes"].clone())
        .collect();
    assert_eq!(indexes, [json!([0]), json!([1])]);
    let surface = agent.session().surface().unwrap();
    assert_eq!(surface.replace_generation().unwrap(), 0);
    assert_eq!(surface.content_generation().unwrap(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn removes_the_recovery_listener_when_its_plugin_is_disposed() {
    let mut env = Env::dependencies(ScriptedAdapter::new(vec![offload_required(1)])).await;
    let fiber = env.mount_offload().await;
    fiber.dispose().await;
    env.register_adapter();
    env.mount_agent_loop().await;
    let agent = env.create_agent("offload-unloaded").await;
    followup(&agent, vec![image_named("a")]);
    agent.when_idle().await;
    assert_eq!(env.adapter.request_count(), 1);
    assert!(decisions(agent.session()).is_empty());
    let error = SharedError::new(
        LlmError::with_options(
            "summary budget exceeded",
            BuiltinLlmFailureCode::ImageOffloadRequired,
            LlmErrorOptions {
                offload_images: Some(1),
                ..Default::default()
            },
        )
        .unwrap(),
    );
    let nodes = agent.session().surface().unwrap().nodes().unwrap();
    assert!(
        !summary_error(&env, agent.session(), nodes, error, None)
            .await
            .unwrap()
    );
    assert!(decisions(agent.session()).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn leaves_every_other_failure_to_downstream_recovery() {
    let env = harness(vec![server_failure()]).await;
    let agent = env.create_agent("offload-other-failure").await;
    let delegated = delegating_listener(&env);
    followup(&agent, vec![image_named("a")]);
    agent.when_idle().await;
    assert_eq!(*lock(&delegated), ["SERVER"]);
    assert!(replacements(agent.session()).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn delegates_image_offload_required_once_nothing_remains_to_offload() {
    let env = harness(vec![offload_required(1)]).await;
    let agent = env.create_agent("offload-exhausted").await;
    let delegated = delegating_listener(&env);
    followup(&agent, vec![text("no images")]);
    agent.when_idle().await;
    assert_eq!(*lock(&delegated), [IMAGE_OFFLOAD_REQUIRED_CODE]);
    assert!(replacements(agent.session()).is_empty());
    let events = agent.session().snapshot_events(None, None);
    let last = events.last().unwrap();
    assert_eq!(last.event_type(), "turn/end");
    assert_eq!(last.data()["reason"]["kind"], "error");
}

// ---- the summary-error listener, driven through the event itself ----
// Added: the source reaches these paths only through `compactNow`/`compactRegion`, which need the
// compaction engine; they stand in for the six cases listed as not ported.

async fn agent_with_images(
    env: &Env,
    id: &str,
    names: &[&str],
) -> (Arc<dyn Agent>, Vec<SessionSeq>) {
    let agent = env.create_agent(id).await;
    let session = agent.session();
    let content: Vec<Value> = names.iter().map(|name| image_named(name)).collect();
    session
        .append(
            "user/message",
            user_message(content),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    let nodes = session.surface().unwrap().nodes().unwrap();
    (agent, nodes)
}

fn offload_error(count: u64) -> SharedError {
    SharedError::new(
        LlmError::with_options(
            "summary budget exceeded",
            BuiltinLlmFailureCode::ImageOffloadRequired,
            LlmErrorOptions {
                offload_images: Some(count),
                ..Default::default()
            },
        )
        .unwrap(),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn summary_error_records_a_decision_for_the_selected_span_and_stops_when_exhausted() {
    let env = harness(vec![]).await;
    let (agent, nodes) = agent_with_images(&env, "summary-span", &["first", "second"]).await;
    let session = agent.session();
    // Only the selected events are visited; one occurrence per decision.
    assert!(
        summary_error(&env, session, nodes.clone(), offload_error(1), None)
            .await
            .unwrap()
    );
    assert!(
        summary_error(&env, session, nodes.clone(), offload_error(1), None)
            .await
            .unwrap()
    );
    assert!(
        !summary_error(&env, session, nodes.clone(), offload_error(1), None)
            .await
            .unwrap()
    );
    let targets: Vec<Value> = decisions(session)
        .iter()
        .map(|event| event.data()["targets"].clone())
        .collect();
    assert_eq!(
        targets,
        [
            json!([{ "seq": nodes[0], "imageIndexes": [0] }]),
            json!([{ "seq": nodes[0], "imageIndexes": [1] }]),
        ]
    );
    assert_eq!(session.surface().unwrap().replace_generation().unwrap(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn summary_error_does_not_record_a_decision_after_cancellation() {
    let env = harness(vec![]).await;
    let (agent, nodes) = agent_with_images(&env, "summary-cancel", &["first"]).await;
    let controller = AbortController::new();
    controller.abort_with("cancel summary");
    let reason = summary_error(
        &env,
        agent.session(),
        nodes,
        offload_error(1),
        Some(controller.signal()),
    )
    .await
    .expect_err("the cancellation reason is thrown");
    assert_eq!(reason.to_string(), "cancel summary");
    assert!(decisions(agent.session()).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn summary_error_of_another_kind_is_delegated() {
    let env = harness(vec![]).await;
    let (agent, nodes) = agent_with_images(&env, "summary-other", &["first"]).await;
    let other =
        SharedError::new(LlmError::new("provider outage", BuiltinLlmFailureCode::Server).unwrap());
    assert!(
        !summary_error(&env, agent.session(), nodes, other, None)
            .await
            .unwrap()
    );
    assert!(decisions(agent.session()).is_empty());
}
