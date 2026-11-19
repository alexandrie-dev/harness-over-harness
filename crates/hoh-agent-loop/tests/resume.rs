//! Port of resume.spec.ts (the factory create/resume cases over the JSONL backend): a stored
//! session is created, repaired, resumed and released through real files in a temporary root.
//! The failure-injection cases that replace persistence handles with stubs are deferred (see
//! PORT_NOTES.md).
mod common;

use common::*;
use hoh_agent::{
    Agent, AgentCreated, AgentHandle, Agents, CreateAgentOptions, ResumeAgentOptions,
    SessionStartSource,
};
use hoh_error::BoxError;
use hoh_session::{
    PrepareSessionOptions, SessionEvent, SessionHeader, SessionId, Sessions, TOOL_OUTCOME_UNKNOWN,
};
use hoh_session_persistence::{SessionAccess, SessionPersistenceService};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, Mutex};

fn raw(event_type: &str, seq: u64, time: u64, data: Value) -> Arc<SessionEvent> {
    Arc::new(SessionEvent::from_value(
        json!({"type": event_type, "seq": seq, "time": time, "data": data}),
    ))
}

fn turn_pair() -> Vec<Arc<SessionEvent>> {
    vec![
        raw("turn/start", 0, 1, json!({"turn": 1})),
        raw(
            "turn/end",
            1,
            2,
            json!({"turn": 1, "reason": {"kind": "completed"}}),
        ),
    ]
}

/// Seed one stored session through the persistence seam (header minted by the store).
async fn seed_stored_session(env: &Env, id: &str, events: Vec<Arc<SessionEvent>>) {
    let store = env.ctx.get::<Sessions>().expect("sessions");
    let detached = store
        .prepare(
            Some(SessionId::from(id)),
            PrepareSessionOptions::Create(Default::default()),
        )
        .expect("prepared");
    let header: SessionHeader =
        serde_json::from_value((*detached.header()).clone()).expect("header");
    let persistence = env
        .ctx
        .get::<SessionPersistenceService>()
        .expect("persistence");
    let handle = persistence.create(&header, None).await.expect("create");
    handle.append(&events, None).await.expect("append");
    handle.close().await.expect("close");
}

/// The physical validated log of one stored session, through a read handle.
async fn read_stored(env: &Env, id: &str) -> Vec<Arc<SessionEvent>> {
    let persistence = env
        .ctx
        .get::<SessionPersistenceService>()
        .expect("persistence");
    let handle = persistence
        .open(&SessionId::from(id), SessionAccess::Read, None)
        .await
        .expect("open");
    let read = handle.read(None, None, None).await.expect("read");
    handle.close().await.expect("close");
    read.events
}

fn types_of(events: &[Arc<SessionEvent>]) -> Vec<String> {
    events.iter().map(|e| e.event_type().to_owned()).collect()
}

fn agents(env: &Env) -> hoh_kernel::Service<Agents> {
    env.ctx.get::<Agents>().expect("agents")
}

async fn create_handle(env: &Env, options: CreateAgentOptions) -> AgentHandle {
    agents(env)
        .create(&env.ctx, options)
        .await
        .expect("agent created")
}

fn create_options(id: &str) -> CreateAgentOptions {
    let mut options = CreateAgentOptions::new(SessionId::from(id));
    options.agent_options = Some(mock_options());
    options
}

async fn resume(env: &Env, id: &str) -> Result<AgentHandle, BoxError> {
    let mut options = ResumeAgentOptions::new(SessionId::from(id));
    options.agent_options = Some(mock_options());
    agents(env).resume(&env.ctx, options).await
}

async fn run_turn(agent: &Arc<dyn Agent>, text: &str) {
    send_and_wait(agent, text).await;
}

// ---------------------------------------------------------------- create

