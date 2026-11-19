//! Port of resource-lifecycle.spec.ts.
//!
//! Ported: "retains Agent resources until all work exits" with 0 failed teardowns. The 1 and 2 failed
//! teardown variants stub `agent.cancel` with a spy that throws, and the other two cases spy on
//! `agent.cancel` / `fiber.dispose` as well; "unloads %s while a background child is stopping" also
//! needs `dsh-subagent`, `dsh-jobs-local`, `dsh-subagent-spawn-in-process` and `dsh-tool-subagent`,
//! which have no Rust crates in this workspace. None of these can be expressed against the loop's
//! trait objects (see the report).
mod support_s17;

use hoh_agent::{Agent, Agents, CancelOptions};
use hoh_kernel::{Context, Fiber, Undo};
use hoh_session::{AgentCancelCause, SessionId, Sessions};
use hoh_session_projection::SessionProjections;
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use support_s17::*;

/// An owner fiber with its own context (`ctx.plugin(Object.assign(inner => …, { inject: ['agents'] }))`).
async fn owner(stack: &Stack) -> (Fiber, Context) {
    let slot = Arc::new(Mutex::new(None));
    let capture = slot.clone();
    let fiber = stack
        .ctx
        .inject::<(), _>(move |ctx, ()| {
            *lock(&capture) = Some(ctx);
            async { Ok(()) }
        })
        .expect("owner mounts");
    fiber.settled().await.expect("owner settles");
    let ctx = lock(&slot).clone().expect("owner context");
    (fiber, ctx)
}

#[tokio::test]
async fn retains_agent_resources_until_all_work_exits_with_0_failed_teardowns() {
    let stack = stack(MockAdapter::chunks(vec![]), StackOptions::default()).await;
    let release = Arc::new(tokio::sync::Notify::new());
    let (owner_fiber, owner_ctx) = owner(&stack).await;
    let mut options = hoh_agent::CreateAgentOptions::new(SessionId::from("pending-cleanup"));
    options.agent_options = Some(mock_agent_options());
    let agent: Arc<dyn Agent> = owner_ctx
        .get::<Agents>()
        .expect("agents")
        .create(&owner_ctx, options)
        .await
        .expect("agent creates")
        .agent;
    let (aborted_tx, aborted_rx) = tokio::sync::oneshot::channel::<()>();
    let work_exited = Arc::new(AtomicBool::new(false));
    let scope_disposed = Arc::new(AtomicBool::new(false));
    let scope_disposed_before_work_exit = Arc::new(AtomicBool::new(false));
    let abort_handled = Arc::new(AtomicUsize::new(0));
    let controller = hoh_timeout::AbortController::new();
    {
        let (signal, subject, handled) =
            (controller.signal(), agent.clone(), abort_handled.clone());
        tokio::spawn(async move {
            signal.cancelled().await;
            for _ in 0..2 {
                subject
                    .cancel(AgentCancelCause::Parent, CancelOptions::default())
                    .expect("cancel");
            }
            handled.fetch_add(1, Ordering::SeqCst);
        });
    }
    {
        let (exited, disposed, before, controller) = (
            work_exited.clone(),
            scope_disposed.clone(),
            scope_disposed_before_work_exit.clone(),
            controller,
        );
        agent
            .ctx()
            .effect("test.scope", move || {
                Undo::sync(move || {
                    controller.abort();
                    controller.abort();
                    disposed.store(true, Ordering::SeqCst);
                    before.store(!exited.load(Ordering::SeqCst), Ordering::SeqCst);
                })
            })
            .expect("scope effect");
    }
    let projections = stack.ctx.get::<SessionProjections>().expect("projections");
    let (gate, exited) = (release.clone(), work_exited.clone());
    let aborted_tx = Mutex::new(Some(aborted_tx));
    let work = hoh_agent::run_maintenance(&*agent, move |signal| async move {
        tokio::spawn(async move {
            signal.cancelled().await;
            if let Some(aborted) = lock(&aborted_tx).take() {
                let _ = aborted.send(());
            }
        });
        gate.notified().await;
        exited.store(true, Ordering::SeqCst);
        Ok(())
    })
    .expect("maintenance admitted");
    let work = tokio::spawn(work);
    let loop_fiber = stack.loop_fiber.clone();
    let disposal = tokio::spawn(async move { loop_fiber.dispose().await });
    aborted_rx.await.expect("work observed the abort");
    let owner_disposal = tokio::spawn(owner_fiber.dispose());
    // Drain runnable unload callbacks while the Agent is held at an explicit barrier.
    tokio::task::yield_now().await;
    release.notify_one();
    work.await.expect("work joins").expect("work completes");
    disposal.await.expect("loop disposal joins");
    owner_disposal.await.expect("owner disposal joins");
    assert!(!scope_disposed_before_work_exit.load(Ordering::SeqCst));
    assert!(scope_disposed.load(Ordering::SeqCst));
    assert_eq!(abort_handled.load(Ordering::SeqCst), 1);
    // The released inbox unit keeps this Session's last row; a Session created after the unload gets none.
    let checkpoint = projections.checkpoint(agent.session()).expect("checkpoint");
    assert!(checkpoint.contains_key("inbox"));
    let sessions = stack.ctx.get::<Sessions>().expect("sessions");
    let fresh = sessions
        .create(&stack.ctx, None, Default::default())
        .expect("fresh session");
    assert!(
        !projections
            .checkpoint(&fresh)
            .expect("checkpoint")
            .contains_key("inbox")
    );
    assert!(checkpoint.contains_key("turnBoundary"));
    assert!(
        stack
            .ctx
            .get::<Agents>()
            .expect("agents")
            .get(agent.id())
            .is_none()
    );
    assert!(sessions.get(agent.session().id()).is_none());
    let _ = json!(null);
    stack.close().await;
}
