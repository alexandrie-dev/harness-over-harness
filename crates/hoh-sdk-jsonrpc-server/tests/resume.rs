mod support;
use hoh_agent::{
    AgentCreated, AgentSessionMeta, Agents, CreateAgentOptions, ModelSelection, SessionStartSource,
};
use hoh_protocol::SessionResumeParams;
use hoh_sdk_jsonrpc_server::integration::ScheduleResolveAgent;
use hoh_session::{SessionEvent, SessionId};
use hoh_session_persistence::{SessionAccess, SessionPersistenceService};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use support::*;
async fn seeded(root: &std::path::Path, id: &str) -> Vec<Arc<SessionEvent>> {
    let h = Env::persistent(root).await;
    let mut p = h.params();
    p.cwd = root.to_str().unwrap().into();
    h.server.initialize(p).await.unwrap();
    h.prompt(id, "remember kiwi").await;
    h.wait(id).await;
    let original = h.agent(id).session().snapshot_events(None, None).to_vec();
    h.close().await;
    original
}
async fn resume(h: &Env, id: &str) -> Result<Value, hoh_error::BoxError> {
    h.server
        .resume(SessionResumeParams {
            session_id: id.into(),
        })
        .await
}
#[tokio::test]
async fn rejects_independent_subagent_restoration_before_opening_or_repairing_its_log() {
    let root = tempfile::tempdir().unwrap();
    let h = Env::persistent(root.path()).await;
    h.initialize().await;
    let child = h
        .ctx
        .get::<Agents>()
        .unwrap()
        .create(
            &h.ctx,
            CreateAgentOptions {
                session_id: Some("saved-child".into()),
                meta: Some(AgentSessionMeta {
                    origin: Some("subagent".into()),
                    parent_session: Some("creator".into()),
                    cwd: Some(root.path().to_str().unwrap().into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    child
        .agent
        .session()
        .append("turn/start", json!({"turn":1}), None)
        .unwrap();
    child.dispose().await.unwrap();
    for _ in 0..2 {
        assert!(
            resume(&h, "saved-child")
                .await
                .unwrap_err()
                .to_string()
                .contains("resume its root session instead")
        );
    }
    assert!(
        h.ctx
            .get::<Agents>()
            .unwrap()
            .get(&"saved-child".into())
            .is_none()
    );
    let handle = h
        .ctx
        .get::<SessionPersistenceService>()
        .unwrap()
        .open(&"saved-child".into(), SessionAccess::Read, None)
        .await
        .unwrap();
    assert_eq!(
        handle
            .read(None, None, None)
            .await
            .unwrap()
            .events
            .last()
            .unwrap()
            .event_type(),
        "turn/start"
    );
    handle.close().await.unwrap();
    h.close().await;
}
#[tokio::test]
async fn applies_the_initialized_route_and_effort_on_resume() {
    for (model, effort, expected) in [
        ("mock", None, Some("high")),
        ("other", None, None),
        ("mock", Some("low"), Some("low")),
    ] {
        let root = tempfile::tempdir().unwrap();
        let h = Env::persistent(root.path()).await;
        h.initialize().await;
        h.prompt("saved-effort", "remember").await;
        h.wait("saved-effort").await;
        h.agent("saved-effort")
            .model()
            .select(&ModelSelection {
                provider: "mock".into(),
                model: "mock".into(),
                reasoning_effort: Some("high".into()),
            })
            .unwrap();
        h.close().await;
        let h = Env::persistent(root.path()).await;
        *lock(&h.model.reasoning) = Some(
            serde_json::from_value(
                json!({"efforts":[{"id":"high","name":"High"},{"id":"low","name":"Low"}]}),
            )
            .unwrap(),
        );
        let mut p = h.params();
        p.model = model.into();
        p.reasoning_effort = effort.map(str::to_owned);
        h.server.initialize(p).await.unwrap();
        resume(&h, "saved-effort").await.unwrap();
        assert_eq!(
            h.agent("saved-effort").model().current(),
            Some(ModelSelection {
                provider: "mock".into(),
                model: model.into(),
                reasoning_effort: expected.map(Into::into)
            })
        );
        assert!(lock(&h.model.requests).is_empty());
        h.close().await;
    }
}
#[tokio::test]
async fn restores_a_non_main_identity_preserves_history_and_origin_and_appends_a_new_turn() {
    let root = tempfile::tempdir().unwrap();
    let original = seeded(root.path(), "saved-session").await;
    let h = Env::persistent(root.path()).await;
    let mut p = h.params();
    p.model = "resumed-model".into();
    p.max_tokens = Some(serde_json::from_value(json!(123)).unwrap());
    h.server.initialize(p).await.unwrap();
    assert_eq!(resume(&h, "saved-session").await.unwrap(), json!({}));
    let restored = h.agent("saved-session");
    assert_eq!(
        restored.session().header()["cwd"],
        root.path().to_str().unwrap()
    );
    let events = restored.session().snapshot_events(None, None);
    assert_eq!(&events[..original.len()], original);
    assert_eq!(events[original.len()].event_type(), "session/end-seed");
    assert_eq!(events.last().unwrap().event_type(), "model/selection");
    assert!(lock(&h.model.requests).is_empty());
    assert_eq!(h.peer.events("session.event").len(), 1);
    assert_eq!(
        h.peer.events("session.event")[0]["event"]["type"],
        "model/selection"
    );
    h.prompt("saved-session", "what did I say?").await;
    h.wait("saved-session").await;
    let request = serde_json::to_value(&lock(&h.model.requests)[0]).unwrap();
    let text = hoh_util_values::js_json_stringify(&request);
    for part in ["remember kiwi", "answer", "what did I say?"] {
        assert!(text.contains(part));
    }
    assert_eq!(request["model"], "resumed-model");
    assert_eq!(request["maxTokens"], 123);
    assert!(
        resume(&h, "saved-session")
            .await
            .unwrap_err()
            .to_string()
            .contains("already open")
    );
    h.close().await;
}
#[tokio::test]
async fn repairs_an_interrupted_final_turn_before_accepting_new_input() {
    let root = tempfile::tempdir().unwrap();
    seeded(root.path(), "interrupted").await;
    let h = Env::persistent(root.path()).await;
    let persistence = h.ctx.get::<SessionPersistenceService>().unwrap();
    let id = SessionId::from("interrupted");
    let handle = persistence
        .open(&id, SessionAccess::Write, None)
        .await
        .unwrap();
    let events = handle.read(None, None, None).await.unwrap().events;
    let last = events.last().unwrap();
    let mut new = Vec::new();
    for (i, tag, data) in [
        (1, "turn/start", json!({"turn":2})),
        (2, "step/start", json!({"turn":2,"step":1})),
    ] {
        new.push(Arc::new(SessionEvent::from_value(
            json!({"type":tag,"seq":last.seq().get()+i,"time":last.time()+i as i64,"data":data}),
        )));
    }
    handle.append(&new, None).await.unwrap();
    handle.flush(None).await.unwrap();
    handle.close().await.unwrap();
    h.initialize().await;
    resume(&h, "interrupted").await.unwrap();
    let a = h.agent("interrupted");
    assert_eq!(
        a.session()
            .snapshot_events(None, None)
            .iter()
            .rfind(|e| e.event_type() == "turn/end")
            .unwrap()
            .data()["reason"]["kind"],
        "interrupted"
    );
    h.prompt("interrupted", "continue").await;
    h.wait("interrupted").await;
    let events = a.session().snapshot_events(None, None);
    let end = events
        .iter()
        .rfind(|e| e.event_type() == "turn/end")
        .unwrap();
    assert_eq!(end.data()["turn"], 3);
    assert_eq!(end.data()["reason"]["kind"], "completed");
    h.close().await;
}
#[tokio::test]
async fn rejects_missing_history_and_releases_a_failed_resume_for_retry() {
    let root = tempfile::tempdir().unwrap();
    let h = Env::persistent(root.path()).await;
    h.initialize().await;
    for _ in 0..2 {
        assert!(resume(&h, "missing").await.is_err());
        assert!(h.ctx.get::<Agents>().unwrap().list().is_empty());
    }
    h.close().await;
}
fn gate(
    h: &Env,
) -> (
    Arc<tokio::sync::Notify>,
    Arc<tokio::sync::Semaphore>,
    Arc<AtomicUsize>,
) {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let count = Arc::new(AtomicUsize::new(0));
    let (e, r, c) = (entered.clone(), release.clone(), count.clone());
    h.ctx
        .on_serial::<AgentCreated, (), _>(move |event| {
            let (e, r, c) = (e.clone(), r.clone(), c.clone());
            async move {
                if event.source == SessionStartSource::Resume {
                    c.fetch_add(1, Ordering::SeqCst);
                    e.notify_one();
                    r.acquire().await.unwrap().forget();
                }
                Ok(None)
            }
        })
        .unwrap();
    (entered, release, count)
}
#[tokio::test]
async fn shares_pending_restoration_with_a_prompt_and_rejects_duplicate_resume() {
    let root = tempfile::tempdir().unwrap();
    seeded(root.path(), "saved").await;
    let h = Env::persistent(root.path()).await;
    h.initialize().await;
    let (entered, release, count) = gate(&h);
    let server = h.server.clone();
    let restoring = tokio::spawn(async move {
        server
            .resume(SessionResumeParams {
                session_id: "saved".into(),
            })
            .await
    });
    entered.notified().await;
    assert!(
        resume(&h, "saved")
            .await
            .unwrap_err()
            .to_string()
            .contains("already open or opening")
    );
    let server = h.server.clone();
    let prompt = tokio::spawn(async move {
        server
            .handle_request(
                "session/prompt",
                json!({"sessionId":"saved","contentBlocks":[{"type":"text","text":"followup"}]}),
            )
            .await
    });
    release.add_permits(1);
    restoring.await.unwrap().unwrap();
    prompt.await.unwrap().unwrap();
    h.wait("saved").await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(h.ctx.get::<Agents>().unwrap().list().len(), 1);
    h.close().await;
}
#[tokio::test]
async fn waits_for_pending_restoration_during_shutdown_and_releases_the_restored_agent() {
    let root = tempfile::tempdir().unwrap();
    seeded(root.path(), "saved").await;
    let h = Env::persistent(root.path()).await;
    h.initialize().await;
    let (entered, release, _) = gate(&h);
    let server = h.server.clone();
    let restoring = tokio::spawn(async move {
        server
            .resume(SessionResumeParams {
                session_id: "saved".into(),
            })
            .await
    });
    entered.notified().await;
    let shutdown = h.server.shutdown();
    assert!(
        resume(&h, "another")
            .await
            .unwrap_err()
            .to_string()
            .contains("shutting down")
    );
    release.add_permits(1);
    restoring.await.unwrap().unwrap();
    shutdown.await.unwrap();
    assert!(h.ctx.get::<Agents>().unwrap().list().is_empty());
    h.close().await;
}
#[tokio::test]
async fn rejects_restoration_when_profile_has_no_persistence_backend() {
    let h = Env::new().await;
    h.initialize().await;
    assert!(
        resume(&h, "saved")
            .await
            .unwrap_err()
            .to_string()
            .contains("session persistence is not configured")
    );
    assert!(h.ctx.get::<Agents>().unwrap().list().is_empty());
    h.close().await;
}
#[tokio::test]
async fn validates_the_wire_identity_and_requires_initialization() {
    let h = Env::new().await;
    for session_id in [Value::Null, json!(""), json!(17)] {
        assert!(
            h.server
                .handle_request("session/resume", json!({"sessionId":session_id}))
                .await
                .unwrap_err()
                .to_string()
                .contains("non-empty sessionId")
        );
    }
    assert!(
        resume(&h, "saved")
            .await
            .unwrap_err()
            .to_string()
            .contains("not initialized")
    );
    h.close().await;
}
#[tokio::test]
async fn restores_due_reminders_after_initialization_and_shares_client_resume() {
    let root = tempfile::tempdir().unwrap();
    seeded(root.path(), "saved").await;
    let h = Env::persistent(root.path()).await;
    let (entered, release, count) = gate(&h);
    let ctx = h.ctx.clone();
    let restoring = tokio::spawn(async move {
        ctx.waterfall(
            ScheduleResolveAgent {
                session_id: "saved".into(),
            },
            |_| async { Err("must not fallback".into()) },
        )
        .await
    });
    tokio::task::yield_now().await;
    assert!(h.ctx.get::<Agents>().unwrap().list().is_empty());
    let mut p = h.params();
    p.model = "reminder-model".into();
    p.max_tokens = Some(serde_json::from_value(json!(1234)).unwrap());
    h.server.initialize(p).await.unwrap();
    entered.notified().await;
    let server = h.server.clone();
    let resuming = tokio::spawn(async move {
        server
            .resume(SessionResumeParams {
                session_id: "saved".into(),
            })
            .await
    });
    let ctx = h.ctx.clone();
    let another = tokio::spawn(async move {
        ctx.waterfall(
            ScheduleResolveAgent {
                session_id: "saved".into(),
            },
            |_| async { Err("must not fallback".into()) },
        )
        .await
    });
    release.add_permits(1);
    let a = restoring.await.unwrap().unwrap();
    let b = another.await.unwrap().unwrap();
    resuming.await.unwrap().unwrap();
    assert!(Arc::ptr_eq(&a, &b));
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(a.session().header()["cwd"], root.path().to_str().unwrap());
    h.prompt("saved", "scheduled reminder").await;
    h.wait("saved").await;
    assert_eq!(lock(&h.model.requests)[0].model, "reminder-model");
    assert_eq!(lock(&h.model.requests)[0].max_tokens, Some(1234));
    assert!(
        resume(&h, "saved")
            .await
            .unwrap_err()
            .to_string()
            .contains("already open")
    );
    h.close().await;
}
#[tokio::test]
async fn reuses_sdk_owned_session_for_reminders_and_releases_failed_reminder_restoration_for_retry()
{
    let root = tempfile::tempdir().unwrap();
    let h = Env::persistent(root.path()).await;
    h.initialize().await;
    assert!(
        h.ctx
            .waterfall(
                ScheduleResolveAgent {
                    session_id: "saved".into()
                },
                |_| async { Err("fallback".into()) }
            )
            .await
            .is_err()
    );
    h.prompt("saved", "create after failed reminder").await;
    h.wait("saved").await;
    let restored = h
        .ctx
        .waterfall(
            ScheduleResolveAgent {
                session_id: "saved".into(),
            },
            |_| async { Err("fallback".into()) },
        )
        .await
        .unwrap();
    assert!(Arc::ptr_eq(&restored, &h.agent("saved")));
    assert!(
        resume(&h, "saved")
            .await
            .unwrap_err()
            .to_string()
            .contains("already open")
    );
    h.close().await;
}
#[tokio::test]
async fn releases_reminder_waiting_for_initialization_when_sdk_shuts_down() {
    let h = Env::new().await;
    let (started, pending) = h.ctx.waterfall_with_start(
        ScheduleResolveAgent {
            session_id: "saved".into(),
        },
        |_| async { Err("fallback".into()) },
    );
    started.await;
    h.server.shutdown().await.unwrap();
    let error = match pending.await {
        Err(error) => error,
        Ok(_) => panic!("scheduled resolution unexpectedly succeeded"),
    };
    assert!(error.to_string().contains("shutting down"));
    assert!(h.ctx.get::<Agents>().unwrap().list().is_empty());
    h.close().await;
}