#[tokio::test]
async fn create_uses_the_caller_supplied_session_id_and_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let env = persistent_harness(MockAdapter::chunks(vec![text_response("hi")]), dir.path()).await;
    let mut options = create_options("custom-session");
    options.meta = Some(hoh_agent::AgentSessionMeta {
        cwd: Some("/w".into()),
        ..Default::default()
    });
    let handle = create_handle(&env, options).await;

    assert_eq!(handle.agent.session().id().as_ref(), "custom-session");
    assert_eq!(handle.agent.session().header()["cwd"], "/w");
    handle.dispose().await.expect("dispose");
    env.close().await;
}

#[tokio::test]
async fn create_works_without_meta() {
    let dir = tempfile::tempdir().unwrap();
    let env = persistent_harness(MockAdapter::chunks(vec![]), dir.path()).await;
    let handle = create_handle(&env, create_options("nometa-session")).await;
    assert!(handle.agent.session().header().get("cwd").is_none());
    handle.dispose().await.expect("dispose");
    env.close().await;
}

#[tokio::test]
async fn create_rejects_a_duplicate_identity_without_orphaning_a_session() {
    let dir = tempfile::tempdir().unwrap();
    let env = persistent_harness(MockAdapter::chunks(vec![text_response("hi")]), dir.path()).await;
    let first = create_handle(&env, create_options("sess-a")).await;

    let error = agents(&env)
        .create(&env.ctx, create_options("sess-a"))
        .await
        .err()
        .expect("duplicate identity is refused");

    assert!(error.to_string().contains("already exists"), "{error}");
    assert_eq!(env.ctx.get::<Sessions>().unwrap().list().len(), 1);
    first.dispose().await.expect("dispose");
    env.close().await;
}

#[tokio::test]
async fn a_created_agent_stores_its_seed_and_live_turn_durably_through_its_handle() {
    let dir = tempfile::tempdir().unwrap();
    let env = persistent_harness(
        MockAdapter::chunks(vec![text_response("stored")]),
        dir.path(),
    )
    .await;
    let mut options = create_options("durable-create");
    options.seed = Some(turn_pair());
    options.meta = Some(hoh_agent::AgentSessionMeta {
        cwd: Some("/w".into()),
        ..Default::default()
    });
    let handle = create_handle(&env, options).await;

    run_turn(&handle.agent, "q").await;
    handle.dispose().await.expect("dispose");

    let stored = read_stored(&env, "durable-create").await;
    let seqs: Vec<u64> = stored.iter().map(|e| e.seq().get()).collect();
    assert_eq!(seqs, (0..seqs.len() as u64).collect::<Vec<_>>());
    // The constructor seed (turn 1 + its end-seed marker) precedes the live turn 2.
    assert_eq!(&types_of(&stored)[..2], ["turn/start", "turn/end"]);
    assert_eq!(stored[2].event_type(), "session/end-seed");
    let turns: Vec<u64> = stored
        .iter()
        .filter(|e| e.event_type() == "turn/start")
        .map(|e| e.data()["turn"].as_u64().unwrap())
        .collect();
    assert_eq!(turns, [1, 2]);
    assert!(
        stored
            .iter()
            .any(|e| e.event_type() == "turn/end" && e.data()["turn"] == 2)
    );
    assert!(
        serde_json::to_string(&stored.iter().map(|e| e.as_value()).collect::<Vec<_>>())
            .unwrap()
            .contains("stored")
    );
    env.close().await;
}

#[tokio::test]
async fn agent_dispose_releases_write_ownership_of_its_stored_session() {
    let dir = tempfile::tempdir().unwrap();
    let env = persistent_harness(MockAdapter::chunks(vec![text_response("hi")]), dir.path()).await;
    let handle = create_handle(&env, create_options("ownership-release")).await;
    let persistence = env.ctx.get::<SessionPersistenceService>().unwrap();
    let id = SessionId::from("ownership-release");

    let blocked = persistence
        .open(&id, SessionAccess::Write, None)
        .await
        .err()
        .expect("the live agent owns the write handle");
    assert!(
        blocked
            .to_string()
            .contains("already owned by an active write handle"),
        "{blocked}"
    );

    // Materialize the log so the session outlives its creator handle.
    run_turn(&handle.agent, "q").await;
    handle.dispose().await.expect("dispose");
    let reopened = persistence
        .open(&id, SessionAccess::Write, None)
        .await
        .expect("ownership released");
    assert_eq!(reopened.access(), SessionAccess::Write);
    reopened.close().await.expect("close");
    env.close().await;
}

