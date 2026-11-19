mod common;

use common::{mount, stub_agent, stub_agent_with_session};
use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentCreated, AgentDisposed, AgentFactory, AgentHandle, AgentRegistryPlugin,
    AgentStatus, AgentStatusChanged, AgentTurnStopping, Agents, CreateAgentOptions,
    ResumeAgentOptions, SessionStartSource, StepBoundary, StepBoundaryKind, agent_events,
};
use hoh_error::BoxError;
use hoh_kernel::{Context, Kernel, Level, Levels};
use hoh_session::{Session, SessionId};
use hoh_session_projection::SessionProjectionRegistry;
use hoh_timeout::AbortController;
use serde_json::json;
use std::sync::{Arc, Mutex};

type Log = Arc<Mutex<Vec<String>>>;
type Heard = Arc<Mutex<Vec<(Arc<dyn Agent>, u64)>>>;

fn log() -> Log {
    Arc::default()
}
fn push(log: &Log, text: impl Into<String>) {
    log.lock().unwrap().push(text.into());
}
fn read(log: &Log) -> Vec<String> {
    log.lock().unwrap().clone()
}

fn warnings(ctx: &Context) -> Log {
    let out = log();
    let sink = out.clone();
    ctx.logger()
        .exporter(
            Levels {
                default: Some(Level::Warn),
                by_name: Default::default(),
            },
            move |record| {
                if record.level == Level::Warn {
                    push(&sink, record.message.clone());
                }
            },
        )
        .unwrap();
    out
}

#[tokio::test]
async fn waits_for_its_projection_owner_and_reads_turn_positions_without_a_loop() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fiber = ctx.plugin::<AgentRegistryPlugin>(json!({})).unwrap();
    assert!(ctx.get::<Agents>().is_none());
    ctx.plugin::<SessionProjectionRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    fiber.settled().await.unwrap();
    let agents = ctx.get::<Agents>().unwrap();
    let session =
        Session::create(SessionId::from("turn-positions"), None, None, None, None).unwrap();
    let initial = agents.turn_boundary_of(&session).unwrap();
    assert_eq!(
        serde_json::to_value(&*initial).unwrap(),
        json!({
            "openTurnStartSeq": null, "lastStepStartSeq": null,
            "lastStepBoundary": null, "lastTurn": 0
        })
    );
    let turn = session
        .append("turn/start", json!({ "turn": 3 }), None)
        .unwrap();
    let step = session
        .append("step/start", json!({ "turn": 3, "step": 1 }), None)
        .unwrap();
    let state = agents.turn_boundary_of(&session).unwrap();
    assert_eq!(state.open_turn_start_seq, Some(turn.seq()));
    assert_eq!(state.last_step_start_seq, Some(step.seq()));
    assert_eq!(
        state.last_step_boundary,
        Some(StepBoundary {
            kind: StepBoundaryKind::Start,
            seq: step.seq()
        })
    );
    assert_eq!(state.last_turn, 3);
    session
        .append(
            "turn/end",
            json!({ "turn": 3, "reason": { "kind": "completed" } }),
            None,
        )
        .unwrap();
    let state = agents.turn_boundary_of(&session).unwrap();
    assert_eq!((state.open_turn_start_seq, state.last_turn), (None, 3));
    kernel.shutdown().await;
}

