//! Port of model.spec.ts: per-Agent model instances share the loop's projection registration and
//! teardown.
//!
//! Not ported (see the report): "waits for model projection registration before publishing the factory"
//! swaps `sessionProjections.register` for a spy while the plugin loads; `expect(agent.model).toBeInstanceOf
//! (AgentModel)` is a compile-time fact in Rust.
mod support_s17;

use hoh_agent::{AgentSetup, Agents, CreateAgentOptions, ModelSelection};
use hoh_agent_loop::AgentLoopPlugin;
use hoh_kernel::Undo;
use hoh_session::SessionId;
use hoh_session_projection::SessionProjections;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use support_s17::*;

fn selection(model: &str) -> ModelSelection {
    ModelSelection {
        provider: "mock".into(),
        model: model.into(),
        reasoning_effort: None,
    }
}

fn options(id: &str, setup: Option<AgentSetup>) -> CreateAgentOptions {
    let mut options = CreateAgentOptions::new(SessionId::from(id));
    options.agent_options = Some(AgentOptions {
        provider: Some(Live::Value("mock".into())),
        model: Some(Live::Value("initial".into())),
        ..AgentOptions::default()
    });
    options.setup = setup;
    options
}

use hoh_agent::{AgentOptions, Live};

fn model_selection_value(stack: &Stack, session: &hoh_session::Session) -> Option<Value> {
    let registry = stack.ctx.get::<SessionProjections>().expect("registry");
    let snapshot = registry.snapshot(session, None).expect("snapshot");
    snapshot
        .values
        .get("modelSelection")
        .map(|value| (**value).clone())
}

async fn dependencies() -> Stack {
    stack(MockAdapter::chunks(vec![]), StackOptions::default()).await
}

#[tokio::test]
async fn binds_each_model_instance_to_its_agent_and_makes_it_available_during_setup() {
    let stack = dependencies().await;
    let agents = stack.ctx.get::<Agents>().expect("agents");
    let setup: AgentSetup = Arc::new(|_ctx, agent| {
        Box::pin(async move {
            agent.model().select(&selection("selected"))?;
            Ok(None)
        })
    });
    let first = agents
        .create(&stack.ctx, options("first", Some(setup)))
        .await
        .expect("first creates");
    let second = agents
        .create(&stack.ctx, options("second", None))
        .await
        .expect("second creates");
    assert!(!std::ptr::addr_eq(
        first.agent.model(),
        second.agent.model()
    ));
    assert_eq!(first.agent.model().current(), Some(selection("selected")));
    assert_eq!(second.agent.model().current(), Some(selection("initial")));
    assert!(
        second
            .agent
            .session()
            .snapshot_events(None, None)
            .is_empty()
    );
    assert_eq!(
        model_selection_value(&stack, first.agent.session()),
        Some(json!({ "lastUsed": null, "next": { "provider": "mock", "model": "selected" } }))
    );
    stack.close().await;
}

async fn retains_model_selection_until_agent_cleanup_finishes(owner_is_loop: bool) {
    let stack = dependencies().await;
    let agents = stack.ctx.get::<Agents>().expect("agents");
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<()>();
    let entered_tx = Arc::new(Mutex::new(Some(entered_tx)));
    let release = Arc::new(tokio::sync::Notify::new());
    let gate = release.clone();
    let setup: AgentSetup = Arc::new(move |agent_ctx, agent| {
        let (entered, gate) = (entered_tx.clone(), gate.clone());
        Box::pin(async move {
            agent_ctx.effect("cleanup", move || {
                Undo::future(move || async move {
                    if let Some(entered) = lock(&entered).take() {
                        let _ = entered.send(());
                    }
                    gate.notified().await;
                    agent.model().select(&selection("selected"))?;
                    assert_eq!(agent.model().current(), Some(selection("selected")));
                    Ok(())
                })
            })?;
            Ok(None)
        })
    });
    let handle = agents
        .create(&stack.ctx, options("cleanup", Some(setup)))
        .await
        .expect("agent creates");
    let disposal = if owner_is_loop {
        tokio::spawn(stack.loop_fiber.dispose())
    } else {
        tokio::spawn(stack.kernel.shutdown())
    };
    entered_rx.await.expect("cleanup entered");
    assert_eq!(handle.agent.model().current(), Some(selection("initial")));
    release.notify_one();
    disposal.await.expect("disposal joins");
    let events = handle.agent.session().snapshot_events(None, None);
    let last = events.last().expect("events");
    assert_eq!(last.event_type(), "model/selection");
    assert_match_object(
        last.data(),
        &json!({ "provider": "mock", "model": "selected" }),
    );
    if owner_is_loop {
        assert_eq!(model_selection_value(&stack, handle.agent.session()), None);
        let fiber = stack
            .ctx
            .plugin::<AgentLoopPlugin>(json!({ "agents": [] }))
            .expect("loop remounts");
        fiber.settled().await.expect("loop settles");
        assert_eq!(
            model_selection_value(&stack, handle.agent.session()),
            Some(json!({ "lastUsed": null, "next": { "provider": "mock", "model": "selected" } }))
        );
        stack.close().await;
    }
}

#[tokio::test]
async fn retains_model_selection_until_agent_cleanup_finishes_on_loop_disposal() {
    retains_model_selection_until_agent_cleanup_finishes(true).await;
}

#[tokio::test]
async fn retains_model_selection_until_agent_cleanup_finishes_on_root_disposal() {
    retains_model_selection_until_agent_cleanup_finishes(false).await;
}