// ---------------------------------------------------------------- resume

#[tokio::test]
async fn resume_cannot_crash_repair_a_turn_owned_by_a_live_agent() {
    let dir = tempfile::tempdir().unwrap();
    let env = persistent_harness(
        MockAdapter::chunks(vec![text_response("unused")]),
        dir.path(),
    )
    .await;
    let first = create_handle(&env, create_options("live-resume-race")).await;
    let session = first.agent.session().clone();
    session
        .append("turn/start", json!({"turn": 1}), None)
        .unwrap();
    env.ctx
        .get::<Sessions>()
        .unwrap()
        .flush(&session)
        .await
        .unwrap();

    let error = resume(&env, "live-resume-race")
        .await
        .err()
        .expect("the live writer owns the session");
    assert!(
        error
            .to_string()
            .contains("already owned by an active write handle"),
        "{error}"
    );

    session
        .append(
            "turn/end",
            json!({"turn": 1, "reason": {"kind": "completed"}}),
            None,
        )
        .unwrap();
    env.ctx
        .get::<Sessions>()
        .unwrap()
        .flush(&session)
        .await
        .unwrap();
    let stored = read_stored(&env, "live-resume-race").await;
    assert_eq!(types_of(&stored), ["turn/start", "turn/end"]);
    assert_eq!(stored[1].data()["reason"]["kind"], "completed");
    env.close().await;
}

#[tokio::test]
async fn resume_appends_interrupted_turn_closers_durably_through_the_handle() {
    let dir = tempfile::tempdir().unwrap();
    // Lifecycle 1: store an interrupted log: an open turn with no turn/end.
    let first = persistent_harness(MockAdapter::chunks(vec![]), dir.path()).await;
    seed_stored_session(
        &first,
        "interrupted-resume",
        vec![raw("turn/start", 0, 1, json!({"turn": 1}))],
    )
    .await;
    // A read returns the PHYSICAL validated log: no synthetic closers.
    assert_eq!(
        types_of(&read_stored(&first, "interrupted-resume").await),
        ["turn/start"]
    );
    first.close().await;

    // Lifecycle 2: resume repairs the tail and stores the repair durably.
    let second = persistent_harness(MockAdapter::chunks(vec![]), dir.path()).await;
    let handle = resume(&second, "interrupted-resume")
        .await
        .expect("resumed");
    let live: Vec<String> = handle
        .agent
        .session()
        .snapshot_events(None, None)
        .iter()
        .map(|e| e.event_type().to_owned())
        .collect();
    assert_eq!(live, ["turn/start", "turn/end", "session/end-seed"]);
    handle.dispose().await.expect("dispose");

    let stored = read_stored(&second, "interrupted-resume").await;
    assert_eq!(
        types_of(&stored),
        ["turn/start", "turn/end", "session/end-seed"]
    );
    assert_eq!(stored[1].data()["reason"]["kind"], "interrupted");
    second.close().await;
}

