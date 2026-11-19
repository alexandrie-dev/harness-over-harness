//! Port of scope-lifecycle.spec.ts: creation ordering, rollback, ownership and quiescence of the
//! agent's scope. Cases that reach into cordis fiber internals (`_disposables`, effect labels,
//! monkeypatched registries) have no counterpart here (see PORT_NOTES.md).
mod common;

use common::*;
use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentCreated, AgentDisposed, AgentHandle, AgentSetup, AgentSetupCommit, AgentStatus,
    AgentStatusChanged, Agents, CreateAgentOptions,
};
use hoh_error::BoxError;
use hoh_kernel::{Context, Fiber, Undo};
use hoh_scope::scope_of;
use hoh_session::{SessionCreated, SessionDisposed, SessionId, Sessions};
use hoh_timeout::AbortController;
use std::future::Future;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Notify;

type Order = Arc<Mutex<Vec<String>>>;

fn order() -> Order {
    Arc::new(Mutex::new(Vec::new()))
}

fn push(order: &Order, entry: &str) {
    lock(order).push(entry.to_owned());
}

fn setup_of<F, Fut>(f: F) -> AgentSetup
where
    F: Fn(Context, Arc<dyn Agent>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Option<AgentSetupCommit>, BoxError>> + Send + 'static,
{
    Arc::new(move |ctx, agent| Box::pin(f(ctx, agent)))
}

fn options(id: &str) -> CreateAgentOptions {
    let mut options = CreateAgentOptions::new(SessionId::from(id));
    options.agent_options = Some(mock_options());
    options
}

fn create(
    ctx: &Context,
    owner: &Context,
    options: CreateAgentOptions,
) -> BoxFuture<'static, Result<AgentHandle, BoxError>> {
    let _ = ctx;
    owner
        .get::<Agents>()
        .expect("agents service")
        .create(owner, options)
}

fn agents(env: &Env) -> hoh_kernel::Service<Agents> {
    env.ctx.get::<Agents>().expect("agents service")
}

fn sessions(env: &Env) -> hoh_kernel::Service<Sessions> {
    env.ctx.get::<Sessions>().expect("sessions service")
}

