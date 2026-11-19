//! Port of input-lifecycle.spec.ts: input atomicity and Agent/Session release under failure.
//!
//! Failure injection: the source throws from an `internal/dispatch` observer of `session/event`; the
//! Rust equivalent is a `SessionEventEmitted` listener rejecting the event through its `validation`.
//! The source mounts `@deepseek-ai/dsh-session-checkpoint-policy`, which has no Rust crate; its
//! checkpointing is not what these cases observe. "reports a quiescence failure while releasing the
//! Agent and Session" replaces `agent.whenIdle` by a spy and is not portable (see the report).
mod support_s17;

use hoh_agent::{Agent, AgentCreated, AgentPreStep, Agents, CancelOptions, PreStepDecision};
use hoh_session::{AgentCancelCause, SessionEventEmitted, SessionId, Sessions};
use hoh_timeout::AbortController;
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use support_s17::*;

struct Fixture {
    stack: Stack,
    agent: Arc<dyn Agent>,
    handle: hoh_agent::AgentHandle,
    _root: tempfile::TempDir,
}

fn input(text: &str) -> hoh_llm::UserMessage {
    user_message(text)
}

async fn harness() -> Fixture {
    let root = tempfile::tempdir().expect("temp dir");
    let stack = stack(
        MockAdapter::chunks((0..5).map(|_| text_response("ok")).collect()),
        StackOptions {
            persistence: Some(Persistence {
                config: json!({ "root": root.path().to_str().expect("utf-8"), "compression": "none" }),
                before_loop: true,
            }),
            ..StackOptions::default()
        },
    )
    .await;
    let mut options = hoh_agent::CreateAgentOptions::new(SessionId::from("diagnostic"));
    options.agent_options = Some(mock_agent_options());
    let handle = stack
        .ctx
        .get::<Agents>()
        .expect("agents")
        .create(&stack.ctx, options)
        .await
        .expect("agent creates");
    Fixture {
        agent: handle.agent.clone(),
        handle,
        stack,
        _root: root,
    }
}

/// Reject every `agent/inbox/spliced` event of the agent's session that `matches`, before it commits:
/// the `internal/dispatch` observer of the source that throws for `session/event`.
fn reject_splices(
    fixture: &Fixture,
    matches: impl Fn(&serde_json::Value) -> bool + Send + Sync + 'static,
    message: &'static str,
) -> hoh_kernel::Disposer {
    let session = fixture.agent.session().clone();
    fixture
        .stack
        .ctx
        .on_dispatch(move |_, name, event| {
            if name != "session/event" {
                return;
            }
            let Some(emitted) = event.downcast_ref::<SessionEventEmitted>() else {
                return;
            };
            if emitted.session == session
                && emitted.event.event_type() == "agent/inbox/spliced"
                && matches(emitted.event.data())
            {
                emitted.validation.reject(message.into());
            }
        })
        .expect("failure injector")
}

fn removed(data: &serde_json::Value) -> bool {
    data["removedCount"].as_u64().unwrap_or(0) > 0
}

#[tokio::test]
async fn cancellation_still_aborts_the_running_request_when_queue_clearing_fails() {
    let fixture = harness().await;
    let agent = fixture.agent.clone();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<hoh_timeout::AbortSignal>();
    let entered_tx = Mutex::new(Some(entered_tx));
    let release = Arc::new(tokio::sync::Notify::new());
    let gate = release.clone();
    let hold = fixture
        .stack
        .ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| {
            if let Some(entered) = lock(&entered_tx).take() {
                let _ = entered.send(event.signal.clone());
            }
            let gate = gate.clone();
            async move {
                gate.notified().await;
                next.run().await
            }
        })
        .expect("hold listener");
    agent.followup(input("running")).expect("followup");
    let signal = entered_rx.await.expect("pre-step entered");
    agent.inject(input("steering")).expect("inject");
    let queued = input("queued");
    agent.followup(queued.clone()).expect("queued followup");
    let stop_rejecting = reject_splices(
        &fixture,
        |data| data["target"] == "next-turn" && removed(data),
        "injected queue clear failure",
    );
    let error = agent
        .cancel(AgentCancelCause::User, CancelOptions::default())
        .expect_err("cancel reports the failed clear");
    assert!(
        error.to_string().contains("injected queue clear failure"),
        "{error}"
    );
    assert!(signal.aborted());
    assert_eq!(
        serde_json::to_value(agent.inbox().next_turn()).expect("queue"),
        serde_json::to_value([queued]).expect("expected")
    );
    stop_rejecting.dispose().await;
    hold.dispose().await;
    agent
        .cancel(
            AgentCancelCause::Disposed,
            CancelOptions { keep_inbox: true },
        )
        .expect("dispose cancel");
    release.notify_one();
    agent.when_idle().await;
    fixture.stack.close().await;
}