#[tokio::test]
async fn resume_closes_an_interrupted_tool_call_durably() {
    let dir = tempfile::tempdir().unwrap();
    let first = persistent_harness(MockAdapter::chunks(vec![]), dir.path()).await;
    seed_stored_session(
        &first,
        "interrupted-tool-resume",
        vec![
            raw("turn/start", 0, 1, json!({"turn": 1})),
            raw("step/start", 1, 1, json!({"turn": 1, "step": 1})),
            Arc::new(SessionEvent::from_value(json!({
                "type": "assistant/message", "seq": 2, "time": 2, "surfaceOp": "append",
                "data": {
                    "turn": 1, "step": 1, "stream": [],
                    "message": {
                        "role": "assistant",
                        "id": "assistant-1",
                        "content": [{"type": "tool-call", "id": "call-1", "name": "bash", "arguments": "{}"}],
                        "source": {"kind": "model", "provider": "mock", "model": "mock"}
                    }
                }
            }))),
            raw(
                "tool/call",
                3,
                2,
                json!({"turn": 1, "step": 1, "callId": "call-1", "name": "bash", "arguments": "{}"}),
            ),
        ],
    )
    .await;
    first.close().await;

    let second = persistent_harness(MockAdapter::chunks(vec![]), dir.path()).await;
    let handle = resume(&second, "interrupted-tool-resume")
        .await
        .expect("resumed");
    handle.dispose().await.expect("dispose");

    // The multi-closer set lands durably in one contiguous batch, and the synthetic
    // tool/result cites the recorded tool/call seq.
    let stored = read_stored(&second, "interrupted-tool-resume").await;
    assert_eq!(
        types_of(&stored),
        [
            "turn/start",
            "step/start",
            "assistant/message",
            "tool/call",
            "tool/result",
            "step/end",
            "turn/end",
            "session/end-seed"
        ]
    );
    let seqs: Vec<u64> = stored.iter().map(|e| e.seq().get()).collect();
    assert_eq!(seqs, [0, 1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(stored[4].as_value()["sourceEventSeqs"], json!([3]));
    assert_eq!(stored[4].data()["error"]["code"], TOOL_OUTCOME_UNKNOWN);
    assert_eq!(stored[6].data()["reason"]["kind"], "interrupted");
    second.close().await;
}

#[tokio::test]
async fn resume_over_a_torn_physical_tail_continues_from_the_committed_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let first = persistent_harness(MockAdapter::chunks(vec![]), dir.path()).await;
    seed_stored_session(
        &first,
        "torn-tail-resume",
        vec![raw("turn/start", 0, 1, json!({"turn": 1}))],
    )
    .await;
    first.close().await;

    // Crash artifact: a half-written record with no trailing newline.
    let log = find_log(dir.path());
    let mut contents = std::fs::read(&log).expect("log readable");
    contents.extend_from_slice(br#"{"type":"assistant/chunk","seq":1,"ti"#);
    std::fs::write(&log, contents).expect("tear the tail");

    // Resume truncates the torn tail under its write open, then appends the closers
    // immediately after the committed prefix: no gap, no fragment.
    let second = persistent_harness(MockAdapter::chunks(vec![]), dir.path()).await;
    let handle = resume(&second, "torn-tail-resume").await.expect("resumed");
    let live: Vec<String> = handle
        .agent
        .session()
        .snapshot_events(None, None)
        .iter()
        .map(|e| e.event_type().to_owned())
        .collect();
    assert_eq!(live, ["turn/start", "turn/end", "session/end-seed"]);
    handle.dispose().await.expect("dispose");

    let stored: Vec<String> = read_stored(&second, "torn-tail-resume")
        .await
        .iter()
        .map(|e| format!("{}@{}", e.event_type(), e.seq().get()))
        .collect();
    assert_eq!(stored, ["turn/start@0", "turn/end@1", "session/end-seed@2"]);
    second.close().await;
}

fn find_log(root: &Path) -> std::path::PathBuf {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("readable root").flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.to_string_lossy().ends_with(".jsonl") {
                found.push(path);
            }
        }
    }
    assert_eq!(
        found.len(),
        1,
        "expected exactly one session log: {found:?}"
    );
    found.remove(0)
}

#[tokio::test]
async fn resume_of_a_session_with_no_cwd_carries_no_cwd_header() {
    let dir = tempfile::tempdir().unwrap();
    // Lifecycle 1: create a no-cwd session and run a turn.
    let first = persistent_harness(MockAdapter::chunks(vec![text_response("a")]), dir.path()).await;
    let handle = create_handle(&first, create_options("nocwd-sess")).await;
    run_turn(&handle.agent, "q").await;
    // Agent disposal drains the writer through the still-open handle.
    handle.dispose().await.expect("dispose");
    first.close().await;

    // Lifecycle 2: resume it; the header cwd stays absent.
    let second =
        persistent_harness(MockAdapter::chunks(vec![text_response("b")]), dir.path()).await;
    let resumed = resume(&second, "nocwd-sess").await.expect("resumed");
    assert!(resumed.agent.session().header().get("cwd").is_none());
    resumed.dispose().await.expect("dispose");
    second.close().await;
}

