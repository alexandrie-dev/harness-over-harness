use crate::*;
use hoh_kernel::{Context, Fiber, Kernel};
use hoh_session::{
    Session, SessionEvent, SessionHeader, SessionLogOffset, SessionSeq, SessionSeqCursor,
    SessionStore, Sessions,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

pub struct Harness {
    pub kernel: Kernel,
    pub ctx: Context,
    pub registry: SessionProjectionRegistry,
    pub session: Session,
    pub sessions: Arc<SessionStore>,
}
pub async fn harness() -> Harness {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let sessions = Arc::new(SessionStore::new());
    ctx.provide::<Sessions>(sessions.clone()).await.unwrap();
    let registry = SessionProjectionRegistry::new(&ctx).unwrap();
    ctx.provide::<SessionProjections>(Arc::new(registry.clone()))
        .await
        .unwrap();
    let session = sessions
        .create(&ctx, Some("first".into()), Default::default())
        .unwrap();
    Harness {
        kernel,
        ctx,
        registry,
        session,
        sessions,
    }
}
impl Harness {
    pub fn another(&self, id: &str) -> Session {
        self.sessions
            .create(&self.ctx, Some(id.into()), Default::default())
            .unwrap()
    }
    pub fn prepared(&self, id: &str, events: Vec<Arc<SessionEvent>>) -> Session {
        self.sessions
            .prepare(
                Some(id.into()),
                hoh_session::PrepareSessionOptions::Create(hoh_session::CreateSessionOptions {
                    seed: Some(events),
                    ..Default::default()
                }),
            )
            .unwrap()
    }
    pub async fn close(self) {
        self.kernel.shutdown().await;
    }
}
pub async fn owner(ctx: &Context) -> (Fiber, Context) {
    let slot = Arc::new(Mutex::new(None));
    let capture = slot.clone();
    let fiber = ctx
        .inject::<(), _>(move |ctx, ()| {
            *lock(&capture) = Some(ctx);
            async { Ok(()) }
        })
        .unwrap();
    fiber.settled().await.unwrap();
    let ctx = lock(&slot).clone().unwrap();
    (fiber, ctx)
}
pub fn offset(value: u64) -> SessionLogOffset {
    SessionLogOffset::new(value).unwrap()
}
pub fn seq(value: i64) -> SessionSeqCursor {
    if value < 0 {
        SessionSeqCursor::BeforeFirst
    } else {
        SessionSeqCursor::Seq(SessionSeq::new(value as u64).unwrap())
    }
}
pub fn raw(event_type: &str, sequence: u64, data: Value) -> Arc<SessionEvent> {
    Arc::new(SessionEvent::from_value(
        json!({"type":event_type,"seq":sequence,"time":sequence,"data":data}),
    ))
}
pub fn header() -> SessionHeader {
    SessionHeader::new("projection-restore".into(), 0)
}
pub fn mark(session: &Session, marks: &[&str]) -> Arc<SessionEvent> {
    session
        .append("test/mark", json!({"marks":marks}), None)
        .unwrap()
}
pub fn turn(session: &Session, turn: u64) -> Arc<SessionEvent> {
    session
        .append("turn/start", json!({"turn":turn}), None)
        .unwrap()
}
pub fn values(snapshot: ProjectionSnapshot) -> Value {
    serde_json::to_value(snapshot.values).unwrap()
}
pub fn row(ver: u64, sequence: i64, val: Value) -> ProjectionCheckpointRow {
    ProjectionCheckpointRow {
        ver,
        seq: seq(sequence),
        val: Some(val),
    }
}
pub fn rows(entries: Vec<(&str, ProjectionCheckpointRow)>) -> ProjectionCheckpoint {
    entries
        .into_iter()
        .map(|(key, row)| (key.into(), row))
        .collect()
}
pub fn json<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap()
}
pub fn marks_schema(value: Option<&Value>) -> Result<Arc<Value>> {
    let marks = value
        .and_then(|value| value.get("marks"))
        .and_then(Value::as_array)
        .filter(|marks| marks.iter().all(Value::is_string))
        .ok_or_else(|| error("invalid marks view"))?;
    Ok(Arc::new(json!({"marks":marks})))
}
pub fn number_schema(value: Option<&Value>) -> Result<Arc<u64>> {
    let value = value
        .and_then(Value::as_f64)
        .filter(|value| {
            value.is_finite()
                && *value >= 0.
                && *value <= hoh_session::MAX_SAFE_INTEGER as f64
                && value.fract() == 0.
        })
        .ok_or_else(|| error("invalid numeric state"))?;
    Ok(Arc::new(value as u64))
}
pub fn number_view_schema(value: Option<&Value>) -> Result<Arc<Value>> {
    Ok(Arc::new(json!(*number_schema(value)?)))
}
pub fn marks_unit() -> ProjectionDefinition<Value> {
    ProjectionDefinition {
        key: "test/marks".into(),
        state_version: 1,
        state_schema: Arc::new(|value| {
            if value == Some(&Value::Null) {
                Ok(Arc::new(Value::Null))
            } else {
                marks_schema(value)
            }
        }),
        init: Arc::new(|_, _| Ok(Arc::new(Value::Null))),
        apply: Arc::new(|state, event| {
            if event.event_type() == "test/mark" {
                Ok(Arc::new(event.data().clone()))
            } else {
                Ok(state.clone())
            }
        }),
        wire: Some(ProjectionWire::new(
            Arc::new(marks_schema),
            Arc::new(|state| {
                Ok(Some(if state.is_null() {
                    Arc::new(json!({"marks":[]}))
                } else {
                    state.clone()
                }))
            }),
        )),
    }
}
pub fn count_unit() -> ProjectionDefinition<u64> {
    ProjectionDefinition {
        key: "test/count".into(),
        state_version: 1,
        state_schema: Arc::new(number_schema),
        init: Arc::new(|_, _| Ok(Arc::new(0))),
        apply: Arc::new(|state, _| Ok(Arc::new(**state + 1))),
        wire: None,
    }
}
pub fn reader_unit() -> ProjectionDefinition<u64> {
    let mut unit = count_unit();
    unit.key = "test/reader".into();
    unit.wire = Some(ProjectionWire::new(
        Arc::new(number_view_schema),
        Arc::new(|state| Ok(Some(Arc::new(json!(**state))))),
    ));
    unit
}
#[derive(Clone, Serialize)]
pub struct StableState {
    pub revision: u64,
    pub value: Arc<Value>,
}
pub fn stable_unit(view: ProjectionView<StableState>) -> ProjectionDefinition<StableState> {
    ProjectionDefinition {
        key: "test/stable-view".into(),
        state_version: 1,
        state_schema: Arc::new(|value| {
            let value = value.ok_or_else(|| error("missing stable state"))?;
            Ok(Arc::new(StableState {
                revision: *number_schema(value.get("revision"))?,
                value: marks_schema(value.get("value"))?,
            }))
        }),
        init: Arc::new(|_, _| {
            Ok(Arc::new(StableState {
                revision: 0,
                value: Arc::new(json!({"marks":[]})),
            }))
        }),
        apply: Arc::new(|state, event| match event.event_type() {
            "turn/start" => Ok(Arc::new(StableState {
                revision: state.revision + 1,
                value: state.value.clone(),
            })),
            "test/mark" => Ok(Arc::new(StableState {
                revision: state.revision + 1,
                value: Arc::new(event.data().clone()),
            })),
            _ => Ok(state.clone()),
        }),
        wire: Some(ProjectionWire::new(Arc::new(marks_schema), view)),
    }
}
pub fn stable() -> ProjectionDefinition<StableState> {
    stable_unit(Arc::new(|state| Ok(Some(state.value.clone()))))
}
pub fn cut_unit() -> ProjectionDefinition<u64> {
    let mut unit = count_unit();
    unit.key = "test/cut".into();
    unit.init = Arc::new(|_, cut| Ok(Arc::new(cut.get())));
    unit.apply = Arc::new(|state, _| Ok(state.clone()));
    unit
}
pub fn key_selection(keys: &[&str]) -> indexmap::IndexSet<String> {
    keys.iter().map(|key| (*key).into()).collect()
}
pub fn selected<'a>(keys: &'a indexmap::IndexSet<String>) -> Option<ProjectionKeySelection<'a>> {
    Some(ProjectionKeySelection::Set(keys))
}
