use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentError, AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget,
    MaintenanceFuture, MaintenanceTask, Model,
};
use hoh_error::BoxError;
use hoh_kernel::{Context, Fiber, Kernel, Service};
use hoh_llm::UserMessage;
use hoh_scope::ScopeKey;
use hoh_session::{
    AgentCancelCause, CreateSessionOptions, SESSION_FORMAT_VERSION, Session, SessionForkSource,
    SessionId, SessionLogOffset, SessionPlugin, SessionSeedEventState, SessionSeq, SessionStore,
    Sessions, SurfaceIntent,
};
use hoh_session_telemetry::{
    SessionTelemetryAttribute, SessionTelemetryCapture, SessionTelemetryCaptureOptions,
    SessionTelemetryChannel, SessionTelemetryCoordinator, SessionTelemetryRecord,
    SessionTelemetryRedactions, SessionTelemetrySeverity, SessionTelemetrySink,
};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(Default)]
struct FakeSink {
    records: Mutex<Vec<SessionTelemetryRecord>>,
    calls: Arc<Mutex<Vec<String>>>,
    fail_emit: AtomicBool,
    reject_seq: Mutex<Option<u64>>,
    fail_shutdown: AtomicBool,
    shutdown_resolved: Arc<AtomicBool>,
    flushes: AtomicUsize,
}

impl FakeSink {
    fn records(&self) -> Vec<SessionTelemetryRecord> {
        lock(&self.records).clone()
    }

    fn ledger(&self) -> Vec<SessionTelemetryRecord> {
        self.records()
            .into_iter()
            .filter(|record| record.channel == SessionTelemetryChannel::Ledger)
            .collect()
    }
}

impl SessionTelemetrySink for FakeSink {
    fn emit(&self, record: SessionTelemetryRecord) -> Result<(), BoxError> {
        if self.fail_emit.load(Ordering::SeqCst) {
            return Err("backend broke".into());
        }
        let seq = attr_u64(record.attributes.get("event.seq"));
        if let Some(rejected) = *lock(&self.reject_seq)
            && seq == Some(rejected)
        {
            return Err(format!("backend rejected seq {rejected}").into());
        }
        let identity = seq
            .map(|seq| seq.to_string())
            .or_else(|| attr_str(record.attributes.get("telemetry.op")))
            .unwrap_or_default();
        lock(&self.calls).push(format!("emit:{identity}"));
        lock(&self.records).push(record);
        Ok(())
    }

    fn flush(&self) -> Result<(), BoxError> {
        self.flushes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn shutdown(&self) -> BoxFuture<'static, Result<(), BoxError>> {
        let fail = self.fail_shutdown.load(Ordering::SeqCst);
        let calls = self.calls.clone();
        let resolved = self.shutdown_resolved.clone();
        Box::pin(async move {
            lock(&calls).push("shutdown".to_owned());
            if fail {
                Err("exporter unreachable".into())
            } else {
                resolved.store(true, Ordering::SeqCst);
                Ok(())
            }
        })
    }
}

struct Env {
    kernel: Kernel,
    ctx: Context,
    sessions: SessionStore,
    backend: Arc<FakeSink>,
    coordinator: SessionTelemetryCoordinator,
    redactions: SessionTelemetryRedactions,
}

#[derive(hoh_kernel::Inject)]
struct TelemetryDeps {
    _sessions: Service<Sessions>,
}

async fn mount_coordinator(
    ctx: &Context,
    backend: Arc<FakeSink>,
    options: SessionTelemetryCaptureOptions,
) -> (Fiber, SessionTelemetryCoordinator) {
    let mounted = Arc::new(Mutex::new(None));
    let output = mounted.clone();
    let fiber = ctx
        .inject::<TelemetryDeps, _>(move |inner, _deps| {
            let backend = backend.clone();
            let output = output.clone();
            async move {
                let coordinator = SessionTelemetryCoordinator::new(inner, backend, options)?;
                *lock(&output) = Some(coordinator);
                Ok(())
            }
        })
        .unwrap();
    fiber.settled().await.unwrap();
    let coordinator = lock(&mounted).clone().unwrap();
    (fiber, coordinator)
}

async fn mount_live_coordinator(ctx: &Context, backend: Arc<FakeSink>) -> Fiber {
    mount_coordinator(ctx, backend, SessionTelemetryCaptureOptions::default())
        .await
        .0
}

async fn setup(capture: SessionTelemetryCapture, include_history: bool) -> Env {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let sessions = (*ctx.get::<Sessions>().unwrap()).clone();
    let backend = Arc::new(FakeSink::default());
    let redactions = SessionTelemetryRedactions::new();
    let coordinator = SessionTelemetryCoordinator::new_with_redactions(
        ctx.clone(),
        backend.clone(),
        SessionTelemetryCaptureOptions {
            capture,
            include_history,
        },
        redactions.clone(),
    )
    .unwrap();
    Env {
        kernel,
        ctx,
        sessions,
        backend,
        coordinator,
        redactions,
    }
}

fn live(env: &Env, id: &str) -> Session {
    env.sessions
        .create(
            &env.ctx,
            Some(SessionId::from(id)),
            CreateSessionOptions::default(),
        )
        .unwrap()
}

fn append(session: &Session, event_type: &str, data: Value) -> Arc<hoh_session::SessionEvent> {
    session.append(event_type, data, None).unwrap()
}

fn append_surface(
    session: &Session,
    event_type: &str,
    data: Value,
) -> Arc<hoh_session::SessionEvent> {
    session
        .append(event_type, data, Some(SurfaceIntent::append()))
        .unwrap()
}

fn append_user_message(session: &Session, text: &str) -> Arc<hoh_session::SessionEvent> {
    append_surface(
        session,
        "user/message",
        json!({
            "id": format!("user-{text}"),
            "role": "user",
            "content": [{"type":"text","text":text}],
            "source": {"kind":"user"}
        }),
    )
}