#[tokio::test]
async fn agent_created_fires_startup_for_create_and_resume_for_resume() {
    let dir = tempfile::tempdir().unwrap();
    // Lifecycle 1: a fresh create announces creation with source `startup`.
    let first = persistent_harness(MockAdapter::chunks(vec![text_response("a")]), dir.path()).await;
    let sources = Arc::new(Mutex::new(Vec::<SessionStartSource>::new()));
    {
        let sink = sources.clone();
        first
            .ctx
            .on_serial::<AgentCreated, (), _>(move |event| {
                let (sink, source) = (sink.clone(), event.source);
                async move {
                    lock(&sink).push(source);
                    Ok(None)
                }
            })
            .expect("listener");
    }
    let handle = create_handle(&first, create_options("start-sess")).await;
    assert_eq!(*lock(&sources), [SessionStartSource::Startup]);
    run_turn(&handle.agent, "q").await;
    handle.dispose().await.expect("dispose");
    first.close().await;

    // Lifecycle 2: resuming the persisted session announces creation `resume`.
    let second =
        persistent_harness(MockAdapter::chunks(vec![text_response("b")]), dir.path()).await;
    let resumed_sources = Arc::new(Mutex::new(Vec::<SessionStartSource>::new()));
    {
        let sink = resumed_sources.clone();
        second
            .ctx
            .on_serial::<AgentCreated, (), _>(move |event| {
                let (sink, source) = (sink.clone(), event.source);
                async move {
                    lock(&sink).push(source);
                    Ok(None)
                }
            })
            .expect("listener");
    }
    let resumed = resume(&second, "start-sess").await.expect("resumed");
    assert_eq!(*lock(&resumed_sources), [SessionStartSource::Resume]);
    resumed.dispose().await.expect("dispose");
    second.close().await;
}

#[tokio::test]
async fn resume_reloads_a_persisted_session_and_numbering_continues() {
    let dir = tempfile::tempdir().unwrap();
    // Lifecycle 1: run one full turn, persisting it.
    let first = persistent_harness(
        MockAdapter::chunks(vec![text_response("first answer")]),
        dir.path(),
    )
    .await;
    let mut options = create_options("sess-resume");
    options.meta = Some(hoh_agent::AgentSessionMeta {
        cwd: Some("/w".into()),
        ..Default::default()
    });
    let h1 = create_handle(&first, options).await;
    run_turn(&h1.agent, "first question").await;
    let events1 = h1.agent.session().snapshot_events(None, None);
    let seqs1: Vec<u64> = events1.iter().map(|e| e.seq().get()).collect();
    assert_eq!(seqs1, (0..seqs1.len() as u64).collect::<Vec<_>>());
    let history1 = h1.agent.session().derive_messages().expect("messages");
    h1.dispose().await.expect("dispose");
    first.close().await;

    // Lifecycle 2: a brand-new context over the SAME root; resume the session.
    let second = persistent_harness(
        MockAdapter::chunks(vec![text_response("second answer")]),
        dir.path(),
    )
    .await;
    let h2 = resume(&second, "sess-resume").await.expect("resumed");
    let a2 = h2.agent.clone();
    // The resumed session carries the prior history followed by one end-seed event.
    assert_eq!(a2.session().id().as_ref(), "sess-resume");
    let resumed_events = a2.session().snapshot_events(None, None);
    assert_eq!(resumed_events.len(), events1.len() + 1);
    assert_eq!(a2.session().first_live_seq().get(), events1.len() as u64);
    assert_eq!(
        resumed_events.last().unwrap().event_type(),
        "session/end-seed"
    );
    assert_eq!(a2.session().derive_messages().expect("messages"), history1);

    // A new turn continues numbering (turn 2) with contiguous seqs.
    run_turn(&a2, "second question").await;
    let all: Vec<u64> = a2
        .session()
        .snapshot_events(None, None)
        .iter()
        .map(|e| e.seq().get())
        .collect();
    assert_eq!(all, (0..all.len() as u64).collect::<Vec<_>>());
    let turns: Vec<u64> = events_of(&a2, "turn/start")
        .iter()
        .map(|e| e["data"]["turn"].as_u64().unwrap())
        .collect();
    assert_eq!(turns, [1, 2]);
    h2.dispose().await.expect("dispose");
    second.close().await;
}