#[tokio::test]
async fn waits_for_admitted_maintenance_after_a_queue_clear_failure_before_releasing_its_session() {
    let fixture = harness().await;
    let agent = fixture.agent.clone();
    let release = Arc::new(tokio::sync::Notify::new());
    let scope_disposed = Arc::new(AtomicBool::new(false));
    let flag = scope_disposed.clone();
    agent
        .ctx()
        .effect("test.scope", move || {
            hoh_kernel::Undo::sync(move || flag.store(true, Ordering::SeqCst))
        })
        .expect("scope effect");
    let (gate, subject) = (release.clone(), agent.clone());
    let work = hoh_agent::run_maintenance(&*agent, move |_signal| async move {
        gate.notified().await;
        subject.session().append(
            "request/context",
            json!({ "provider": "mock", "model": "mock" }),
            None,
        )?;
        Ok(())
    })
    .expect("maintenance admitted");
    let work = tokio::spawn(work);
    agent.inject(input("pending")).expect("inject");
    let stop_rejecting = reject_splices(&fixture, removed, "queue clear failed");
    let closing = tokio::spawn(fixture.handle.dispose());
    // Disposal must be waiting for the admitted maintenance: neither the scope nor the Session is released.
    settle(25).await;
    assert!(!scope_disposed.load(Ordering::SeqCst));
    let sessions = fixture.stack.ctx.get::<Sessions>().expect("sessions");
    assert_eq!(sessions.get(agent.id()).as_ref(), Some(agent.session()));
    release.notify_one();
    work.await
        .expect("work joins")
        .expect("maintenance completes");
    let outcome = closing.await.expect("closing joins");
    let error = outcome.expect_err("disposal reports the failed clear");
    assert!(error.to_string().contains("queue clear failed"), "{error}");
    assert!(scope_disposed.load(Ordering::SeqCst));
    let events = agent.session().snapshot_events(None, None);
    assert_eq!(
        events.last().expect("events").event_type(),
        "request/context"
    );
    stop_rejecting.dispose().await;
    fixture.stack.close().await;
}

#[tokio::test]
async fn keeps_creation_resources_alive_until_an_interrupted_creation_listener_finishes() {
    let fixture = harness().await;
    let id = SessionId::from("interrupted-announcement");
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<Arc<dyn Agent>>();
    let entered_tx = Mutex::new(Some(entered_tx));
    let release = Arc::new(tokio::sync::Notify::new());
    let (gate, watched) = (release.clone(), id.clone());
    let stop = fixture
        .stack
        .ctx
        .on_serial::<AgentCreated, (), _>(move |event| {
            let entered = (event.agent.id() == &watched)
                .then(|| lock(&entered_tx).take())
                .flatten();
            let (agent, gate) = (event.agent.clone(), gate.clone());
            async move {
                if let Some(entered) = entered {
                    let _ = entered.send(agent);
                    gate.notified().await;
                }
                Ok(None)
            }
        })
        .expect("creation listener");
    let controller = AbortController::new();
    let mut options = hoh_agent::CreateAgentOptions::new(id.clone());
    options.agent_options = Some(mock_agent_options());
    options.signal = Some(controller.signal());
    let agents = fixture.stack.ctx.get::<Agents>().expect("agents");
    let creation = tokio::spawn({
        let create = agents.create(&fixture.stack.ctx, options);
        async move {
            match create.await {
                Ok(handle) => handle.dispose().await.err(),
                Err(error) => Some(error),
            }
        }
    });
    let agent = entered_rx.await.expect("listener entered");
    controller.abort_with("creation interrupted");
    let sessions = fixture.stack.ctx.get::<Sessions>().expect("sessions");
    assert_eq!(sessions.get(&id).as_ref(), Some(agent.session()));
    release.notify_one();
    let error = creation
        .await
        .expect("creation joins")
        .expect("creation rejected");
    assert!(
        error.to_string().contains("creation interrupted"),
        "{error}"
    );
    assert!(sessions.get(&id).is_none());
    stop.dispose().await;
    fixture.stack.close().await;
}