#[tokio::test]
async fn registers_exact_entries_emits_lifecycle_events_and_unregisters_on_owner_disposal() {
    let h = mount().await;
    let agents = h.ctx.get::<Agents>().unwrap();
    let lifecycle = log();
    let l = lifecycle.clone();
    h.ctx
        .on_serial::<AgentCreated, (), _>(move |e| {
            push(&l, format!("created:{}", e.agent.id()));
            async { Ok(None) }
        })
        .unwrap();
    let l = lifecycle.clone();
    h.ctx
        .on_contained::<AgentDisposed>(move |e| {
            push(&l, format!("disposed:{}", e.agent.id()));
            Ok(())
        })
        .unwrap();

    let agent = stub_agent(&h.ctx, "a1");
    let dispose = agents.register(&h.ctx, agent.clone()).await.unwrap();
    assert!(Arc::ptr_eq(&agents.get(agent.id()).unwrap(), &agent));
    assert_eq!(agents.list().len(), 1);
    let error = agents
        .register(&h.ctx, stub_agent(&h.ctx, "a1"))
        .await
        .err()
        .expect("duplicate id must be rejected");
    assert!(error.to_string().contains("already registered"));

    dispose.dispose().await;
    assert!(agents.get(agent.id()).is_none());
    assert_eq!(read(&lifecycle), ["created:a1", "disposed:a1"]);
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_an_agent_whose_registry_and_session_identities_differ() {
    let h = mount().await;
    let agents = h.ctx.get::<Agents>().unwrap();
    let agent = stub_agent_with_session(&h.ctx, "agent-id", "session-id");
    let error = agents
        .enter(&h.ctx, agent)
        .err()
        .expect("identity mismatch");
    assert_eq!(
        error.to_string(),
        "agent id \"agent-id\" does not match session id \"session-id\""
    );
    assert!(agents.list().is_empty());
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn rolls_an_entry_back_and_pairs_a_partially_delivered_creation_when_a_listener_throws() {
    let h = mount().await;
    let agents = h.ctx.get::<Agents>().unwrap();
    let lifecycle = log();
    let l = lifecycle.clone();
    h.ctx
        .on_serial::<AgentCreated, (), _>(move |e| {
            push(&l, format!("created:{}", e.agent.id()));
            async { Ok(None) }
        })
        .unwrap();
    h.ctx
        .on_serial::<AgentCreated, (), _>(|_| async {
            Err::<Option<()>, BoxError>("creation veto".into())
        })
        .unwrap();
    let l = lifecycle.clone();
    h.ctx
        .on_contained::<AgentDisposed>(move |e| {
            push(&l, format!("disposed:{}", e.agent.id()));
            Ok(())
        })
        .unwrap();

    let error = agents
        .register(&h.ctx, stub_agent(&h.ctx, "vetoed"))
        .await
        .err()
        .expect("veto");
    assert_eq!(error.to_string(), "creation veto");
    assert!(agents.get(&SessionId::from("vetoed")).is_none());
    assert_eq!(read(&lifecycle), ["created:vetoed", "disposed:vetoed"]);
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn rolls_back_creation_rejection_and_contains_every_disposal_listener_failure() {
    let h = mount().await;
    let agents = h.ctx.get::<Agents>().unwrap();
    let warned = warnings(&h.ctx);
    let heard = log();
    h.ctx
        .on_serial::<AgentCreated, (), _>(|_| async {
            Err::<Option<()>, BoxError>("created async".into())
        })
        .unwrap();
    let l = heard.clone();
    h.ctx
        .on_serial::<AgentCreated, (), _>(move |_| {
            push(&l, "unreachable");
            async { Ok(None) }
        })
        .unwrap();
    h.ctx
        .on_contained::<AgentDisposed>(|_| Err("disposed sync".into()))
        .unwrap();
    h.ctx
        .on_contained_async::<AgentDisposed, _>(|_| async {
            Err::<(), BoxError>("disposed async".into())
        })
        .unwrap();
    let l = heard.clone();
    h.ctx
        .on_contained::<AgentDisposed>(move |e| {
            push(&l, e.agent.id().to_string());
            Ok(())
        })
        .unwrap();

    let error = agents
        .register(&h.ctx, stub_agent(&h.ctx, "contained"))
        .await
        .err()
        .expect("rejection");
    assert_eq!(error.to_string(), "created async");
    tokio::task::yield_now().await;

    assert_eq!(read(&heard), ["contained"]);
    assert!(agents.list().is_empty());
    assert_eq!(
        read(&warned),
        [
            "agent \"contained\": agent/disposed listener threw: Error: disposed sync",
            "agent \"contained\": agent/disposed listener rejected: Error: disposed async",
        ]
    );
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn awaits_each_creation_listener_before_the_next_listener_and_registration_completion() {
    let h = mount().await;
    let agents = h.ctx.get::<Agents>().unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let order = log();
    let (o, en, re) = (order.clone(), entered.clone(), release.clone());
    h.ctx
        .on_serial::<AgentCreated, (), _>(move |_| {
            let (o, en, re) = (o.clone(), en.clone(), re.clone());
            async move {
                push(&o, "first:start");
                en.notify_one();
                re.notified().await;
                push(&o, "first:end");
                Ok(None)
            }
        })
        .unwrap();
    let o = order.clone();
    h.ctx
        .on_serial::<AgentCreated, (), _>(move |_| {
            push(&o, "second");
            async { Ok(None) }
        })
        .unwrap();

    let agent = stub_agent(&h.ctx, "serial");
    let registration = {
        let (agents, ctx, agent) = (agents.clone(), h.ctx.clone(), agent.clone());
        let order = order.clone();
        tokio::spawn(async move {
            let disposer = agents.register(&ctx, agent).await.unwrap();
            push(&order, "ready");
            disposer
        })
    };
    entered.notified().await;
    assert_eq!(read(&order), ["first:start"]);
    let error = agents
        .announce(&h.ctx, &agent, SessionStartSource::Startup, None)
        .await
        .expect_err("already announced");
    assert!(error.to_string().contains("already announced"));
    release.notify_one();
    let disposer = registration.await.unwrap();
    assert_eq!(
        read(&order),
        ["first:start", "first:end", "second", "ready"]
    );
    disposer.dispose().await;
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn separates_entry_from_announcement_and_stale_or_idempotent_detach_cannot_remove_a_replacement()
 {
    let h = mount().await;
    let agents = h.ctx.get::<Agents>().unwrap();
    let lifecycle = log();
    let l = lifecycle.clone();
    h.ctx
        .on_serial::<AgentCreated, (), _>(move |e| {
            push(&l, format!("created:{}", e.agent.id()));
            async { Ok(None) }
        })
        .unwrap();
    let l = lifecycle.clone();
    h.ctx
        .on_contained::<AgentDisposed>(move |e| {
            push(&l, format!("disposed:{}", e.agent.id()));
            Ok(())
        })
        .unwrap();

    let first = stub_agent(&h.ctx, "split");
    let detach_first = agents.enter(&h.ctx, first.clone()).unwrap();
    assert!(read(&lifecycle).is_empty());
    agents
        .announce(&h.ctx, &first, SessionStartSource::Startup, None)
        .await
        .unwrap();
    let again = agents
        .announce(&h.ctx, &first, SessionStartSource::Startup, None)
        .await
        .expect_err("second announcement");
    assert!(again.to_string().contains("already announced"));
    detach_first.detach();
    detach_first.detach();

    let replacement = stub_agent(&h.ctx, "split");
    let detach_replacement = agents.enter(&h.ctx, replacement.clone()).unwrap();
    detach_first.detach();
    assert!(Arc::ptr_eq(
        &agents.get(replacement.id()).unwrap(),
        &replacement
    ));
    let stale = agents
        .announce(&h.ctx, &first, SessionStartSource::Startup, None)
        .await
        .expect_err("stale announce");
    assert!(stale.to_string().contains("not live"));
    detach_replacement.detach();
    assert_eq!(read(&lifecycle), ["created:split", "disposed:split"]);
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn defers_detach_requested_by_a_creation_listener_until_that_dispatch_unwinds() {
    let h = mount().await;
    let agents = h.ctx.get::<Agents>().unwrap();
    let order = log();
    let agent = stub_agent(&h.ctx, "reentrant");
    let detach = agents.enter(&h.ctx, agent.clone()).unwrap();
    let (o, a, ag, d) = (order.clone(), agents.clone(), agent.clone(), detach.clone());
    h.ctx
        .on_serial::<AgentCreated, (), _>(move |_| {
            push(&o, format!("first:{}", a.get(ag.id()).is_some()));
            d.detach();
            push(&o, format!("after-detach:{}", a.get(ag.id()).is_some()));
            async { Ok(None) }
        })
        .unwrap();
    let (o, a, ag) = (order.clone(), agents.clone(), agent.clone());
    h.ctx
        .on_serial::<AgentCreated, (), _>(move |_| {
            push(&o, format!("second:{}", a.get(ag.id()).is_some()));
            async { Ok(None) }
        })
        .unwrap();
    let o = order.clone();
    h.ctx
        .on_contained::<AgentDisposed>(move |_| {
            push(&o, "disposed");
            Ok(())
        })
        .unwrap();
    agents
        .announce(&h.ctx, &agent, SessionStartSource::Startup, None)
        .await
        .unwrap();
    assert_eq!(
        read(&order),
        ["first:true", "after-detach:true", "second:true", "disposed"]
    );
    assert!(agents.get(agent.id()).is_none());
    h.kernel.shutdown().await;
}

// ---- agent_events() ------------------------------------------------------------------------

#[tokio::test]
async fn agent_events_contains_each_failure_of_a_notification_listener() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let warned = warnings(&ctx);
    let heard = log();
    let agent = stub_agent(&ctx, "event");
    ctx.on_contained::<AgentStatusChanged>(|_| Err("sync listener".into()))
        .unwrap();
    ctx.on_contained_async::<AgentStatusChanged, _>(|_| async {
        Err::<(), BoxError>("async listener".into())
    })
    .unwrap();
    let l = heard.clone();
    ctx.on_contained::<AgentStatusChanged>(move |e| {
        push(&l, e.status.as_str());
        Ok(())
    })
    .unwrap();

    agent_events(&ctx, &agent).emit(|agent| AgentStatusChanged {
        agent,
        status: AgentStatus::Running,
    });
    tokio::task::yield_now().await;
    assert_eq!(read(&heard), ["running"]);
    assert_eq!(
        read(&warned),
        [
            "agent event \"agent/status\" listener threw: Error: sync listener",
            "agent event \"agent/status\" listener rejected: Error: async listener",
        ]
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn agent_events_dispatches_serial_listeners_with_the_fused_agent_subject() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let agent = stub_agent(&ctx, "serial-event");
    let signal = AbortController::new().signal();
    let heard: Heard = Arc::default();
    let h = heard.clone();
    ctx.on_serial::<AgentTurnStopping, (), _>(move |e| {
        let (h, subject, turn) = (h.clone(), e.agent.clone(), e.turn);
        async move {
            tokio::task::yield_now().await;
            h.lock().unwrap().push((subject, turn));
            Ok(None)
        }
    })
    .unwrap();

    agent_events(&ctx, &agent)
        .serial::<AgentTurnStopping, ()>(|agent| AgentTurnStopping {
            agent,
            turn: 3,
            signal,
        })
        .await
        .unwrap();
    {
        let heard = heard.lock().unwrap();
        assert_eq!(heard.len(), 1);
        assert!(Arc::ptr_eq(&heard[0].0, &agent));
        assert_eq!(heard[0].1, 3);
    }
    kernel.shutdown().await;
}

#[tokio::test]
async fn agent_events_injects_the_fused_subject_even_when_the_payload_carries_a_conflicting_agent_field()
 {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let agent = stub_agent(&ctx, "fused-subject");
    let other = stub_agent(&ctx, "payload-agent");
    let heard: Heard = Arc::default();
    let h = heard.clone();
    ctx.on_contained::<AgentStatusChanged>(move |e| {
        h.lock().unwrap().push((e.agent.clone(), 0));
        Ok(())
    })
    .unwrap();

    // The builder ignores the subject it is given and puts a foreign agent in the payload.
    let foreign = other.clone();
    agent_events(&ctx, &agent).emit(move |_| AgentStatusChanged {
        agent: foreign,
        status: AgentStatus::Running,
    });
    {
        let heard = heard.lock().unwrap();
        assert_eq!(heard.len(), 1);
        assert!(Arc::ptr_eq(&heard[0].0, &agent));
        assert!(!Arc::ptr_eq(&heard[0].0, &other));
    }
    kernel.shutdown().await;
}

#[tokio::test]
async fn agent_events_injects_the_fused_subject_into_serial_and_waterfall_payloads_too() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let agent = stub_agent(&ctx, "fused-serial");
    let other = stub_agent(&ctx, "payload-agent");
    let signal = AbortController::new().signal();
    let heard: Heard = Arc::default();
    let h = heard.clone();
    ctx.on_serial::<AgentTurnStopping, (), _>(move |e| {
        h.lock().unwrap().push((e.agent.clone(), e.turn));
        async { Ok(None) }
    })
    .unwrap();
    // Scope routing is by the dispatcher's carrier: a listener of the foreign agent's scope
    // never hears an event dispatched for this agent.
    agent_events(&ctx, &agent)
        .serial::<AgentTurnStopping, ()>(|_| AgentTurnStopping {
            agent: other.clone(),
            turn: 1,
            signal,
        })
        .await
        .unwrap();
    {
        let heard = heard.lock().unwrap();
        assert_eq!(heard.len(), 1);
        assert!(Arc::ptr_eq(&heard[0].0, &agent));
    }
    kernel.shutdown().await;
}

#[tokio::test]
async fn agent_events_names_a_named_error_class_like_string_of_the_source() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let warned = warnings(&ctx);
    let agent = stub_agent(&ctx, "named");
    ctx.on_contained::<AgentStatusChanged>(|_| {
        Err(hoh_error::HarnessError::named("StorageError", "disk full", "ENOSPC").into())
    })
    .unwrap();
    agent_events(&ctx, &agent).emit(|agent| AgentStatusChanged {
        agent,
        status: AgentStatus::Idle,
    });
    tokio::task::yield_now().await;
    assert_eq!(
        read(&warned),
        ["agent event \"agent/status\" listener threw: StorageError: disk full"]
    );
    kernel.shutdown().await;
}

// ---- factory seam --------------------------------------------------------------------------

struct StubFactory {
    calls: Arc<Mutex<Vec<(String, u32)>>>,
}

impl AgentFactory for StubFactory {
    fn create_agent(
        &self,
        owner_ctx: Context,
        options: CreateAgentOptions,
    ) -> BoxFuture<'static, Result<AgentHandle, BoxError>> {
        self.calls
            .lock()
            .unwrap()
            .push(("create".into(), owner_ctx.fiber().uid()));
        let agent = stub_agent(&owner_ctx, options.session_id.as_ref().unwrap().as_ref());
        Box::pin(async move { Ok(AgentHandle::new(agent, || Box::pin(async { Ok(()) }))) })
    }
    fn resume(
        &self,
        owner_ctx: Context,
        options: ResumeAgentOptions,
    ) -> BoxFuture<'static, Result<AgentHandle, BoxError>> {
        self.calls
            .lock()
            .unwrap()
            .push(("resume".into(), owner_ctx.fiber().uid()));
        let agent = stub_agent(&owner_ctx, options.resume_session_id.as_ref());
        Box::pin(async move { Ok(AgentHandle::new(agent, || Box::pin(async { Ok(()) }))) })
    }
}

#[tokio::test]
async fn factory_seam_requires_a_factory_and_delegates_through_the_calling_context() {
    let h = mount().await;
    let agents = h.ctx.get::<Agents>().unwrap();
    let error = agents
        .create(&h.ctx, CreateAgentOptions::new(SessionId::from("s")))
        .await
        .err()
        .expect("no factory");
    assert!(error.to_string().contains("no agent factory"));
    let calls = Arc::default();
    let _disposer = agents
        .set_factory(
            &h.ctx,
            Arc::new(StubFactory {
                calls: Arc::clone(&calls),
            }),
        )
        .unwrap();
    let child = h.ctx.child("caller").unwrap();
    agents
        .create(&child, CreateAgentOptions::new(SessionId::from("create-s")))
        .await
        .unwrap();
    agents
        .resume(&child, ResumeAgentOptions::new(SessionId::from("resume-s")))
        .await
        .unwrap();
    let uid = child.fiber().uid();
    assert_eq!(
        *calls.lock().unwrap(),
        [("create".to_owned(), uid), ("resume".to_owned(), uid)]
    );
    h.kernel.shutdown().await;
}

#[tokio::test]
async fn factory_seam_rejects_a_second_factory_and_clears_the_slot_with_its_owner() {
    let h = mount().await;
    let agents = h.ctx.get::<Agents>().unwrap();
    let owner = h.ctx.child("owner").unwrap();
    let factory = |calls: &Arc<Mutex<Vec<(String, u32)>>>| {
        Arc::new(StubFactory {
            calls: Arc::clone(calls),
        }) as Arc<dyn AgentFactory>
    };
    let calls = Arc::default();
    agents.set_factory(&owner, factory(&calls)).unwrap();
    let error = agents
        .set_factory(&owner, factory(&calls))
        .err()
        .expect("second factory");
    assert!(error.to_string().contains("already registered"));
    agents
        .create(&h.ctx, CreateAgentOptions::new(SessionId::from("before-s")))
        .await
        .unwrap();
    owner.fiber().dispose().await;
    let error = agents
        .create(&h.ctx, CreateAgentOptions::new(SessionId::from("after-s")))
        .await
        .err()
        .expect("slot cleared");
    assert!(error.to_string().contains("no agent factory"));
    h.kernel.shutdown().await;
}

// ---- announce / detach orders ---------------------------------------------------------------

/// `created` and `disposed` of one agent, in the order the listeners saw them.
fn lifecycle_log(h: &common::Harness) -> Log {
    let lifecycle = log();
    let l = lifecycle.clone();
    h.ctx
        .on_serial::<AgentCreated, (), _>(move |_| {
            push(&l, "created");
            async { Ok(None) }
        })
        .unwrap();
    let l = lifecycle.clone();
    h.ctx
        .on_contained::<AgentDisposed>(move |_| {
            push(&l, "disposed");
            Ok(())
        })
        .unwrap();
    lifecycle
}

/// The detach wins: the announcement fails with `not live` and nothing is emitted.
#[tokio::test]
async fn announce_after_detach_fails_not_live_and_emits_nothing() {
    let h = mount().await;
    let agents = h.ctx.get::<Agents>().unwrap();
    let lifecycle = lifecycle_log(&h);
    let agent = stub_agent(&h.ctx, "detached-first");
    let detach = agents.enter(&h.ctx, agent.clone()).unwrap();
    detach.detach();
    let error = agents
        .announce(&h.ctx, &agent, SessionStartSource::Startup, None)
        .await
        .expect_err("not live");
    assert!(error.to_string().contains("not live"));
    assert!(read(&lifecycle).is_empty());
    h.kernel.shutdown().await;
}

/// The announcement wins: `created`, then the detach's `disposed`.
#[tokio::test]
async fn detach_after_announce_emits_created_then_disposed() {
    let h = mount().await;
    let agents = h.ctx.get::<Agents>().unwrap();
    let lifecycle = lifecycle_log(&h);
    let agent = stub_agent(&h.ctx, "announced-first");
    let detach = agents.enter(&h.ctx, agent.clone()).unwrap();
    agents
        .announce(&h.ctx, &agent, SessionStartSource::Startup, None)
        .await
        .unwrap();
    detach.detach();
    assert_eq!(read(&lifecycle), ["created", "disposed"]);
    assert!(agents.get(agent.id()).is_none());
    h.kernel.shutdown().await;
}

/// The two overlap: another thread detaches while the creation dispatch is running. The detach
/// returns at once and records the request; the removal and `disposed` follow the dispatch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_detach_from_another_thread_during_the_creation_dispatch_follows_it() {
    let h = mount().await;
    let agents = h.ctx.get::<Agents>().unwrap();
    let lifecycle = lifecycle_log(&h);
    let (entered, release) = (
        Arc::new(tokio::sync::Notify::new()),
        Arc::new(tokio::sync::Notify::new()),
    );
    let (l, en, re) = (lifecycle.clone(), entered.clone(), release.clone());
    h.ctx
        .on_serial::<AgentCreated, (), _>(move |_| {
            let (l, en, re) = (l.clone(), en.clone(), re.clone());
            async move {
                push(&l, "listener:start");
                en.notify_one();
                re.notified().await;
                push(&l, "listener:end");
                Ok(None)
            }
        })
        .unwrap();
    let agent = stub_agent(&h.ctx, "overlapping");
    let detach = agents.enter(&h.ctx, agent.clone()).unwrap();
    let announcing = {
        let (agents, ctx, agent) = (agents.clone(), h.ctx.clone(), agent.clone());
        tokio::spawn(async move {
            agents
                .announce(&ctx, &agent, SessionStartSource::Startup, None)
                .await
        })
    };
    entered.notified().await;
    // On a thread of its own, while the dispatch is parked in the listener.
    tokio::task::spawn_blocking(move || detach.detach())
        .await
        .unwrap();
    assert!(
        agents.get(agent.id()).is_some(),
        "removed while its creation dispatch was still running"
    );
    release.notify_one();
    announcing.await.unwrap().unwrap();
    assert_eq!(
        read(&lifecycle),
        ["created", "listener:start", "listener:end", "disposed"]
    );
    assert!(agents.get(agent.id()).is_none());
    h.kernel.shutdown().await;
}

// ---- turnBoundary persisted state ----------------------------------------------------------

/// A checkpoint row of the real turn-boundary projection restored over an empty log.
async fn restore_turn_boundary(val: serde_json::Value) -> Result<(), String> {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let registry = SessionProjectionRegistry::new(&ctx).unwrap();
    registry
        .register(&ctx, hoh_agent::turn_boundary_projection_definition())
        .await
        .unwrap();
    let checkpoint = [(
        "turnBoundary".to_owned(),
        hoh_session_projection::ProjectionCheckpointRow {
            ver: 2,
            seq: hoh_session::SessionSeqCursor::BeforeFirst,
            val: Some(val),
        },
    )]
    .into_iter()
    .collect();
    let result = registry
        .restore(
            &checkpoint,
            &[],
            hoh_session::SessionLogOffset::new(0).unwrap(),
            &hoh_session::SessionHeader::new("restore-turns".into(), 0),
            hoh_session::SessionLogOffset::new(0).unwrap(),
        )
        .map(|_| ())
        .map_err(|error| error.to_string());
    kernel.shutdown().await;
    result
}

#[tokio::test]
async fn restore_rejects_a_turn_boundary_state_without_its_required_keys() {
    for missing in [
        json!({ "lastTurn": 0 }),
        json!({ "openTurnStartSeq": null, "lastStepStartSeq": null, "lastStepBoundary": null }),
        json!({ "openTurnStartSeq": null, "lastStepStartSeq": null, "lastTurn": 0 }),
        json!({ "openTurnStartSeq": null, "lastStepBoundary": null, "lastTurn": 0 }),
        json!({ "lastStepStartSeq": null, "lastStepBoundary": null, "lastTurn": 0 }),
    ] {
        let error = restore_turn_boundary(missing.clone()).await.unwrap_err();
        assert!(error.contains("turnBoundary"), "{missing}: {error}");
    }
}

#[tokio::test]
async fn restore_accepts_a_complete_turn_boundary_state_with_null_keys() {
    restore_turn_boundary(json!({
        "openTurnStartSeq": null, "lastStepStartSeq": null,
        "lastStepBoundary": null, "lastTurn": 0
    }))
    .await
    .unwrap();
    restore_turn_boundary(json!({
        "openTurnStartSeq": 4, "lastStepStartSeq": 5,
        "lastStepBoundary": { "kind": "end", "seq": 6 }, "lastTurn": 2
    }))
    .await
    .unwrap();
}