/// An owner fiber with its own context (the `ctx.plugin(inject: ['agents'])` of the source).
async fn owner(env: &Env) -> (Fiber, Context) {
    let slot = Arc::new(Mutex::new(None));
    let capture = slot.clone();
    let fiber = env
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

async fn dispose_fiber(fiber: &Fiber) {
    if let Some(disposer) = fiber.disposer() {
        disposer.dispose().await;
    }
}

fn scoped_marker(order: &Order, label: &'static str) -> impl Fn(Context) + Send + Sync + 'static {
    let order = order.clone();
    move |ctx: Context| {
        let order = order.clone();
        ctx.effect(format!("test.scope({label})"), move || {
            Undo::sync(move || push(&order, label))
        })
        .expect("scoped effect");
    }
}

// ---------------------------------------------------------------- creation

#[tokio::test]
async fn awaits_serial_creation_before_caller_readiness() {
    let env = harness(MockAdapter::chunks(vec![text_response("ok")])).await;
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let log = order();
    {
        let (entered, release, log) = (entered.clone(), release.clone(), log.clone());
        env.ctx
            .on_serial::<AgentCreated, (), _>(move |_| {
                let (entered, release, log) = (entered.clone(), release.clone(), log.clone());
                async move {
                    push(&log, "created:start");
                    entered.notify_one();
                    release.notified().await;
                    push(&log, "created:end");
                    Ok(None)
                }
            })
            .expect("listener");
    }
    {
        let log = log.clone();
        env.ctx
            .on_serial::<AgentCreated, (), _>(move |_| {
                let log = log.clone();
                async move {
                    push(&log, "created:second");
                    Ok(None)
                }
            })
            .expect("listener");
    }
    let creating = {
        let (future, log) = (
            create(&env.ctx, &env.ctx, options("serial-start")),
            log.clone(),
        );
        tokio::spawn(async move {
            let handle = future.await;
            push(&log, "ready");
            handle
        })
    };

    entered.notified().await;
    assert_eq!(*lock(&log), ["created:start"]);
    release.notify_one();
    let handle = creating.await.expect("join").expect("created");
    assert_eq!(
        *lock(&log),
        ["created:start", "created:end", "created:second", "ready"]
    );
    handle.dispose().await.expect("dispose");
}

#[tokio::test]
async fn rolls_back_both_registries_and_the_scope_when_asynchronous_creation_fails() {
    let adapter = MockAdapter::chunks(vec![text_response("unexpected")]);
    let env = harness(adapter.clone()).await;
    let log = order();
    env.ctx
        .on_serial::<AgentCreated, (), _>(move |event| {
            let agent = event.agent.clone();
            async move {
                let _ = agent.followup(user_message("queued during creation"));
                tokio::task::yield_now().await;
                Err("async creation veto".into())
            }
        })
        .expect("veto listener");
    {
        let log = log.clone();
        env.ctx
            .on_serial::<AgentCreated, (), _>(move |_| {
                let log = log.clone();
                async move {
                    push(&log, "later-listener");
                    Ok(None)
                }
            })
            .expect("listener");
    }
    {
        let log = log.clone();
        env.ctx
            .on_contained::<AgentDisposed>(move |_| {
                push(&log, "agent-disposed");
                Ok(())
            })
            .expect("listener");
    }
    {
        let log = log.clone();
        env.ctx
            .on_contained::<SessionDisposed>(move |_| {
                push(&log, "session-disposed");
                Ok(())
            })
            .expect("listener");
    }
    let mut create_options = options("async-veto");
    let marker = scoped_marker(&log, "scope-disposed");
    create_options.setup = Some(setup_of(move |ctx, _| {
        marker(ctx);
        async { Ok(None) }
    }));

    let error = create(&env.ctx, &env.ctx, create_options)
        .await
        .err()
        .expect("creation is vetoed");
    assert_eq!(error.to_string(), "async creation veto");
    assert_eq!(
        *lock(&log),
        ["scope-disposed", "agent-disposed", "session-disposed"]
    );
    assert!(agents(&env).list().is_empty());
    assert!(sessions(&env).list().is_empty());
    assert_eq!(adapter.request_count(), 0);
}

#[tokio::test]
async fn rejects_an_already_aborted_creation_signal_before_publishing_either_object() {
    let env = harness(MockAdapter::chunks(vec![])).await;
    let controller = AbortController::new();
    controller.abort_with("cancelled before creation".to_owned());
    let mut create_options = options("pre-aborted-s");
    create_options.signal = Some(controller.signal());

    let outcome = create(&env.ctx, &env.ctx, create_options).await;

    assert!(outcome.is_err(), "an aborted signal must reject creation");
    assert!(
        agents(&env)
            .get(&SessionId::from("pre-aborted-s"))
            .is_none()
    );
    assert!(
        sessions(&env)
            .get(&SessionId::from("pre-aborted-s"))
            .is_none()
    );
}

#[tokio::test]
async fn tags_agent_ctx_with_the_agent_scope_key() {
    let env = harness(MockAdapter::chunks(vec![])).await;
    let agent = create_agent(&env, "a1").await;
    assert_eq!(scope_of(agent.ctx()), Some(agent.scope_key()));
    agent.when_idle().await;
}

#[tokio::test]
async fn agent_ctx_listeners_hear_only_their_own_agent() {
    let env = harness(MockAdapter::chunks(vec![
        text_response("one"),
        text_response("two"),
    ]))
    .await;
    let a = create_agent(&env, "a").await;
    let b = create_agent(&env, "b").await;
    let heard = order();
    {
        let heard = heard.clone();
        a.ctx()
            .on_contained::<AgentStatusChanged>(move |event| {
                push(
                    &heard,
                    &format!("a-sees:{}:{:?}", event.agent.id(), event.status),
                );
                Ok(())
            })
            .expect("scoped listener");
    }

    send_and_wait(&b, "for b").await;
    assert!(
        lock(&heard).is_empty(),
        "nothing of b's leaked into a's scope"
    );

    send_and_wait(&a, "for a").await;
    assert!(lock(&heard).contains(&"a-sees:a:Running".to_owned()));
}

#[tokio::test]
async fn runs_setup_before_agent_created() {
    let env = harness(MockAdapter::chunks(vec![])).await;
    let log = order();
    {
        let log = log.clone();
        env.ctx
            .on_serial::<AgentCreated, (), _>(move |_| {
                let log = log.clone();
                async move {
                    push(&log, "agent/created");
                    Ok(None)
                }
            })
            .expect("listener");
    }
    let mut create_options = options("child-s");
    let setup_log = log.clone();
    create_options.setup = Some(setup_of(move |_, _| {
        let setup_log = setup_log.clone();
        async move {
            push(&setup_log, "setup");
            tokio::task::yield_now().await;
            Ok(None)
        }
    }));

    let handle = create(&env.ctx, &env.ctx, create_options)
        .await
        .expect("created");
    assert_eq!(*lock(&log), ["setup", "agent/created"]);
    handle.dispose().await.expect("dispose");
}

#[tokio::test]
async fn announces_the_session_before_setup_and_the_agent_after_setup_completes() {
    let env = harness(MockAdapter::chunks(vec![])).await;
    let gate = Arc::new(Notify::new());
    let started = Arc::new(Notify::new());
    let log = order();
    {
        let (log, probe) = (log.clone(), agents(&env));
        env.ctx
            .on_contained::<SessionCreated>(move |event| {
                assert!(probe.get(event.session.id()).is_none());
                push(&log, "session/created");
                Ok(())
            })
            .expect("listener");
    }
    {
        let log = log.clone();
        env.ctx
            .on_serial::<AgentCreated, (), _>(move |_| {
                let log = log.clone();
                async move {
                    push(&log, "agent/created");
                    Ok(None)
                }
            })
            .expect("listener");
    }
    let mut create_options = options("atomic");
    {
        let (log, gate, started) = (log.clone(), gate.clone(), started.clone());
        let (registry, store) = (agents(&env), sessions(&env));
        create_options.setup = Some(setup_of(move |_, agent| {
            let (log, gate, started) = (log.clone(), gate.clone(), started.clone());
            let (registry, store) = (registry.clone(), store.clone());
            async move {
                assert_eq!(agent.id(), &SessionId::from("atomic"));
                push(&log, "setup:start");
                started.notify_one();
                gate.notified().await;
                push(&log, "setup:end");
                let commit: AgentSetupCommit = Box::new(move || {
                    assert!(registry.get(&SessionId::from("atomic")).is_none());
                    assert!(store.get(&SessionId::from("atomic")).is_some());
                    push(&log, "setup:commit");
                    Ok(())
                });
                Ok(Some(commit))
            }
        }));
    }
    let creating = tokio::spawn(create(&env.ctx, &env.ctx, create_options));

    started.notified().await;
    assert!(agents(&env).get(&SessionId::from("atomic")).is_none());
    assert!(sessions(&env).get(&SessionId::from("atomic")).is_some());
    assert_eq!(*lock(&log), ["session/created", "setup:start"]);
    gate.notify_one();
    let handle = creating.await.expect("join").expect("created");
    assert_eq!(
        *lock(&log),
        [
            "session/created",
            "setup:start",
            "setup:end",
            "setup:commit",
            "agent/created"
        ]
    );
    handle.dispose().await.expect("dispose");
}

#[tokio::test]
async fn rejects_a_concurrent_same_id_creation_before_its_setup_starts() {
    let env = harness(MockAdapter::chunks(vec![])).await;
    let gate = Arc::new(Notify::new());
    let started = Arc::new(Notify::new());
    let id = SessionId::from("concurrent-enter");
    let mut first_options = options("concurrent-enter");
    {
        let (gate, started) = (gate.clone(), started.clone());
        first_options.setup = Some(setup_of(move |_, _| {
            let (gate, started) = (gate.clone(), started.clone());
            async move {
                started.notify_one();
                gate.notified().await;
                Ok(None)
            }
        }));
    }
    let first = tokio::spawn(create(&env.ctx, &env.ctx, first_options));
    started.notified().await;
    let session = sessions(&env).get(&id).expect("session announced");

    let second_ran = Arc::new(AtomicBool::new(false));
    let mut second_options = options("concurrent-enter");
    {
        let ran = second_ran.clone();
        second_options.setup = Some(setup_of(move |_, _| {
            ran.store(true, Ordering::SeqCst);
            async { Ok(None) }
        }));
    }
    let error = create(&env.ctx, &env.ctx, second_options)
        .await
        .err()
        .expect("duplicate id is refused");
    assert!(error.to_string().contains("already exists"), "{error}");
    assert!(!second_ran.load(Ordering::SeqCst));
    assert!(agents(&env).list().is_empty());
    assert_eq!(sessions(&env).list(), std::slice::from_ref(&session));

    gate.notify_one();
    let handle = first.await.expect("join").expect("created");
    assert!(handle.agent.session() == &session);
    handle.dispose().await.expect("dispose");
    assert!(agents(&env).list().is_empty());
    assert!(sessions(&env).list().is_empty());
}

#[tokio::test]
async fn the_signal_governs_only_creation_not_a_returned_live_handle() {
    let adapter = MockAdapter::chunks(vec![text_response("ok")]);
    let env = harness(adapter.clone()).await;
    let pending = AbortController::new();
    let started = Arc::new(Notify::new());
    let mut pending_options = options("signal-pending-s");
    pending_options.signal = Some(pending.signal());
    {
        let started = started.clone();
        pending_options.setup = Some(setup_of(move |_, _| {
            let started = started.clone();
            async move {
                started.notify_one();
                std::future::pending::<()>().await;
                Ok(None)
            }
        }));
    }
    let creating = tokio::spawn(create(&env.ctx, &env.ctx, pending_options));
    started.notified().await;
    pending.abort_with("cancel pending creation".to_owned());
    let outcome = creating.await.expect("join");
    assert!(outcome.is_err(), "the aborted creation must reject");
    assert!(
        agents(&env)
            .get(&SessionId::from("signal-pending-s"))
            .is_none()
    );
    assert!(
        sessions(&env)
            .get(&SessionId::from("signal-pending-s"))
            .is_none()
    );

    let live_signal = AbortController::new();
    let mut live_options = options("signal-live-s");
    live_options.signal = Some(live_signal.signal());
    let live = create(&env.ctx, &env.ctx, live_options)
        .await
        .expect("live handle");
    live_signal.abort_with("too late".to_owned());
    tokio::task::yield_now().await;
    assert!(agents(&env).get(live.agent.id()).is_some());
    assert_eq!(live.agent.status(), AgentStatus::Idle);
    send_and_wait(&live.agent, "continue").await;
    assert_eq!(adapter.request_count(), 1);
    live.dispose().await.expect("dispose");
}

#[tokio::test]
async fn a_rejecting_setup_disposes_the_announced_session_and_frees_the_id() {
    let env = harness(MockAdapter::chunks(vec![])).await;
    let published = order();
    {
        let log = published.clone();
        env.ctx
            .on_contained::<SessionCreated>(move |_| {
                push(&log, "session/created");
                Ok(())
            })
            .expect("listener");
    }
    {
        let log = published.clone();
        env.ctx
            .on_contained::<SessionDisposed>(move |_| {
                push(&log, "session/disposed");
                Ok(())
            })
            .expect("listener");
    }
    {
        let log = published.clone();
        env.ctx
            .on_serial::<AgentCreated, (), _>(move |_| {
                let log = log.clone();
                async move {
                    push(&log, "agent/created");
                    Ok(None)
                }
            })
            .expect("listener");
    }
    let mut failing = options("bad-s");
    failing.setup = Some(setup_of(|_, _| async {
        tokio::task::yield_now().await;
        Err("boom setup".into())
    }));

    let error = create(&env.ctx, &env.ctx, failing)
        .await
        .err()
        .expect("setup failure surfaces");
    assert_eq!(error.to_string(), "boom setup");

    // Nothing leaked: no agent, no session, and the id is reusable.
    assert_eq!(*lock(&published), ["session/created", "session/disposed"]);
    assert!(agents(&env).get(&SessionId::from("bad-s")).is_none());
    assert!(sessions(&env).get(&SessionId::from("bad-s")).is_none());
    let retry = create(&env.ctx, &env.ctx, options("bad-s"))
        .await
        .expect("retry");
    retry.dispose().await.expect("dispose");
}

// ---------------------------------------------------------------- ownership and quiescence

#[tokio::test]
async fn owner_unload_aborts_pending_creation_and_leaves_no_registry_entries() {
    let env = harness(MockAdapter::chunks(vec![])).await;
    let (owner_fiber, owner_ctx) = owner(&env).await;
    let started = Arc::new(Notify::new());
    let mut pending_options = options("owner-unload-setup");
    {
        let started = started.clone();
        pending_options.setup = Some(setup_of(move |_, _| {
            let started = started.clone();
            async move {
                started.notify_one();
                std::future::pending::<()>().await;
                Ok(None)
            }
        }));
    }
    let creating = tokio::spawn(create(&env.ctx, &owner_ctx, pending_options));
    started.notified().await;

    dispose_fiber(&owner_fiber).await;
    let outcome = creating.await.expect("join");

    assert!(outcome.is_err(), "owner unload must reject the creation");
    assert!(
        agents(&env)
            .get(&SessionId::from("owner-unload-setup"))
            .is_none()
    );
    assert!(
        sessions(&env)
            .get(&SessionId::from("owner-unload-setup"))
            .is_none()
    );
}

#[tokio::test]
async fn owner_unload_disposes_a_live_agent_and_the_handle_shares_the_boundary() {
    let env = harness(MockAdapter::chunks(vec![])).await;
    let (owner_fiber, owner_ctx) = owner(&env).await;
    let handle = create(&env.ctx, &owner_ctx, options("h1-s"))
        .await
        .expect("created");
    let torn_down = order();
    {
        let log = torn_down.clone();
        env.ctx
            .on_contained::<AgentDisposed>(move |_| {
                push(&log, "unregistered");
                Ok(())
            })
            .expect("listener");
    }

    // Owner unload begins first; a concurrent handle.dispose() must not resolve before the
    // chain actually finished.
    let unload = tokio::spawn(async move { dispose_fiber(&owner_fiber).await });
    handle.dispose().await.expect("dispose");
    assert!(lock(&torn_down).contains(&"unregistered".to_owned()));
    assert!(agents(&env).get(&SessionId::from("h1-s")).is_none());
    assert!(sessions(&env).get(&SessionId::from("h1-s")).is_none());
    unload.await.expect("owner unload");
}

#[tokio::test]
async fn reopens_ids_after_the_prior_private_scope_finishes_quiescing() {
    let env = harness(MockAdapter::chunks(vec![])).await;
    let gate = Arc::new(Notify::new());
    let cleanup_started = Arc::new(Notify::new());
    let id = SessionId::from("quiescent-reuse");
    let mut first_options = options("quiescent-reuse");
    {
        let (gate, cleanup_started) = (gate.clone(), cleanup_started.clone());
        first_options.setup = Some(setup_of(move |ctx, _| {
            let (gate, cleanup_started) = (gate.clone(), cleanup_started.clone());
            async move {
                ctx.effect("test.slow-cleanup", move || {
                    Undo::future(move || async move {
                        cleanup_started.notify_one();
                        gate.notified().await;
                        Ok(())
                    })
                })?;
                Ok(None)
            }
        }));
    }
    let first = create(&env.ctx, &env.ctx, first_options)
        .await
        .expect("created");

    let disposing = tokio::spawn(first.dispose());
    cleanup_started.notified().await;
    assert!(agents(&env).get(&id).is_some());
    assert!(sessions(&env).get(&id).is_some());
    gate.notify_one();
    disposing.await.expect("join").expect("dispose");
    assert!(agents(&env).get(&id).is_none());
    assert!(sessions(&env).get(&id).is_none());

    let replacement = create(&env.ctx, &env.ctx, options("quiescent-reuse"))
        .await
        .expect("replacement");
    assert!(agents(&env).get(&id).is_some());
    replacement.dispose().await.expect("dispose");
}

#[tokio::test]
async fn agent_loop_unload_is_a_structural_co_owner_of_every_live_agent() {
    let env = harness(MockAdapter::chunks(vec![])).await;
    let handle = create(&env.ctx, &env.ctx, options("factory-live-s"))
        .await
        .expect("created");

    // Unloading the loop disposes its agents.
    dispose_fiber(&env.loop_fiber).await;

    assert!(
        agents(&env)
            .get(&SessionId::from("factory-live-s"))
            .is_none()
    );
    assert!(
        sessions(&env)
            .get(&SessionId::from("factory-live-s"))
            .is_none()
    );
    assert_eq!(handle.agent.status(), AgentStatus::Idle);
    // The consumer handle shares the provider's completed quiescence boundary.
    handle.dispose().await.expect("dispose");
    // A creation against the inactive loop is refused.
    let refused = create(&env.ctx, &env.ctx, options("factory-inactive-s"))
        .await
        .err()
        .expect("inactive loop refuses creation");
    assert!(!refused.to_string().is_empty());
}

#[tokio::test]
async fn a_handle_disposed_while_idle_leaves_the_registries_empty() {
    let env = harness(MockAdapter::chunks(vec![
        text_response("one"),
        text_response("never awaited"),
    ]))
    .await;
    let handle = create(&env.ctx, &env.ctx, options("drain-reentered-run"))
        .await
        .expect("created");
    let agent = handle.agent.clone();
    let reentered = Arc::new(AtomicBool::new(false));
    {
        let (subject, flag) = (agent.clone(), reentered.clone());
        env.ctx
            .on_contained::<AgentStatusChanged>(move |event| {
                if event.agent.id() == subject.id()
                    && event.status == AgentStatus::Idle
                    && !flag.swap(true, Ordering::SeqCst)
                {
                    let _ = subject.followup(user_message("reentrant"));
                }
                Ok(())
            })
            .expect("listener");
    }

    send_and_wait(&agent, "go").await;
    agent.when_idle().await;
    assert!(reentered.load(Ordering::SeqCst));

    handle.dispose().await.expect("dispose");

    // The reentrant run either never started or was drained: the registries are empty and
    // nothing still drives the detached session.
    assert!(agents(&env).get(agent.id()).is_none());
    assert!(sessions(&env).get(agent.id()).is_none());
    let after = agent.session().snapshot_events(None, None).len();
    settle(30).await;
    assert_eq!(agent.session().snapshot_events(None, None).len(), after);
}
