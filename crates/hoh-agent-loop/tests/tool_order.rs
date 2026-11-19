//! Port of tool-order.spec.ts: loop-level tool-order determinism. The request/header event — and
//! therefore the frozen request the adapter receives — carries the assembly's canonical tool order
//! (system-prompt's `toolOrder` config, or lexicographic name order), regardless of the order tool
//! plugins happened to register in.
//!
//! `Object.isFrozen(request)` has no Rust counterpart (every value is immutable once it is owned by
//! the adapter); the loop's own marker `agent_loop_request`, which the reconstruction invariant keys
//! on, is asserted instead, in a separate case (`the_dispatched_request_keeps_the_loop_request_marker`).
mod support_s17;

use hoh_agent::Agent;
use hoh_system_prompt::TOOL_ORDER_REST;
use serde_json::{Value, json};
use std::sync::Arc;
use support_s17::*;

struct Run {
    stack: Stack,
    agent: Arc<dyn Agent>,
    adapter: Arc<MockAdapter>,
}

async fn harness(adapter: Arc<MockAdapter>, tool_order: Option<Value>) -> Stack {
    let mut system_prompt = json!({ "personaPrefix": "stable base" });
    if let Some(order) = tool_order {
        system_prompt["toolOrder"] = order;
    }
    stack(
        adapter,
        StackOptions {
            system_prompt,
            ..StackOptions::default()
        },
    )
    .await
}

fn register_named(stack: &Stack, name: &str) {
    stack
        .tools
        .register(
            &stack.ctx,
            text_tool(name, &format!("the {name} tool"), name),
            Default::default(),
        )
        .expect("tool registers");
}

/// Run one text-only turn and return the harness + agent.
async fn run_turn(registration_order: &[&str], tool_order: Option<Value>) -> Run {
    let adapter = MockAdapter::chunks(vec![text_response("done")]);
    let stack = harness(adapter.clone(), tool_order).await;
    for name in registration_order {
        register_named(&stack, name);
    }
    let agent = stack.create_agent("a1", "mock").await;
    send_and_wait(&agent, "go").await;
    Run {
        stack,
        agent,
        adapter,
    }
}

fn first_request_tools(run: &Run) -> Option<Vec<String>> {
    tool_names(&lock(&run.adapter.requests)[0])
}

fn names(items: &[&str]) -> Option<Vec<String>> {
    Some(items.iter().map(|s| (*s).to_owned()).collect())
}

#[tokio::test]
async fn logs_the_request_header_with_tools_in_canonical_order_not_registration_order() {
    let run = run_turn(&["zulu", "alpha", "mike"], None).await;
    assert_eq!(
        header_tool_names(&run.agent),
        names(&["alpha", "mike", "zulu"])
    );
    // The dispatched request is built FROM the logged header (whose tools the assembly already
    // canonicalized) and reaches the adapter marked as loop-built — the marker the reconstruction
    // invariant keys on.
    assert_eq!(first_request_tools(&run), names(&["alpha", "mike", "zulu"]));
    {
        let requests = lock(&run.adapter.requests);
        assert_eq!(
            requests[0].session_id.as_ref(),
            Some(run.agent.session().id())
        );
    }
    run.stack.close().await;
}

/// `Object.isFrozen(adapter.requests[0])` of the first two cases, as the loop's own marker.
#[tokio::test]
async fn the_dispatched_request_keeps_the_loop_request_marker() {
    let run = run_turn(&["zulu", "alpha", "mike"], None).await;
    assert!(lock(&run.adapter.requests)[0].agent_loop_request);
    run.stack.close().await;
}

#[tokio::test]
async fn produces_the_same_header_order_for_any_registration_order() {
    let first = run_turn(&["alpha", "mike", "zulu"], None).await;
    let second = run_turn(&["zulu", "mike", "alpha"], None).await;
    assert_eq!(
        header_tool_names(&first.agent),
        names(&["alpha", "mike", "zulu"])
    );
    assert_eq!(
        header_tool_names(&second.agent),
        header_tool_names(&first.agent)
    );
    first.stack.close().await;
    second.stack.close().await;
}

#[tokio::test]
async fn honors_a_configured_tool_order_in_the_logged_header_and_the_dispatched_request() {
    let run = run_turn(
        &["alpha", "zulu", "mike"],
        Some(json!(["zulu", TOOL_ORDER_REST])),
    )
    .await;
    assert_eq!(
        header_tool_names(&run.agent),
        names(&["zulu", "alpha", "mike"])
    );
    assert_eq!(first_request_tools(&run), names(&["zulu", "alpha", "mike"]));
    run.stack.close().await;
}

#[tokio::test]
async fn closes_a_no_step_turn_when_tool_order_names_an_unregistered_tool() {
    let adapter = MockAdapter::chunks(vec![text_response("never sent")]);
    let stack = harness(adapter.clone(), Some(json!(["ghost", TOOL_ORDER_REST]))).await;
    register_named(&stack, "alpha");
    let agent = stack.create_agent("a1", "mock").await;
    send_and_wait(&agent, "go").await;
    assert_eq!(adapter.request_count(), 0);
    assert_eq!(header_tool_names(&agent), None);
    let types = event_types(&agent);
    assert!(types.iter().any(|t| t == "turn/start"));
    assert!(types.iter().any(|t| t == "turn/end"));
    assert!(!types.iter().any(|t| t == "step/start"));
    assert!(!types.iter().any(|t| t == "step/end"));
    stack.close().await;
}