fn append_assistant_message(
    session: &Session,
    turn: u64,
    step: u64,
    texts: &[&str],
    time0: i64,
) -> Arc<hoh_session::SessionEvent> {
    let text = texts.concat();
    let dt = vec![5; texts.len().saturating_sub(1)];
    let finish_time = time0 + i64::try_from(texts.len().saturating_sub(1)).unwrap() * 5;
    append_surface(
        session,
        "assistant/message",
        json!({
            "turn":turn,
            "step":step,
            "message":{
                "id":format!("assistant-{turn}-{step}"),
                "role":"assistant",
                "content":[{"type":"text","text":text}],
                "source":{"kind":"model","provider":"mock","model":"mock"}
            },
            "stream":[
                {"type":"text-chunks","time0":time0,"index":0,"dt":dt,"texts":texts},
                {"type":"chunk","time":finish_time,"chunk":{"type":"finish","reason":{"kind":"stop"}}}
            ]
        }),
    )
}

fn append_open_tool_request(session: &Session, call_id: &str) {
    append(session, "step/start", json!({"turn":1,"step":1}));
    append_surface(
        session,
        "assistant/message",
        json!({
            "turn":1,
            "step":1,
            "stream":[],
            "message":{
                "id":"assistant-tool-call",
                "role":"assistant",
                "content":[{"type":"tool-call","id":call_id,"name":"bash","arguments":"{}"}],
                "source":{"kind":"model","provider":"mock","model":"mock"}
            }
        }),
    );
}

fn attr_u64(value: Option<&SessionTelemetryAttribute>) -> Option<u64> {
    match value {
        Some(SessionTelemetryAttribute::Unsigned(value)) => Some(*value),
        Some(SessionTelemetryAttribute::Integer(value)) => u64::try_from(*value).ok(),
        _ => None,
    }
}