#[tokio::test]
async fn resume_rejects_when_session_persistence_is_not_configured() {
    let env = harness(MockAdapter::chunks(vec![text_response("x")])).await;
    let error = resume(&env, "nope")
        .await
        .err()
        .expect("no persistence backend");
    assert!(
        error
            .to_string()
            .contains("session persistence is not configured"),
        "{error}"
    );
}

// ---------------------------------------------------------------- failed resume releases ownership

#[tokio::test]
async fn a_resume_failing_after_the_load_releases_the_write_handle_before_returning_the_error() {
    let dir = tempfile::tempdir().unwrap();
    let env = persistent_harness(MockAdapter::chunks(vec![]), dir.path()).await;
    seed_stored_session(&env, "rollback", turn_pair()).await;

    let mut options = ResumeAgentOptions::new(SessionId::from("rollback"));
    options.agent_options = Some(mock_options());
    let setup: hoh_agent::AgentSetup = Arc::new(|_ctx, _agent| {
        Box::pin(async {
            Err::<Option<hoh_agent::AgentSetupCommit>, BoxError>("setup refused".into())
        })
    });
    options.setup = Some(setup);
    let failure = agents(&env).resume(&env.ctx, options).await;
    let error = failure.err().expect("the failing setup fails the resume");
    assert!(error.to_string().contains("setup refused"), "{error}");

    // With no yield in between: the rolled-back resume must already have released ownership.
    let persistence = env.ctx.get::<SessionPersistenceService>().unwrap();
    let reopened = persistence
        .open(&SessionId::from("rollback"), SessionAccess::Write, None)
        .await
        .unwrap_or_else(|blocked| {
            panic!("the failed resume left the write handle open: {blocked}")
        });
    reopened.close().await.expect("close");

    // And an immediate second resume of the same id succeeds.
    let again = resume(&env, "rollback").await.expect("second resume");
    again.dispose().await.expect("dispose");
    env.close().await;
}

// ---------------------------------------------------------------- failure injection at the store

/// The JSONL store behind a wrapper whose handles fail `read` on demand and release write
/// ownership only after a delay: a rollback that merely spawns the close leaves the id owned when
/// the error surfaces.
struct SlowClosePersistence {
    inner: hoh_session_persistence_jsonl::JsonlSessionPersistence,
    fail_reads: Arc<std::sync::atomic::AtomicBool>,
}

struct SlowCloseHandle {
    inner: Arc<dyn hoh_session_persistence::SessionHandle>,
    fail_reads: Arc<std::sync::atomic::AtomicBool>,
}

impl hoh_session_persistence::SessionPersistence for SlowClosePersistence {
    fn identity(&self) -> hoh_session_persistence::SessionPersistenceIdentity {
        self.inner.identity()
    }
    fn create<'a>(
        &'a self,
        header: &'a SessionHeader,
        options: Option<&'a hoh_session_persistence::SessionPersistenceCreateOptions>,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<Arc<dyn hoh_session_persistence::SessionHandle>, BoxError>,
    > {
        self.inner.create(header, options)
    }
    fn open<'a>(
        &'a self,
        id: &'a SessionId,
        access: SessionAccess,
        options: Option<&'a hoh_session_persistence::SessionPersistenceOpenOptions>,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<Arc<dyn hoh_session_persistence::SessionHandle>, BoxError>,
    > {
        Box::pin(async move {
            let inner = self.inner.open(id, access, options).await?;
            Ok(Arc::new(SlowCloseHandle {
                inner,
                fail_reads: self.fail_reads.clone(),
            })
                as Arc<dyn hoh_session_persistence::SessionHandle>)
        })
    }
    fn flush(&self) -> futures_util::future::BoxFuture<'_, Result<(), BoxError>> {
        self.inner.flush()
    }
    fn stat<'a>(
        &'a self,
        id: &'a SessionId,
        options: Option<&'a hoh_session_persistence::SessionPersistenceStatOptions>,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<Option<hoh_session_persistence::SessionPersistenceSnapshot>, BoxError>,
    > {
        self.inner.stat(id, options)
    }
    fn list<'a>(
        &'a self,
        options: Option<&'a hoh_session_persistence::SessionPersistenceListOptions>,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<Vec<hoh_session_persistence::SessionPersistenceSnapshot>, BoxError>,
    > {
        self.inner.list(options)
    }
}

