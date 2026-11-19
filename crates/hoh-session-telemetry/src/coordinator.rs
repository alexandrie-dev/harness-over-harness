use crate::{
    SessionTelemetryAttribute, SessionTelemetryChannel, SessionTelemetryRecord,
    SessionTelemetryRedactions, SessionTelemetrySeverity, SessionTelemetrySink,
    SessionTelemetrySourceEvent,
};
use hoh_agent::AgentError;
use hoh_error::{BoxError, HarnessError};
use hoh_kernel::{Context, Undo};
use hoh_session::{
    Session, SessionDisposed, SessionEvent, SessionEventEmitted, SessionFlush, SessionLogOffset,
    SessionSeq, SessionStore, Sessions,
};
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex, MutexGuard, OnceLock, Weak},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionTelemetryCapture {
    #[default]
    Live,
    OnDemand,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionTelemetryCaptureOptions {
    pub capture: SessionTelemetryCapture,
    pub include_history: bool,
}

struct CoordinatorInner {
    ctx: Context,
    backend: Arc<dyn SessionTelemetrySink>,
    redactions: SessionTelemetryRedactions,
    options: SessionTelemetryCaptureOptions,
    adopted: Mutex<Vec<Session>>,
}

/// Capture coordinator for a single backend fiber.
#[derive(Clone)]
pub struct SessionTelemetryCoordinator(Arc<CoordinatorInner>);

impl SessionTelemetryCoordinator {
    pub fn new(
        ctx: Context,
        backend: Arc<dyn SessionTelemetrySink>,
        options: SessionTelemetryCaptureOptions,
    ) -> Result<Self, BoxError> {
        Self::new_with_redactions(ctx, backend, options, SessionTelemetryRedactions::new())
    }

    pub fn new_with_redactions(
        ctx: Context,
        backend: Arc<dyn SessionTelemetrySink>,
        options: SessionTelemetryCaptureOptions,
        redactions: SessionTelemetryRedactions,
    ) -> Result<Self, BoxError> {
        let this = Self(Arc::new(CoordinatorInner {
            ctx: ctx.clone(),
            backend,
            redactions,
            options,
            adopted: Mutex::new(Vec::new()),
        }));
        if options.capture == SessionTelemetryCapture::Live {
            this.install_live_listeners()?;
            let sessions = ctx
                .get::<Sessions>()
                .ok_or_else(|| BoxError::from("session telemetry requires the sessions service"))?;
            for session in sessions.list() {
                this.adopt(session);
            }
        }
        let dispose = this.clone();
        ctx.effect("telemetry capture", move || {
            Undo::future(move || async move {
                if dispose.0.options.capture == SessionTelemetryCapture::Live {
                    let sessions = lock(&dispose.0.adopted).clone();
                    for session in sessions {
                        dispose
                            .contain(|| dispose.deliver(&session, shutdown_record(&session), None));
                    }
                }
                if let Err(error) = dispose.0.backend.shutdown().await {
                    dispose.0.ctx.logger().warn(format_args!(
                        "telemetry: backend shutdown failed: {}",
                        js_error(&*error)
                    ));
                }
                Ok(())
            })
        })?;
        Ok(this)
    }

    fn install_live_listeners(&self) -> Result<(), BoxError> {
        let this = self.clone();
        SessionStore::on_created(&self.0.ctx, move |session| {
            this.adopt(session.clone());
            Ok(())
        })?;
        let this = self.clone();
        self.0.ctx.on_contained::<SessionDisposed>(move |event| {
            let removed = {
                let mut adopted = lock(&this.0.adopted);
                adopted
                    .iter()
                    .position(|session| session == &event.session)
                    .map(|index| adopted.remove(index))
                    .is_some()
            };
            if removed {
                this.contain(|| {
                    this.deliver(&event.session, shutdown_record(&event.session), None)
                });
            }
            Ok(())
        })?;
        let this = self.clone();
        self.0
            .ctx
            .on_contained::<SessionEventEmitted>(move |event| {
                this.contain(|| this.capture_event(&event.session, &event.event));
                Ok(())
            })?;
        let this = self.clone();
        self.0.ctx.on_contained::<SessionFlush>(move |event| {
            if lock(&this.0.adopted)
                .iter()
                .any(|session| session == &event.session)
            {
                this.contain(|| this.0.backend.flush());
            }
            Ok(())
        })?;
        let this = self.clone();
        self.0.ctx.on_contained::<AgentError>(move |event| {
            this.contain(|| this.relay_agent_error(event));
            Ok(())
        })?;
        Ok(())
    }

    pub fn capture_session(&self, session: &Session, through_seq: Option<SessionSeq>) {
        let start = cursor_of(session).map_or_else(
            || {
                let lifecycle = u64::from(session.first_lifecycle_seq());
                if self.0.options.include_history || lifecycle == 0 {
                    0
                } else {
                    lifecycle
                }
            },
            |cursor| u64::from(cursor).saturating_add(1),
        );
        let Ok(offset) = SessionLogOffset::new(start) else {
            return;
        };
        for event in session.snapshot_events(Some(offset), None).iter() {
            if through_seq.is_some_and(|through| event.seq() > through) {
                break;
            }
            self.contain(|| self.capture_event(session, event));
        }
    }

    fn adopt(&self, session: Session) {
        {
            let mut adopted = lock(&self.0.adopted);
            if adopted.iter().any(|entry| entry == &session) {
                return;
            }
            adopted.push(session.clone());
        }
        self.capture_session(&session, None);
    }

    fn capture_event(&self, session: &Session, event: &SessionEvent) -> Result<(), BoxError> {
        let mut envelope = event.as_value().clone();
        if let Value::Object(object) = &mut envelope {
            object.shift_remove("data");
        }
        let record = SessionTelemetryRecord {
            source_event: Some(SessionTelemetrySourceEvent {
                session_id: session.id().clone(),
                envelope,
            }),
            channel: SessionTelemetryChannel::Ledger,
            time: event.time(),
            severity: severity_of(event),
            attributes: identity_of(session, event),
            body: event.data().clone(),
        };
        self.deliver(session, record, Some(event.seq()))
    }

    fn deliver(
        &self,
        session: &Session,
        record: SessionTelemetryRecord,
        seq: Option<SessionSeq>,
    ) -> Result<(), BoxError> {
        let record = self.0.redactions.apply(record)?;
        self.0.backend.emit(record)?;
        if let Some(seq) = seq {
            set_cursor(session, seq);
        }
        Ok(())
    }

    fn relay_agent_error(&self, event: &AgentError) -> Result<(), BoxError> {
        let error = event.error.get_ref();
        let (name, message) = error_detail(error);
        let session = event.agent.session();
        let mut attributes = IndexMap::new();
        attributes.insert("telemetry.op".into(), "agent-error".into());
        attributes.insert("session.id".into(), session.id().to_string().into());
        attributes.insert("agent.id".into(), event.agent.id().to_string().into());
        attributes.insert("error.name".into(), name.clone().into());
        attributes.insert("turn".into(), event.turn.into());
        attributes.insert("step".into(), event.step.into());
        self.deliver(
            session,
            SessionTelemetryRecord {
                source_event: None,
                channel: SessionTelemetryChannel::Ops,
                time: now(),
                severity: SessionTelemetrySeverity::Error,
                attributes,
                body: json!({"name": name, "message": message}),
            },
            None,
        )
    }

    fn contain(&self, step: impl FnOnce() -> Result<(), BoxError>) {
        let result = catch_unwind(AssertUnwindSafe(step))
            .map_err(panic_error)
            .and_then(|r| r);
        if let Err(error) = result {
            self.0.ctx.logger().warn(format_args!(
                "telemetry: capture step failed: {}",
                js_error(&*error)
            ));
        }
    }
}

fn shutdown_record(session: &Session) -> SessionTelemetryRecord {
    let mut attributes = IndexMap::new();
    attributes.insert("telemetry.op".into(), "shutdown".into());
    attributes.insert("session.id".into(), session.id().to_string().into());
    SessionTelemetryRecord {
        source_event: None,
        channel: SessionTelemetryChannel::Ops,
        time: now(),
        severity: SessionTelemetrySeverity::Info,
        attributes,
        body: json!({"op": "shutdown"}),
    }
}

fn severity_of(event: &SessionEvent) -> SessionTelemetrySeverity {
    let failed = match event.event_type() {
        "tool/result" => event.data()["message"]["isError"] == true,
        "turn/end" => event.data()["reason"]["kind"] == "error",
        _ => false,
    };
    if failed {
        SessionTelemetrySeverity::Error
    } else {
        SessionTelemetrySeverity::Info
    }
}

fn identity_of(
    session: &Session,
    event: &SessionEvent,
) -> IndexMap<String, SessionTelemetryAttribute> {
    let header = session.header();
    let mut attributes = IndexMap::new();
    attributes.insert("session.id".into(), session.id().to_string().into());
    if let Some(version) = header["version"].as_u64() {
        attributes.insert("session.format_version".into(), version.into());
    }
    attributes.insert("event.type".into(), event.event_type().into());
    attributes.insert("event.seq".into(), u64::from(event.seq()).into());
    if let Some(cwd) = header.get("cwd").and_then(Value::as_str) {
        attributes.insert("session.cwd".into(), cwd.into());
    }
    if let Some(parent) = header.get("parentSession").and_then(Value::as_str) {
        attributes.insert("session.parent_id".into(), parent.into());
    }
    if header.get("isSeeded").and_then(Value::as_bool) == Some(true) {
        attributes.insert(
            "session.seed_length".into(),
            u64::from(session.inherited_event_count()).into(),
        );
    }
    attributes
}

fn error_detail(error: &(dyn std::error::Error + 'static)) -> (String, String) {
    let name = if let Some(error) = HarnessError::of(error) {
        error.name().to_owned()
    } else if let Some(error) = error.downcast_ref::<hoh_session::SessionError>() {
        error.name().to_owned()
    } else {
        "Error".to_owned()
    };
    (name, error.to_string())
}

fn js_error(error: &(dyn std::error::Error + 'static)) -> String {
    let (name, message) = error_detail(error);
    if message.is_empty() {
        name
    } else {
        format!("{name}: {message}")
    }
}

fn panic_error(payload: Box<dyn std::any::Any + Send>) -> BoxError {
    let message = payload
        .downcast_ref::<&str>()
        .map(|value| (*value).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "telemetry capture panicked".to_owned());
    message.into()
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

type HandoffCursors = Mutex<Vec<(Weak<Value>, SessionSeq)>>;

fn cursors() -> &'static HandoffCursors {
    static CURSORS: OnceLock<HandoffCursors> = OnceLock::new();
    CURSORS.get_or_init(|| Mutex::new(Vec::new()))
}

fn cursor_of(session: &Session) -> Option<SessionSeq> {
    let identity = Arc::downgrade(&session.header());
    let mut cursors = lock(cursors());
    cursors.retain(|(entry, _)| entry.strong_count() != 0);
    cursors
        .iter()
        .find(|(entry, _)| entry.ptr_eq(&identity))
        .map(|(_, seq)| *seq)
}

fn set_cursor(session: &Session, seq: SessionSeq) {
    let identity = Arc::downgrade(&session.header());
    let mut cursors = lock(cursors());
    cursors.retain(|(entry, _)| entry.strong_count() != 0);
    if let Some((_, cursor)) = cursors
        .iter_mut()
        .find(|(entry, _)| entry.ptr_eq(&identity))
    {
        *cursor = seq;
    } else {
        cursors.push((identity, seq));
    }
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