fn attr_str(value: Option<&SessionTelemetryAttribute>) -> Option<String> {
    match value {
        Some(SessionTelemetryAttribute::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn event_types(records: &[SessionTelemetryRecord]) -> Vec<String> {
    records
        .iter()
        .filter_map(|record| attr_str(record.attributes.get("event.type")))
        .collect()
}

fn seqs(records: &[SessionTelemetryRecord]) -> Vec<u64> {
    records
        .iter()
        .filter_map(|record| attr_u64(record.attributes.get("event.seq")))
        .collect()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct RelayAgent {
    session: Session,
    ctx: Context,
    scope: ScopeKey,
    options: AgentOptions,
}

impl Agent for RelayAgent {
    fn id(&self) -> &SessionId {
        self.session.id()
    }
    fn scope_key(&self) -> ScopeKey {
        self.scope.clone()
    }
    fn options(&self) -> &AgentOptions {
        &self.options
    }
    fn session(&self) -> &Session {
        &self.session
    }
    fn model(&self) -> &dyn Model {
        panic!("relay fixture has no model")
    }
    fn inbox(&self) -> &dyn Inbox {
        panic!("relay fixture has no inbox")
    }
    fn status(&self) -> AgentStatus {
        AgentStatus::Idle
    }
    fn ctx(&self) -> &Context {
        &self.ctx
    }
    fn cancel(&self, _: AgentCancelCause, _: CancelOptions) -> Result<(), BoxError> {
        Ok(())
    }
    fn when_idle(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance_erased(&self, _: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        panic!("not driven")
    }
    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
        panic!("not driven")
    }
    fn followup(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("not driven")
    }
    fn steer(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("not driven")
    }
    fn inject(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("not driven")
    }
}

fn relay_agent(env: &Env, session: Session) -> Arc<dyn Agent> {
    Arc::new(RelayAgent {
        session,
        ctx: env.ctx.clone(),
        scope: ScopeKey::new(),
        options: AgentOptions::default(),
    })
}

fn emit_agent_error(env: &Env, agent: Arc<dyn Agent>, error: BoxError) {
    env.ctx
        .dispatch::<AgentError>(Arc::new(AgentError {
            agent,
            turn: 3,
            step: 2,
            error: hoh_error::SharedError::new(error),
        }))
        .invoke(|failure| panic!("agent/error listener failed: {failure:?}"));
}

#[tokio::test]
async fn hands_every_appended_event_over_with_envelope_identity_and_cloned_body() {
    let env = setup(SessionTelemetryCapture::Live, false).await;
    let session = live(&env, "cap");
    let event = append_user_message(&session, "original");
    let mut records = env.backend.ledger();
    let record = records.first_mut().unwrap();
    assert_eq!(
        attr_str(record.attributes.get("session.id")).as_deref(),
        Some("cap")
    );
    assert_eq!(attr_u64(record.attributes.get("event.seq")), Some(0));
    assert_eq!(
        attr_u64(record.attributes.get("session.format_version")),
        Some(SESSION_FORMAT_VERSION)
    );
    assert_eq!(
        attr_str(record.attributes.get("event.type")).as_deref(),
        Some("user/message")
    );
    assert_eq!(record.time, event.time());
    assert_eq!(record.severity, SessionTelemetrySeverity::Info);
    let source = record.source_event.as_mut().unwrap();
    assert_eq!(source.session_id, *session.id());
    assert!(source.envelope.get("data").is_none());
    assert_eq!(
        source.envelope,
        json!({
            "type":"user/message",
            "seq":event.seq(),
            "time":event.time(),
            "surfaceOp":"append"
        })
    );
    record.body["content"][0]["text"] = json!("tampered");
    source.envelope["time"] = json!(0);
    assert_eq!(
        session
            .event_at(SessionSeq::new(0).unwrap())
            .unwrap()
            .data()["content"][0]["text"],
        "original"
    );
    assert_ne!(event.time(), 0);
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn captures_a_live_request_header_without_replaying_previously_withheld_events() {
    let env = setup(SessionTelemetryCapture::Live, false).await;
    let session = live(&env, "live-header");
    let rule = env
        .redactions
        .register(&env.ctx, |_record, _next| Err("withheld".into()))
        .unwrap();
    append(&session, "turn/start", json!({"turn":1}));
    rule.dispose().await;
    append(
        &session,
        "request/header",
        json!({"header":{"config":{"provider":"mock","model":"mock"}},"reason":"initial"}),
    );
    assert_eq!(event_types(&env.backend.ledger()), ["request/header"]);
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn stamps_header_facts_on_every_record_when_present() {
    let env = setup(SessionTelemetryCapture::Live, false).await;
    let session = env
        .sessions
        .create(
            &env.ctx,
            Some("child".into()),
            CreateSessionOptions {
                meta: Some(json!({"cwd":"/tmp/proj","parentSession":"parent"})),
                ..Default::default()
            },
        )
        .unwrap();
    append(&session, "turn/start", json!({"turn":1}));
    let record = env.backend.ledger().remove(0);
    assert_eq!(
        attr_str(record.attributes.get("session.cwd")).as_deref(),
        Some("/tmp/proj")
    );
    assert_eq!(
        attr_str(record.attributes.get("session.parent_id")).as_deref(),
        Some("parent")
    );
    assert_eq!(
        attr_u64(record.attributes.get("session.format_version")),
        Some(4)
    );
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn maps_outcome_flags_to_severity_unknown_types_falling_through_as_info() {
    let env = setup(SessionTelemetryCapture::Live, false).await;
    let session = live(&env, "severity");
    append(&session, "turn/start", json!({"turn":1}));
    append_surface(
        &session,
        "tool/result",
        json!({"turn":1,"step":1,"message":{"role":"tool","toolCallId":"c1","content":[],"isError":true}}),
    );
    append_surface(
        &session,
        "tool/result",
        json!({"turn":1,"step":1,"message":{"role":"tool","toolCallId":"c2","content":[],"isError":false}}),
    );
    append(&session, "telemetry-test/opaque", json!({"payload":{}}));
    append(
        &session,
        "turn/end",
        json!({"turn":1,"reason":{"kind":"error","error":{"message":"boom","code":"UNKNOWN"}}}),
    );
    let severities = env
        .backend
        .ledger()
        .into_iter()
        .map(|r| r.severity)
        .collect::<Vec<_>>();
    assert_eq!(
        severities,
        [
            SessionTelemetrySeverity::Info,
            SessionTelemetrySeverity::Error,
            SessionTelemetrySeverity::Info,
            SessionTelemetrySeverity::Info,
            SessionTelemetrySeverity::Error
        ]
    );
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn passes_unknown_merged_event_types_through_unchanged() {
    let env = setup(SessionTelemetryCapture::Live, false).await;
    let session = live(&env, "unknown");
    append(
        &session,
        "telemetry-test/opaque",
        json!({"payload":{"nested":["a","b"]}}),
    );
    let record = env.backend.ledger().remove(0);
    assert_eq!(record.body, json!({"payload":{"nested":["a","b"]}}));
    assert_eq!(record.severity, SessionTelemetrySeverity::Info);
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn ships_every_assistant_stream_in_canonical_order_with_its_complete_body() {
    let env = setup(SessionTelemetryCapture::Live, false).await;
    let a = live(&env, "a");
    let b = live(&env, "b");
    append_assistant_message(&a, 1, 1, &["a11-first", "a11-second"], 110);
    append_assistant_message(&a, 1, 2, &["a12-first"], 120);
    append_assistant_message(&b, 1, 1, &["b11-first", "b11-second"], 210);
    let records = env.backend.ledger();
    assert_eq!(
        records
            .iter()
            .map(|record| {
                json!([
                    attr_str(record.attributes.get("session.id")).unwrap(),
                    attr_u64(record.attributes.get("event.seq")).unwrap(),
                    record.body["stream"]
                ])
            })
            .collect::<Vec<_>>(),
        [
            json!(["a",0,[
                {"type":"text-chunks","time0":110,"index":0,"dt":[5],"texts":["a11-first","a11-second"]},
                {"type":"chunk","time":115,"chunk":{"type":"finish","reason":{"kind":"stop"}}}
            ]]),
            json!(["a",1,[
                {"type":"text-chunks","time0":120,"index":0,"dt":[],"texts":["a12-first"]},
                {"type":"chunk","time":120,"chunk":{"type":"finish","reason":{"kind":"stop"}}}
            ]]),
            json!(["b",0,[
                {"type":"text-chunks","time0":210,"index":0,"dt":[5],"texts":["b11-first","b11-second"]},
                {"type":"chunk","time":215,"chunk":{"type":"finish","reason":{"kind":"stop"}}}
            ]]),
        ]
    );
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn captures_one_canonical_log_prefix_at_a_time_without_following_later_events() {
    let env = setup(SessionTelemetryCapture::OnDemand, false).await;
    let session = live(&env, "prefix");
    let first = append(&session, "turn/start", json!({"turn":1}));
    append(&session, "telemetry-test/opaque", json!({"n":2}));
    env.coordinator.capture_session(&session, Some(first.seq()));
    append(
        &session,
        "turn/end",
        json!({"turn":1,"reason":{"kind":"completed"}}),
    );
    assert_eq!(seqs(&env.backend.ledger()), [0]);
    env.coordinator.capture_session(&session, None);
    env.coordinator.capture_session(&session, None);
    assert_eq!(seqs(&env.backend.ledger()), [0, 1, 2]);
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn includes_inherited_history_only_when_explicitly_requested_through_the_exact_sequence() {
    let env = setup(SessionTelemetryCapture::OnDemand, true).await;
    let parent = live(&env, "history-parent");
    append(&parent, "turn/start", json!({"turn":1}));
    append_user_message(&parent, "hello");
    append(
        &parent,
        "turn/end",
        json!({"turn":1,"reason":{"kind":"completed"}}),
    );
    let child = env
        .sessions
        .fork(
            &env.ctx,
            SessionForkSource::Session(parent),
            None,
            Some("history-child".into()),
        )
        .unwrap();
    let boundary = child.snapshot_events(None, None).last().unwrap().clone();
    append(&child, "turn/start", json!({"turn":2}));
    env.coordinator
        .capture_session(&child, Some(boundary.seq()));
    env.coordinator
        .capture_session(&child, Some(boundary.seq()));
    let records = env
        .backend
        .ledger()
        .into_iter()
        .filter(|record| {
            attr_str(record.attributes.get("session.id")).as_deref() == Some("history-child")
        })
        .collect::<Vec<_>>();
    assert_eq!(seqs(&records), [0, 1, 2, 3]);
    assert_eq!(seqs(&records).last(), Some(&u64::from(boundary.seq())));
    env.kernel.shutdown().await;
}

async fn seeded_history_case(capture: SessionTelemetryCapture, include_history: bool) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let sessions = (*ctx.get::<Sessions>().unwrap()).clone();
    let parent = sessions
        .create(
            &ctx,
            Some("cold-parent".into()),
            CreateSessionOptions::default(),
        )
        .unwrap();
    append(&parent, "turn/start", json!({"turn":1}));
    append_user_message(&parent, "first");
    append(
        &parent,
        "turn/end",
        json!({"turn":1,"reason":{"kind":"completed"}}),
    );
    let child = sessions
        .fork(
            &ctx,
            SessionForkSource::Session(parent),
            None,
            Some("cold-child".into()),
        )
        .unwrap();
    append(&child, "turn/start", json!({"turn":2}));
    append(
        &child,
        "turn/end",
        json!({"turn":2,"reason":{"kind":"completed"}}),
    );
    let resumed_id = SessionId::from(format!(
        "restored-{}-{}",
        match capture {
            SessionTelemetryCapture::Live => "live",
            SessionTelemetryCapture::OnDemand => "on-demand",
        },
        include_history
    ));
    let mut header = child.header().as_ref().clone();
    header["id"] = json!(resumed_id);
    let session = Session::from_restore(
        resumed_id.clone(),
        &child.snapshot_events(None, None),
        header,
        child.inherited_event_count(),
        SessionSeedEventState::SharedFrozen,
        None,
    )
    .unwrap();
    let backend = Arc::new(FakeSink::default());
    let coordinator = SessionTelemetryCoordinator::new(
        ctx.clone(),
        backend.clone(),
        SessionTelemetryCaptureOptions {
            capture,
            include_history,
        },
    )
    .unwrap();
    let _detach = sessions.enter(&ctx, session.clone()).unwrap();
    sessions.announce(&session).unwrap();
    let start = if include_history {
        SessionLogOffset::default()
    } else {
        session.first_lifecycle_seq()
    };
    let expected = session.snapshot_events(Some(start), None);
    let captured = || {
        backend
            .ledger()
            .into_iter()
            .filter(|record| {
                attr_str(record.attributes.get("session.id")).as_deref()
                    == Some(resumed_id.as_ref())
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        seqs(&captured()),
        if capture == SessionTelemetryCapture::Live {
            expected
                .iter()
                .map(|event| u64::from(event.seq()))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        }
    );
    coordinator.capture_session(&session, None);
    coordinator.capture_session(&session, None);
    assert_eq!(
        seqs(&captured()),
        expected
            .iter()
            .map(|event| u64::from(event.seq()))
            .collect::<Vec<_>>()
    );
    assert_eq!(expected.last().unwrap().event_type(), "session/end-seed");
    kernel.shutdown().await;
}

#[tokio::test]
async fn restores_a_seeded_session_with_live_capture_and_include_history_false() {
    seeded_history_case(SessionTelemetryCapture::Live, false).await;
}
#[tokio::test]
async fn restores_a_seeded_session_with_on_demand_capture_and_include_history_false() {
    seeded_history_case(SessionTelemetryCapture::OnDemand, false).await;
}
#[tokio::test]
async fn restores_a_seeded_session_with_live_capture_and_include_history_true() {
    seeded_history_case(SessionTelemetryCapture::Live, true).await;
}
#[tokio::test]
async fn restores_a_seeded_session_with_on_demand_capture_and_include_history_true() {
    seeded_history_case(SessionTelemetryCapture::OnDemand, true).await;
}

#[tokio::test]
async fn runs_the_currently_mounted_redaction_policy_during_canonical_log_capture() {
    let env = setup(SessionTelemetryCapture::OnDemand, false).await;
    let session = live(&env, "redact-late");
    append(&session, "telemetry-test/opaque", json!({"secret":"value"}));
    let rule = env
        .redactions
        .register(&env.ctx, |_record, next| {
            let mut record = next.run()?;
            record.body = Value::Null;
            Ok(record)
        })
        .unwrap();
    env.coordinator.capture_session(&session, None);
    assert_eq!(env.backend.ledger()[0].body, Value::Null);
    rule.dispose().await;
    append(
        &session,
        "turn/end",
        json!({"turn":1,"reason":{"kind":"completed"}}),
    );
    env.coordinator.capture_session(&session, None);
    assert_eq!(
        env.backend.ledger()[1].body,
        json!({"turn":1,"reason":{"kind":"completed"}})
    );
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn contains_each_backend_failure_independently_while_replaying_a_prefix() {
    let env = setup(SessionTelemetryCapture::OnDemand, false).await;
    let session = live(&env, "partial");
    for n in 0..3 {
        append(&session, "telemetry-test/opaque", json!({"n":n}));
    }
    *lock(&env.backend.reject_seq) = Some(1);
    env.coordinator.capture_session(&session, None);
    assert_eq!(seqs(&env.backend.ledger()), [0, 2]);
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn captures_a_pending_prefix_after_coordinator_reload_without_retained_records() {
    let env = setup(SessionTelemetryCapture::OnDemand, false).await;
    let session = live(&env, "reload-on-demand");
    append(&session, "telemetry-test/opaque", json!({"n":1}));
    let second = Arc::new(FakeSink::default());
    let reloaded = SessionTelemetryCoordinator::new(
        env.ctx.clone(),
        second.clone(),
        SessionTelemetryCaptureOptions {
            capture: SessionTelemetryCapture::OnDemand,
            include_history: false,
        },
    )
    .unwrap();
    reloaded.capture_session(&session, None);
    assert_eq!(seqs(&second.ledger()), [0]);
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn registers_no_continuous_capture_flush_or_ops_listeners() {
    let env = setup(SessionTelemetryCapture::OnDemand, false).await;
    let redaction_calls = Arc::new(AtomicUsize::new(0));
    let calls = redaction_calls.clone();
    env.redactions
        .register(&env.ctx, move |_record, next| {
            calls.fetch_add(1, Ordering::SeqCst);
            next.run()
        })
        .unwrap();
    let session = live(&env, "on-demand");
    append(&session, "telemetry-test/opaque", json!({"n":1}));
    env.sessions.flush(&session).await.unwrap();
    emit_agent_error(
        &env,
        relay_agent(&env, session.clone()),
        "local only".into(),
    );
    assert!(env.backend.records().is_empty());
    assert_eq!(env.backend.flushes.load(Ordering::SeqCst), 0);
    assert_eq!(redaction_calls.load(Ordering::SeqCst), 0);
    env.coordinator.capture_session(&session, None);
    assert_eq!(env.backend.ledger().len(), 1);
    assert_eq!(redaction_calls.load(Ordering::SeqCst), 1);
    let backend = env.backend.clone();
    env.kernel.shutdown().await;
    assert_eq!(
        backend
            .records()
            .iter()
            .map(|record| record.channel)
            .collect::<Vec<_>>(),
        [SessionTelemetryChannel::Ledger]
    );
}

async fn fork_marker_case(capture: SessionTelemetryCapture) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let sessions = (*ctx.get::<Sessions>().unwrap()).clone();
    let first = Arc::new(FakeSink::default());
    let (fiber, coordinator) = mount_coordinator(
        &ctx,
        first.clone(),
        SessionTelemetryCaptureOptions {
            capture,
            include_history: false,
        },
    )
    .await;
    let parent = sessions
        .create(
            &ctx,
            Some("fork-parent".into()),
            CreateSessionOptions::default(),
        )
        .unwrap();
    append(&parent, "turn/start", json!({"turn":1}));
    append_user_message(&parent, "hello");
    append_open_tool_request(&parent, "fork-call");
    let child = sessions
        .fork(
            &ctx,
            SessionForkSource::Session(parent),
            None,
            Some("fork-child".into()),
        )
        .unwrap();
    let own = child.own_events();
    assert_eq!(
        own.iter()
            .map(|event| event.event_type())
            .collect::<Vec<_>>(),
        ["session/end-seed", "tool/result", "step/end", "turn/end"]
    );
    assert_eq!(
        u64::from(child.first_live_seq()),
        u64::from(child.inherited_event_count()) + own.len() as u64
    );
    let captured = || {
        first
            .ledger()
            .into_iter()
            .filter(|record| {
                attr_str(record.attributes.get("session.id")).as_deref() == Some("fork-child")
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        seqs(&captured()),
        if capture == SessionTelemetryCapture::Live {
            own.iter()
                .map(|event| u64::from(event.seq()))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        }
    );
    coordinator.capture_session(&child, None);
    coordinator.capture_session(&child, None);
    assert_eq!(
        seqs(&captured()),
        own.iter()
            .map(|event| u64::from(event.seq()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        captured()
            .iter()
            .map(|record| record.body.clone())
            .collect::<Vec<_>>(),
        own.iter()
            .map(|event| event.data().clone())
            .collect::<Vec<_>>()
    );

    fiber.dispose().await;
    let second = Arc::new(FakeSink::default());
    let (_reloaded_fiber, reloaded) = mount_coordinator(
        &ctx,
        second.clone(),
        SessionTelemetryCaptureOptions {
            capture,
            include_history: false,
        },
    )
    .await;
    reloaded.capture_session(&child, None);
    assert!(second.ledger().is_empty());
    let next = append(&child, "turn/start", json!({"turn":2}));
    reloaded.capture_session(&child, None);
    assert_eq!(seqs(&second.ledger()), [u64::from(next.seq())]);
    kernel.shutdown().await;
}

#[tokio::test]
async fn captures_the_child_owned_fork_marker_and_closers_in_live_mode() {
    fork_marker_case(SessionTelemetryCapture::Live).await;
}
#[tokio::test]
async fn captures_the_child_owned_fork_marker_and_closers_in_on_demand_mode() {
    fork_marker_case(SessionTelemetryCapture::OnDemand).await;
}

#[tokio::test]
async fn replays_a_new_fork_object_from_its_constructor_boundary_without_its_inherited_prefix() {
    let env = setup(SessionTelemetryCapture::Live, false).await;
    let parent = live(&env, "seed-parent");
    append(&parent, "turn/start", json!({"turn":1}));
    append_user_message(&parent, "hello");
    let child = env
        .sessions
        .prepare(
            Some("seeded".into()),
            hoh_session::PrepareSessionOptions::Create(CreateSessionOptions {
                seed: Some(parent.snapshot_events(None, None).to_vec()),
                ..Default::default()
            }),
        )
        .unwrap();
    let _detach = env.sessions.enter(&env.ctx, child.clone()).unwrap();
    env.sessions.announce(&child).unwrap();
    append(
        &child,
        "turn/end",
        json!({"turn":1,"reason":{"kind":"completed"}}),
    );
    let records = env.backend.ledger();
    assert!(records.iter().any(|record| {
        attr_str(record.attributes.get("session.id")).as_deref() == Some("seed-parent")
            && attr_u64(record.attributes.get("event.seq")) == Some(0)
    }));
    assert!(records.iter().any(|record| {
        attr_str(record.attributes.get("session.id")).as_deref() == Some("seed-parent")
            && attr_u64(record.attributes.get("event.seq")) == Some(1)
    }));
    let child_records = records
        .into_iter()
        .filter(|record| attr_str(record.attributes.get("session.id")).as_deref() == Some("seeded"))
        .collect::<Vec<_>>();
    assert_eq!(seqs(&child_records), [2, 3]);
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn replays_a_restored_post_migration_session_from_its_constructor_boundary() {
    let env = setup(SessionTelemetryCapture::Live, false).await;
    let donor = Session::create("donor".into(), None, None, None, None).unwrap();
    append(&donor, "turn/start", json!({"turn":1}));
    append_assistant_message(&donor, 1, 1, &["first"], 100);
    let resumed = Session::from_restore(
        "resumed".into(),
        &donor.snapshot_events(None, None),
        json!({
            "version":SESSION_FORMAT_VERSION,
            "id":"resumed",
            "createdAt":1,
            "isSeeded":false
        }),
        SessionLogOffset::default(),
        SessionSeedEventState::Detached,
        None,
    )
    .unwrap();
    let _detach = env.sessions.enter(&env.ctx, resumed.clone()).unwrap();
    env.sessions.announce(&resumed).unwrap();
    let captured = || {
        env.backend
            .ledger()
            .into_iter()
            .filter(|record| {
                attr_str(record.attributes.get("session.id")).as_deref() == Some("resumed")
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(seqs(&captured()), [2]);
    assert!(captured().iter().all(|record| {
        attr_u64(record.attributes.get("session.format_version")) == Some(SESSION_FORMAT_VERSION)
    }));
    append_assistant_message(&resumed, 1, 1, &["continuation"], 200);
    append_assistant_message(&resumed, 1, 2, &["next step"], 300);
    let records = captured();
    assert_eq!(seqs(&records), [2, 3, 4]);
    assert_eq!(
        records
            .iter()
            .map(|record| record.body["stream"][0]["texts"].clone())
            .collect::<Vec<_>>(),
        [Value::Null, json!(["continuation"]), json!(["next step"])]
    );
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn stamps_session_seed_length_from_the_exact_session_cut() {
    let env = setup(SessionTelemetryCapture::Live, false).await;
    let parent = live(&env, "stitch-parent");
    append(&parent, "turn/start", json!({"turn":1}));
    let child = env
        .sessions
        .create(
            &env.ctx,
            Some("stitch-child".into()),
            CreateSessionOptions {
                seed: Some(parent.snapshot_events(None, None).to_vec()),
                inherited_event_count: Some(parent.seq()),
                meta: Some(json!({"isSeeded":true,"parentSession":"stitch-parent"})),
            },
        )
        .unwrap();
    append(
        &child,
        "turn/end",
        json!({"turn":1,"reason":{"kind":"completed"}}),
    );
    let record = env
        .backend
        .ledger()
        .into_iter()
        .find(|r| attr_str(r.attributes.get("session.id")).as_deref() == Some("stitch-child"))
        .unwrap();
    assert_eq!(
        attr_u64(record.attributes.get("session.seed_length")),
        Some(1)
    );
    env.kernel.shutdown().await;
}

async fn double_adoption_case(include_history: bool) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let sessions = (*ctx.get::<Sessions>().unwrap()).clone();
    let session = Session::create(
        if include_history {
            "double-yes"
        } else {
            "double-no"
        }
        .into(),
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let detach = sessions.enter(&ctx, session.clone()).unwrap();
    let backend = Arc::new(FakeSink::default());
    let _coordinator = SessionTelemetryCoordinator::new(
        ctx.clone(),
        backend.clone(),
        SessionTelemetryCaptureOptions {
            capture: SessionTelemetryCapture::Live,
            include_history,
        },
    )
    .unwrap();
    let before = backend.ledger().len();
    sessions.announce(&session).unwrap();
    assert_eq!(backend.ledger().len(), before);
    append(&session, "telemetry-test/opaque", json!({"n":1}));
    assert_eq!(backend.ledger().len(), before + 1);
    detach.detach();
    kernel.shutdown().await;
}

#[tokio::test]
async fn adopts_exactly_once_when_created_fires_after_the_sweep_without_history() {
    double_adoption_case(false).await;
}
#[tokio::test]
async fn adopts_exactly_once_when_created_fires_after_the_sweep_with_history() {
    double_adoption_case(true).await;
}

#[tokio::test]
async fn resumes_from_the_handoff_cursor_across_same_object_readoption_without_duplicates() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let sessions = (*ctx.get::<Sessions>().unwrap()).clone();
    let first = Arc::new(FakeSink::default());
    let first_fiber = mount_live_coordinator(&ctx, first.clone()).await;
    let session = sessions
        .create(&ctx, Some("hmr".into()), CreateSessionOptions::default())
        .unwrap();
    append(&session, "telemetry-test/opaque", json!({"n":1}));
    append(&session, "telemetry-test/opaque", json!({"n":2}));
    assert_eq!(seqs(&first.ledger()), [0, 1]);
    first_fiber.dispose().await;
    append(&session, "telemetry-test/opaque", json!({"n":2}));
    append(&session, "telemetry-test/opaque", json!({"n":3}));
    let second = Arc::new(FakeSink::default());
    let _second_fiber = mount_live_coordinator(&ctx, second.clone()).await;
    assert_eq!(seqs(&second.ledger()), [2, 3]);
    kernel.shutdown().await;
}

#[tokio::test]
async fn disposal_does_not_erase_the_process_handoff_cursor_seen_by_a_new_coordinator() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let sessions = (*ctx.get::<Sessions>().unwrap()).clone();
    let first = Arc::new(FakeSink::default());
    let _first = SessionTelemetryCoordinator::new(
        ctx.clone(),
        first.clone(),
        SessionTelemetryCaptureOptions::default(),
    )
    .unwrap();
    let session = sessions
        .prepare(
            Some("disposed-handoff".into()),
            hoh_session::PrepareSessionOptions::default(),
        )
        .unwrap();
    let detach = sessions.enter(&ctx, session.clone()).unwrap();
    sessions.announce(&session).unwrap();
    append(&session, "telemetry-test/opaque", json!({"n":1}));
    assert_eq!(seqs(&first.ledger()), [0]);

    let second = Arc::new(FakeSink::default());
    let reloaded = SessionTelemetryCoordinator::new(
        ctx.clone(),
        second.clone(),
        SessionTelemetryCaptureOptions {
            capture: SessionTelemetryCapture::OnDemand,
            include_history: false,
        },
    )
    .unwrap();
    detach.detach();
    reloaded.capture_session(&session, None);
    assert!(second.ledger().is_empty());
    kernel.shutdown().await;
}

#[tokio::test]
async fn replays_past_a_record_the_backend_rejects_one_event_withheld_the_rest_adopted() {
    let env = setup(SessionTelemetryCapture::OnDemand, false).await;
    let session = live(&env, "partial-adoption");
    for n in 0..3 {
        append(&session, "telemetry-test/opaque", json!({"n":n}));
    }
    *lock(&env.backend.reject_seq) = Some(1);
    env.coordinator.capture_session(&session, None);
    assert_eq!(seqs(&env.backend.ledger()), [0, 2]);
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn rehands_the_full_log_when_no_cursor_survived_for_a_fresh_session_object() {
    let env = setup(SessionTelemetryCapture::OnDemand, false).await;
    let session = live(&env, "fresh");
    append(&session, "telemetry-test/opaque", json!({"n":1}));
    append(&session, "telemetry-test/opaque", json!({"n":2}));
    env.coordinator.capture_session(&session, None);
    assert_eq!(seqs(&env.backend.ledger()), [0, 1]);
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn forwards_session_flush_as_a_hint_without_awaiting_backend_work() {
    let env = setup(SessionTelemetryCapture::Live, false).await;
    let session = live(&env, "flush");
    env.sessions.flush(&session).await.unwrap();
    assert_eq!(env.backend.flushes.load(Ordering::SeqCst), 1);
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn ignores_flush_hints_for_sessions_it_never_adopted() {
    let env = setup(SessionTelemetryCapture::Live, false).await;
    let stranger = Session::create("stranger".into(), None, None, None, None).unwrap();
    let detach = env.sessions.enter(&env.ctx, stranger.clone()).unwrap();
    env.sessions.flush(&stranger).await.unwrap();
    assert_eq!(env.backend.flushes.load(Ordering::SeqCst), 0);
    detach.detach();
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn emits_no_marker_for_a_session_whose_announcement_was_vetoed_before_adoption() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let sessions = (*ctx.get::<Sessions>().unwrap()).clone();
    SessionStore::on_created(&ctx, |_| Err("vetoed by an earlier listener".into())).unwrap();
    let backend = Arc::new(FakeSink::default());
    let _coordinator = SessionTelemetryCoordinator::new(
        ctx.clone(),
        backend.clone(),
        SessionTelemetryCaptureOptions::default(),
    )
    .unwrap();
    assert!(
        sessions
            .create(&ctx, Some("vetoed".into()), CreateSessionOptions::default())
            .is_err()
    );
    assert!(
        backend
            .records()
            .iter()
            .all(|record| record.channel != SessionTelemetryChannel::Ops)
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn emits_each_adopted_sessions_shutdown_record_before_awaiting_backend_shutdown() {
    let env = setup(SessionTelemetryCapture::Live, false).await;
    live(&env, "s1");
    live(&env, "s2");
    let backend = env.backend.clone();
    env.kernel.shutdown().await;
    let ops = backend
        .records()
        .into_iter()
        .filter(|r| r.channel == SessionTelemetryChannel::Ops)
        .collect::<Vec<_>>();
    assert_eq!(ops.len(), 2);
    let mut session_ids = ops
        .iter()
        .map(|record| attr_str(record.attributes.get("session.id")).unwrap())
        .collect::<Vec<_>>();
    session_ids.sort();
    assert_eq!(session_ids, ["s1", "s2"]);
    assert!(ops.iter().all(|record| {
        attr_str(record.attributes.get("telemetry.op")).as_deref() == Some("shutdown")
            && record.severity == SessionTelemetrySeverity::Info
            && !record.attributes.contains_key("event.seq")
            && !record.attributes.contains_key("event.type")
    }));
    assert_eq!(
        *lock(&backend.calls),
        ["emit:shutdown", "emit:shutdown", "shutdown"]
    );
    assert!(backend.shutdown_resolved.load(Ordering::SeqCst));
}

#[tokio::test]
async fn emits_the_shutdown_marker_at_the_sessions_own_disposal_edge_then_retires_it() {
    let env = setup(SessionTelemetryCapture::Live, false).await;
    live(&env, "survivor");
    let session = Session::create("ephemeral".into(), None, None, None, None).unwrap();
    let detach = env.sessions.enter(&env.ctx, session.clone()).unwrap();
    env.sessions.announce(&session).unwrap();
    detach.detach();
    let at_edge = env
        .backend
        .records()
        .into_iter()
        .filter(|record| record.channel == SessionTelemetryChannel::Ops)
        .collect::<Vec<_>>();
    assert_eq!(at_edge.len(), 1);
    assert_eq!(
        attr_str(at_edge[0].attributes.get("session.id")).as_deref(),
        Some("ephemeral")
    );
    assert_eq!(
        attr_str(at_edge[0].attributes.get("telemetry.op")).as_deref(),
        Some("shutdown")
    );
    let backend = env.backend.clone();
    env.kernel.shutdown().await;
    assert_eq!(
        backend
            .records()
            .iter()
            .filter(|r| r.channel == SessionTelemetryChannel::Ops)
            .map(|record| attr_str(record.attributes.get("session.id")).unwrap())
            .collect::<Vec<_>>(),
        ["ephemeral", "survivor"]
    );
}

#[tokio::test]
async fn warns_instead_of_throwing_when_backend_shutdown_fails() {
    let env = setup(SessionTelemetryCapture::Live, false).await;
    env.backend.fail_shutdown.store(true, Ordering::SeqCst);
    let logger = env.ctx.logger();
    env.kernel.shutdown().await;
    assert!(!env.backend.shutdown_resolved.load(Ordering::SeqCst));
    assert!(logger.buffer().iter().any(|record| {
        record.message == "telemetry: backend shutdown failed: Error: exporter unreachable"
    }));
}

struct EmptyShutdownErrorSink;

impl SessionTelemetrySink for EmptyShutdownErrorSink {
    fn emit(&self, _: SessionTelemetryRecord) -> Result<(), BoxError> {
        Ok(())
    }

    fn shutdown(&self) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async { Err("".into()) })
    }
}

#[tokio::test]
async fn formats_an_empty_error_like_javascript_without_a_trailing_separator() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    SessionTelemetryCoordinator::new(
        ctx.clone(),
        Arc::new(EmptyShutdownErrorSink),
        SessionTelemetryCaptureOptions::default(),
    )
    .unwrap();
    let logger = ctx.logger();
    kernel.shutdown().await;
    assert!(
        logger
            .buffer()
            .iter()
            .any(|record| { record.message == "telemetry: backend shutdown failed: Error" })
    );
}

#[tokio::test]
async fn contains_emit_failures_the_append_succeeds_and_capture_heals() {
    let env = setup(SessionTelemetryCapture::Live, false).await;
    let session = live(&env, "heal");
    let logger = env.ctx.logger();
    env.backend.fail_emit.store(true, Ordering::SeqCst);
    append(&session, "turn/start", json!({"turn":1}));
    env.backend.fail_emit.store(false, Ordering::SeqCst);
    append(
        &session,
        "turn/end",
        json!({"turn":1,"reason":{"kind":"completed"}}),
    );
    assert_eq!(event_types(&env.backend.ledger()), ["turn/end"]);
    assert!(logger.buffer().iter().any(|record| {
        record.message == "telemetry: capture step failed: Error: backend broke"
    }));
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn relays_agent_error_error_values_as_an_ops_record_with_normalized_identity() {
    let env = setup(SessionTelemetryCapture::Live, false).await;
    let session = live(&env, "erring");
    emit_agent_error(
        &env,
        relay_agent(&env, session),
        Box::new(hoh_error::HarnessError::named(
            "TypeError",
            "adapter exploded",
            "TYPE",
        )),
    );
    let record = env
        .backend
        .records()
        .into_iter()
        .find(|record| record.channel == SessionTelemetryChannel::Ops)
        .unwrap();
    assert_eq!(record.severity, SessionTelemetrySeverity::Error);
    assert_eq!(
        attr_str(record.attributes.get("session.id")).as_deref(),
        Some("erring")
    );
    assert_eq!(
        attr_str(record.attributes.get("agent.id")).as_deref(),
        Some("erring")
    );
    assert_eq!(
        attr_str(record.attributes.get("error.name")).as_deref(),
        Some("TypeError")
    );
    assert_eq!(attr_u64(record.attributes.get("turn")), Some(3));
    assert_eq!(attr_u64(record.attributes.get("step")), Some(2));
    assert_eq!(
        record.body,
        json!({"name":"TypeError","message":"adapter exploded"})
    );
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn relays_agent_error_non_error_values_as_an_ops_record_with_normalized_identity() {
    let env = setup(SessionTelemetryCapture::Live, false).await;
    let session = live(&env, "plain-error");
    emit_agent_error(&env, relay_agent(&env, session), "plain failure".into());
    let record = env
        .backend
        .records()
        .into_iter()
        .find(|record| record.channel == SessionTelemetryChannel::Ops)
        .unwrap();
    assert_eq!(
        attr_str(record.attributes.get("error.name")).as_deref(),
        Some("Error")
    );
    assert_eq!(
        record.body,
        json!({"name":"Error","message":"plain failure"})
    );
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn passes_records_through_unchanged_when_no_listener_is_mounted() {
    let env = setup(SessionTelemetryCapture::Live, false).await;
    let session = live(&env, "plain");
    append(
        &session,
        "telemetry-test/opaque",
        json!({"secret":"visible"}),
    );
    assert_eq!(env.backend.ledger()[0].body, json!({"secret":"visible"}));
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn applies_a_mounted_rule_to_every_outbound_record_ops_records_included() {
    let env = setup(SessionTelemetryCapture::Live, false).await;
    env.redactions
        .register(&env.ctx, |_input, next| {
            let mut record = next.run()?;
            record.body = json!({"scrubbed":true});
            Ok(record)
        })
        .unwrap();
    live(&env, "rules");
    let session = env.sessions.get(&SessionId::from("rules")).unwrap();
    append(&session, "telemetry-test/opaque", json!({"secret":"x"}));
    env.kernel.shutdown().await;
    let records = env.backend.records();
    assert!(
        records
            .iter()
            .all(|record| record.body == json!({"scrubbed":true}))
    );
    let ops = records
        .iter()
        .filter(|record| record.channel == SessionTelemetryChannel::Ops)
        .collect::<Vec<_>>();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].body, json!({"scrubbed":true}));
}

#[tokio::test]
async fn keeps_the_canonical_log_untouched_by_a_mounted_rule() {
    let env = setup(SessionTelemetryCapture::Live, false).await;
    env.redactions
        .register(&env.ctx, |_input, next| {
            let mut record = next.run()?;
            record.body = Value::Null;
            Ok(record)
        })
        .unwrap();
    let session = live(&env, "canonical");
    append(&session, "telemetry-test/opaque", json!({"secret":"kept"}));
    assert_eq!(
        session
            .event_at(SessionSeq::new(0).unwrap())
            .unwrap()
            .data(),
        &json!({"secret":"kept"})
    );
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn stacks_listeners_outermost_first_around_next() {
    let env = setup(SessionTelemetryCapture::Live, false).await;
    let order = Arc::new(Mutex::new(Vec::new()));
    let first = order.clone();
    env.redactions
        .register(&env.ctx, move |_input, next| {
            lock(&first).push("outer-before");
            let record = next.run()?;
            lock(&first).push("outer-after");
            Ok(record)
        })
        .unwrap();
    let second = order.clone();
    env.redactions
        .register(&env.ctx, move |_input, next| {
            lock(&second).push("inner");
            next.run()
        })
        .unwrap();
    let session = live(&env, "stack");
    append(&session, "telemetry-test/opaque", json!({}));
    assert_eq!(*lock(&order), ["outer-before", "inner", "outer-after"]);
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn a_listener_that_skips_next_replaces_everything_beneath_it() {
    let env = setup(SessionTelemetryCapture::Live, false).await;
    env.redactions
        .register(&env.ctx, |_input, _next| {
            Ok(SessionTelemetryRecord {
                source_event: None,
                channel: SessionTelemetryChannel::Ops,
                time: 0,
                severity: SessionTelemetrySeverity::Info,
                attributes: Default::default(),
                body: json!("replaced"),
            })
        })
        .unwrap();
    let called = Arc::new(AtomicBool::new(false));
    let probe = called.clone();
    env.redactions
        .register(&env.ctx, move |_input, next| {
            probe.store(true, Ordering::SeqCst);
            next.run()
        })
        .unwrap();
    let session = live(&env, "replace");
    append(&session, "telemetry-test/opaque", json!({}));
    assert_eq!(env.backend.records()[0].body, json!("replaced"));
    assert!(!called.load(Ordering::SeqCst));
    env.kernel.shutdown().await;
}

#[tokio::test]
async fn a_throwing_rule_withholds_the_record_fail_closed_without_disturbing_the_log() {
    let env = setup(SessionTelemetryCapture::Live, false).await;
    env.redactions
        .register(&env.ctx, |_input, _next| Err("rule exploded".into()))
        .unwrap();
    let session = live(&env, "closed");
    append(&session, "telemetry-test/opaque", json!({"kept":true}));
    assert!(env.backend.records().is_empty());
    assert_eq!(session.snapshot_events(None, None).len(), 1);
    env.kernel.shutdown().await;
}