impl hoh_session_persistence::SessionHandle for SlowCloseHandle {
    fn id(&self) -> &SessionId {
        self.inner.id()
    }
    fn header(&self) -> &SessionHeader {
        self.inner.header()
    }
    fn inherited_event_count(&self) -> hoh_session::SessionLogOffset {
        self.inner.inherited_event_count()
    }
    fn access(&self) -> SessionAccess {
        self.inner.access()
    }
    fn read<'a>(
        &'a self,
        offset: Option<u64>,
        length: Option<u64>,
        options: Option<&'a hoh_session_persistence::SessionHandleReadOptions>,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<hoh_session_persistence::SessionHandleReadResult, BoxError>,
    > {
        if self.fail_reads.load(std::sync::atomic::Ordering::SeqCst) {
            return Box::pin(async { Err::<_, BoxError>("injected read failure".into()) });
        }
        self.inner.read(offset, length, options)
    }
    fn append<'a>(
        &'a self,
        events: &'a [Arc<SessionEvent>],
        options: Option<&'a hoh_session_persistence::SessionHandleAppendOptions>,
    ) -> futures_util::future::BoxFuture<'a, Result<(), BoxError>> {
        self.inner.append(events, options)
    }
    fn flush<'a>(
        &'a self,
        options: Option<&'a hoh_session_persistence::SessionHandleFlushOptions>,
    ) -> futures_util::future::BoxFuture<'a, Result<(), BoxError>> {
        self.inner.flush(options)
    }
    fn close(&self) -> futures_util::future::BoxFuture<'_, Result<(), BoxError>> {
        Box::pin(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            self.inner.close().await
        })
    }
}

#[tokio::test]
async fn a_resume_failing_in_the_read_waits_for_the_deferred_close_before_the_error_surfaces() {
    let dir = tempfile::tempdir().unwrap();
    let env = try_harness(
        MockAdapter::chunks(vec![]),
        json!({"agents": []}),
        None,
        None,
    )
    .await
    .expect("harness");
    let inner = hoh_session_persistence_jsonl::JsonlSessionPersistence::new(
        &env.ctx,
        serde_json::from_value(
            json!({"root": dir.path().to_str().unwrap(), "compression": "none"}),
        )
        .expect("config"),
    )
    .await
    .expect("jsonl backend");
    let fail_reads = Arc::new(std::sync::atomic::AtomicBool::new(false));
    env.ctx
        .provide::<SessionPersistenceService>(Arc::new(SlowClosePersistence {
            inner,
            fail_reads: fail_reads.clone(),
        }))
        .await
        .expect("provide persistence");
    seed_stored_session(&env, "slow-close", turn_pair()).await;

    fail_reads.store(true, std::sync::atomic::Ordering::SeqCst);
    let error = resume(&env, "slow-close")
        .await
        .err()
        .expect("the failing read fails the resume");
    assert!(
        error.to_string().contains("injected read failure"),
        "{error}"
    );

    // The error surfaced only after the (delayed) close completed: ownership is already free.
    fail_reads.store(false, std::sync::atomic::Ordering::SeqCst);
    let again = resume(&env, "slow-close")
        .await
        .expect("an immediate second resume owns the session");
    again.dispose().await.expect("dispose");
    env.close().await;
}
