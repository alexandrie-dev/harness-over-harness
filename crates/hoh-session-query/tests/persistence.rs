use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use hoh_error::BoxError;
use hoh_kernel::Kernel;
use hoh_session::{
    CreateSessionOptions, SessionEvent, SessionHeader, SessionId, SessionLogOffset, SessionPlugin,
    SessionSeedEventState, SessionStore, Sessions,
};
use hoh_session_persistence::{
    SessionAccess, SessionFormatStatus, SessionHandle, SessionHandleAppendOptions,
    SessionHandleFlushOptions, SessionHandleReadOptions, SessionHandleReadResult,
    SessionPersistence, SessionPersistenceCreateOptions, SessionPersistenceIdentity,
    SessionPersistenceListOptions, SessionPersistenceOpenOptions, SessionPersistenceRevision,
    SessionPersistenceService, SessionPersistenceSnapshot, SessionPersistenceStatOptions,
};
use hoh_session_projection::{
    ProjectionDefinition, ProjectionWire, SessionProjectionRegistry, SessionProjections,
};
use hoh_session_projection_cache::SessionProjectionCachePlugin;
use hoh_session_query::*;
use hoh_storage::StoragePlugin;
use hoh_storage_domain::StorageDomainPlugin;
use hoh_storage_json::StorageJsonPlugin;
use hoh_timeout::AbortController;
use serde_json::json;
use std::panic::AssertUnwindSafe;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::Semaphore;

fn failure(message: &'static str) -> BoxError {
    Box::new(std::io::Error::other(message))
}

#[derive(Default)]
struct Counts {
    opens: AtomicUsize,
    reads: AtomicUsize,
    closes: AtomicUsize,
    lists: AtomicUsize,
    stats: AtomicUsize,
}

struct FakeState {
    header: Mutex<SessionHeader>,
    events: Mutex<Vec<Arc<SessionEvent>>>,
    revision: Mutex<SessionPersistenceRevision>,
    format_status: Mutex<Option<SessionFormatStatus>>,
    counts: Counts,
    fail_list: AtomicBool,
    fail_read: AtomicBool,
    fail_close: AtomicBool,
    fail_stat: AtomicBool,
    missing_stat: AtomicBool,
    missing_open: AtomicBool,
    abort_read_with_signal: AtomicBool,
    abort_list_with_signal: AtomicBool,
    read_signals: AtomicUsize,
    list_signals: AtomicUsize,
    read_error: Mutex<Option<BoxError>>,
    stat_header: Mutex<Option<SessionHeader>>,
    on_read: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    on_stat: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    on_list: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    list_gate: Mutex<Option<Arc<Gate>>>,
    read_gate: Mutex<Option<Arc<Gate>>>,
    additional: Mutex<Vec<Arc<FakeState>>>,
}

struct Gate {
    started: Semaphore,
    release: Semaphore,
}

impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            started: Semaphore::new(0),
            release: Semaphore::new(0),
        })
    }

    async fn block(&self) {
        self.started.add_permits(1);
        self.release.acquire().await.unwrap().forget();
    }
}

impl FakeState {
    fn new(id: &str, events: Vec<Arc<SessionEvent>>) -> Arc<Self> {
        Arc::new(Self {
            header: Mutex::new(SessionHeader::new(SessionId::from(id), 1)),
            events: Mutex::new(events),
            revision: Mutex::new(SessionPersistenceRevision::from("r1")),
            format_status: Mutex::new(None),
            counts: Counts::default(),
            fail_list: AtomicBool::new(false),
            fail_read: AtomicBool::new(false),
            fail_close: AtomicBool::new(false),
            fail_stat: AtomicBool::new(false),
            missing_stat: AtomicBool::new(false),
            missing_open: AtomicBool::new(false),
            abort_read_with_signal: AtomicBool::new(false),
            abort_list_with_signal: AtomicBool::new(false),
            read_signals: AtomicUsize::new(0),
            list_signals: AtomicUsize::new(0),
            read_error: Mutex::new(None),
            stat_header: Mutex::new(None),
            on_read: Mutex::new(None),
            on_stat: Mutex::new(None),
            on_list: Mutex::new(None),
            list_gate: Mutex::new(None),
            read_gate: Mutex::new(None),
            additional: Mutex::new(Vec::new()),
        })
    }
}

struct FakeHandle {
    state: Arc<FakeState>,
    id: SessionId,
    header: SessionHeader,
}

impl SessionHandle for FakeHandle {
    fn id(&self) -> &SessionId {
        &self.id
    }
    fn header(&self) -> &SessionHeader {
        &self.header
    }
    fn inherited_event_count(&self) -> SessionLogOffset {
        SessionLogOffset::default()
    }
    fn access(&self) -> SessionAccess {
        SessionAccess::Read
    }
    fn read<'a>(
        &'a self,
        _offset: Option<u64>,
        _length: Option<u64>,
        options: Option<&'a SessionHandleReadOptions>,
    ) -> BoxFuture<'a, Result<SessionHandleReadResult, BoxError>> {
        let signal = options.and_then(|options| options.signal.clone());
        Box::pin(async move {
            self.state.counts.reads.fetch_add(1, Ordering::SeqCst);
            if signal.is_some() {
                self.state.read_signals.fetch_add(1, Ordering::SeqCst);
            }
            if self.state.abort_read_with_signal.load(Ordering::SeqCst) {
                let reason = signal
                    .expect("abort-aware read requires a signal")
                    .cancelled()
                    .await;
                return Err(Box::new(reason) as BoxError);
            }
            let gate = self.state.read_gate.lock().unwrap().clone();
            if let Some(gate) = gate {
                gate.block().await;
            }
            if self.state.fail_read.load(Ordering::SeqCst) {
                return Err(failure("read failed"));
            }
            if let Some(error) = self.state.read_error.lock().unwrap().take() {
                return Err(error);
            }
            if let Some(hook) = self.state.on_read.lock().unwrap().take() {
                hook();
            }
            Ok(SessionHandleReadResult {
                event_state: SessionSeedEventState::Detached,
                events: self.state.events.lock().unwrap().clone(),
            })
        })
    }
    fn append<'a>(
        &'a self,
        _events: &'a [Arc<SessionEvent>],
        _options: Option<&'a SessionHandleAppendOptions>,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async { Err(failure("read only")) })
    }
    fn flush<'a>(
        &'a self,
        _options: Option<&'a SessionHandleFlushOptions>,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async { Err(failure("read only")) })
    }
    fn close(&self) -> BoxFuture<'_, Result<(), BoxError>> {
        Box::pin(async move {
            self.state.counts.closes.fetch_add(1, Ordering::SeqCst);
            if self.state.fail_close.load(Ordering::SeqCst) {
                Err(failure("close failed"))
            } else {
                Ok(())
            }
        })
    }
}

struct FakePersistence {
    identity: SessionPersistenceIdentity,
    state: Arc<FakeState>,
}

impl FakePersistence {
    fn new(state: Arc<FakeState>) -> Self {
        Self {
            identity: SessionPersistenceIdentity::new(),
            state,
        }
    }

    fn states(&self) -> Vec<Arc<FakeState>> {
        let mut states = vec![self.state.clone()];
        states.extend(self.state.additional.lock().unwrap().iter().cloned());
        states
    }

    fn state_for(&self, id: &SessionId) -> Option<Arc<FakeState>> {
        self.states()
            .into_iter()
            .find(|state| state.header.lock().unwrap().id == *id)
    }
}

impl SessionPersistence for FakePersistence {
    fn identity(&self) -> SessionPersistenceIdentity {
        self.identity.clone()
    }
    fn create<'a>(
        &'a self,
        _header: &'a SessionHeader,
        _options: Option<&'a SessionPersistenceCreateOptions>,
    ) -> BoxFuture<'a, Result<Arc<dyn SessionHandle>, BoxError>> {
        Box::pin(async { Err(failure("unused create")) })
    }
    fn open<'a>(
        &'a self,
        id: &'a SessionId,
        _access: SessionAccess,
        _options: Option<&'a SessionPersistenceOpenOptions>,
    ) -> BoxFuture<'a, Result<Arc<dyn SessionHandle>, BoxError>> {
        let state = self.state_for(id);
        Box::pin(async move {
            let Some(state) = state else {
                return Err(Box::new(
                    hoh_session_persistence::SessionPersistenceNotFoundError::new(id.clone()),
                ) as BoxError);
            };
            state.counts.opens.fetch_add(1, Ordering::SeqCst);
            if state.missing_open.load(Ordering::SeqCst) {
                return Err(Box::new(
                    hoh_session_persistence::SessionPersistenceNotFoundError::new(id.clone()),
                ) as BoxError);
            }
            let header = state.header.lock().unwrap().clone();
            Ok(Arc::new(FakeHandle {
                state,
                id: id.clone(),
                header,
            }) as Arc<dyn SessionHandle>)
        })
    }
    fn flush(&self) -> BoxFuture<'_, Result<(), BoxError>> {
        Box::pin(async { Ok(()) })
    }
    fn stat<'a>(
        &'a self,
        id: &'a SessionId,
        _options: Option<&'a SessionPersistenceStatOptions>,
    ) -> BoxFuture<'a, Result<Option<SessionPersistenceSnapshot>, BoxError>> {
        let state = self.state_for(id);
        Box::pin(async move {
            let Some(state) = state else { return Ok(None) };
            state.counts.stats.fetch_add(1, Ordering::SeqCst);
            if state.fail_stat.load(Ordering::SeqCst) {
                return Err(failure("stat failed"));
            }
            if let Some(hook) = state.on_stat.lock().unwrap().take() {
                hook();
            }
            if state.missing_stat.load(Ordering::SeqCst) {
                return Ok(None);
            }
            let stored = state.header.lock().unwrap().clone();
            let header = state.stat_header.lock().unwrap().clone().unwrap_or(stored);
            Ok(Some(SessionPersistenceSnapshot {
                header,
                format_status: *state.format_status.lock().unwrap(),
                revision: state.revision.lock().unwrap().clone(),
                event_count: None,
                size_bytes: None,
                location: None,
            }))
        })
    }
    fn list<'a>(
        &'a self,
        options: Option<&'a SessionPersistenceListOptions>,
    ) -> BoxFuture<'a, Result<Vec<SessionPersistenceSnapshot>, BoxError>> {
        let signal = options.and_then(|options| options.signal.clone());
        Box::pin(async move {
            self.state.counts.lists.fetch_add(1, Ordering::SeqCst);
            if signal.is_some() {
                self.state.list_signals.fetch_add(1, Ordering::SeqCst);
            }
            if self.state.abort_list_with_signal.load(Ordering::SeqCst) {
                let reason = signal
                    .expect("abort-aware list requires a signal")
                    .cancelled()
                    .await;
                return Err(Box::new(reason) as BoxError);
            }
            let gate = self.state.list_gate.lock().unwrap().clone();
            if let Some(gate) = gate {
                gate.block().await;
            }
            if self.state.fail_list.load(Ordering::SeqCst) {
                return Err(failure("list failed"));
            }
            let snapshots = self
                .states()
                .into_iter()
                .map(|state| SessionPersistenceSnapshot {
                    header: state.header.lock().unwrap().clone(),
                    format_status: *state.format_status.lock().unwrap(),
                    revision: state.revision.lock().unwrap().clone(),
                    event_count: None,
                    size_bytes: None,
                    location: None,
                })
                .collect();
            if let Some(hook) = self.state.on_list.lock().unwrap().take() {
                hook();
            }
            Ok(snapshots)
        })
    }
}

fn raw_event(kind: &str, seq: u64, data: serde_json::Value) -> Arc<SessionEvent> {
    Arc::new(SessionEvent::from_value(
        json!({"type":kind,"seq":seq,"time":seq as i64,"data":data}),
    ))
}

fn prepared_projection(fail: bool) -> ProjectionDefinition<u64> {
    ProjectionDefinition {
        key: if fail {
            "query/prepared-failing"
        } else {
            "query/prepared-count"
        }
        .into(),
        state_version: 1,
        state_schema: Arc::new(|value| {
            value
                .and_then(serde_json::Value::as_u64)
                .map(Arc::new)
                .ok_or_else(|| failure("invalid projection state"))
        }),
        init: Arc::new(|_, _| Ok(Arc::new(0))),
        apply: Arc::new(move |state, _| {
            if fail {
                Err(failure("prepared projection failed"))
            } else {
                Ok(Arc::new(**state + 1))
            }
        }),
        wire: Some(ProjectionWire::new(
            Arc::new(|value| Ok(Arc::new(value.cloned().unwrap_or(serde_json::Value::Null)))),
            Arc::new(|state| Ok(Some(Arc::new(json!(**state))))),
        )),
    }
}

async fn mount_projection(kernel: &Kernel, fail: bool) {
    let registry = SessionProjectionRegistry::new(&kernel.root()).unwrap();
    registry
        .register(&kernel.root(), prepared_projection(fail))
        .await
        .unwrap();
    kernel
        .root()
        .provide::<SessionProjections>(Arc::new(registry))
        .await
        .unwrap();
}

struct SearchDisabled;
impl SessionSearchProvider for SearchDisabled {
    fn search_sessions<'a>(
        &'a self,
        _request: SessionSearchRequest,
        _exec: SessionSearchExecContext,
    ) -> BoxFuture<'a, Result<SessionSearchPage<SessionSearchHit>, BoxError>> {
        Box::pin(async {
            Err(Box::new(SessionQueryError::new(
                "disabled",
                SessionQueryErrorCode::SearchDisabled,
            )) as BoxError)
        })
    }
    fn search_events<'a>(
        &'a self,
        _request: SessionEventSearchRequest,
        _exec: SessionSearchExecContext,
    ) -> BoxFuture<'a, Result<SessionEventSearchPage, BoxError>> {
        Box::pin(async {
            Err(Box::new(SessionQueryError::new(
                "disabled",
                SessionQueryErrorCode::SearchDisabled,
            )) as BoxError)
        })
    }
}

async fn mounted_with_config(state: Arc<FakeState>, config: Config) -> (Kernel, SessionStore) {
    let kernel = Kernel::start();
    let store = SessionStore::new();
    kernel
        .root()
        .provide::<Sessions>(Arc::new(store.clone()))
        .await
        .unwrap();
    let persistence: Arc<dyn SessionPersistence> = Arc::new(FakePersistence::new(state));
    kernel
        .root()
        .provide::<SessionPersistenceService>(persistence)
        .await
        .unwrap();
    let sessions = kernel.root().get::<Sessions>().unwrap();
    mount_session_query(&kernel.root(), sessions, Arc::new(SearchDisabled), config)
        .await
        .unwrap();
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    (kernel, store)
}

async fn mounted(state: Arc<FakeState>) -> (Kernel, SessionStore) {
    mounted_with_config(state, Config::default()).await
}

#[tokio::test]
async fn cold_read_closes_a_successful_handle() {
    let state = FakeState::new("stored", Vec::new());
    let persistence = FakePersistence::new(state.clone());
    let log = read_cold_session_log(&persistence, &SessionId::from("stored"), None)
        .await
        .unwrap();
    assert!(log.events.is_empty());
    assert_eq!(state.counts.opens.load(Ordering::SeqCst), 1);
    assert_eq!(state.counts.reads.load(Ordering::SeqCst), 1);
    assert_eq!(state.counts.closes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cold_read_preserves_read_failure_when_close_also_fails() {
    let state = FakeState::new("stored", Vec::new());
    state.fail_read.store(true, Ordering::SeqCst);
    state.fail_close.store(true, Ordering::SeqCst);
    let error = read_cold_session_log(
        &FakePersistence::new(state.clone()),
        &SessionId::from("stored"),
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), "read failed");
    assert_eq!(state.counts.closes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cold_read_surfaces_close_failure_after_successful_read() {
    let state = FakeState::new("stored", Vec::new());
    state.fail_close.store(true, Ordering::SeqCst);
    let error = read_cold_session_log(
        &FakePersistence::new(state),
        &SessionId::from("stored"),
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), "close failed");
}

#[tokio::test]
async fn cold_read_balances_an_interrupted_turn_in_memory() {
    let events = vec![raw_event("turn/start", 0, json!({"turn":1}))];
    let state = FakeState::new("stored", events);
    let log = read_cold_session_log(
        &FakePersistence::new(state),
        &SessionId::from("stored"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(log.events.len(), 2);
    assert_eq!(log.events[1].event_type(), "turn/end");
}

#[tokio::test]
async fn observation_prepares_cold_session_and_reuses_unchanged_revision() {
    let state = FakeState::new("stored", Vec::new());
    let (kernel, _store) = mounted(state.clone()).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let mut first = query
        .observe_session(
            &SessionId::from("stored"),
            SessionObservationOptions {
                projection_mode: ProjectionMode::None,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(first.source(), SessionObservationSource::Prepared);
    assert_eq!(first.revision().unwrap().as_ref(), "r1");
    first.dispose();
    let mut second = query
        .observe_session(
            &SessionId::from("stored"),
            SessionObservationOptions {
                projection_mode: ProjectionMode::None,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(state.counts.reads.load(Ordering::SeqCst), 1);
    second.dispose();
    kernel.shutdown().await;
}

#[tokio::test]
async fn prepared_observation_hydrates_projections_without_a_projection_cache() {
    let state = FakeState::new(
        "prepared-projection",
        vec![raw_event(
            "user/message",
            0,
            json!({"id":"u","role":"user","content":[],"source":{"kind":"user"}}),
        )],
    );
    state.events.lock().unwrap()[0] = Arc::new(SessionEvent::from_value(json!({
        "type":"user/message", "seq":0, "time":0,
        "data":{"id":"u","role":"user","content":[],"source":{"kind":"user"}},
        "surfaceOp":"append"
    })));
    let (kernel, _store) = mounted(state).await;
    mount_projection(&kernel, false).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let observation = query
        .observe_session(&SessionId::from("prepared-projection"), Default::default())
        .await
        .unwrap();
    let snapshot = observation.projections().unwrap();
    assert_eq!(snapshot.values["query/prepared-count"].as_u64(), Some(1));
    kernel.shutdown().await;
}

#[tokio::test]
async fn prepared_observation_wraps_projection_failure_as_corrupt_session() {
    let state = FakeState::new(
        "prepared-projection-failure",
        vec![raw_event(
            "user/message",
            0,
            json!({"id":"u","role":"user","content":[],"source":{"kind":"user"}}),
        )],
    );
    state.events.lock().unwrap()[0] = Arc::new(SessionEvent::from_value(json!({
        "type":"user/message", "seq":0, "time":0,
        "data":{"id":"u","role":"user","content":[],"source":{"kind":"user"}},
        "surfaceOp":"append"
    })));
    let (kernel, _store) = mounted(state).await;
    mount_projection(&kernel, true).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let error = query
        .observe_session(
            &SessionId::from("prepared-projection-failure"),
            Default::default(),
        )
        .await
        .err()
        .unwrap();
    let harness = hoh_error::HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(
        harness.code(),
        SessionQueryErrorCode::CorruptSession.as_str()
    );
    assert!(harness.to_string().contains("failed to project session"));
    kernel.shutdown().await;
}

#[tokio::test]
async fn prepared_observation_hydrates_through_a_mounted_projection_cache() {
    let root = tempfile::tempdir().unwrap();
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<StoragePlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<StorageJsonPlugin>(json!({"root":root.path().to_string_lossy()}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<StorageDomainPlugin>(json!({"backend":"json"}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<SessionProjectionRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.get::<SessionProjections>()
        .unwrap()
        .register(&ctx, prepared_projection(false))
        .await
        .unwrap();
    ctx.plugin::<SessionProjectionCachePlugin>(json!({
        "writeEveryEvents": 1,
        "writeIntervalMs": 1000
    }))
    .unwrap()
    .settled()
    .await
    .unwrap();

    let event = Arc::new(SessionEvent::from_value(json!({
        "type":"user/message", "seq":0, "time":1,
        "data":{"id":"user-cache","role":"user","content":[{"type":"text","text":"cached"}],"source":{"kind":"user"}},
        "surfaceOp":"append"
    })));
    let state = FakeState::new("prepared-cache", vec![event]);
    let persistence: Arc<dyn SessionPersistence> = Arc::new(FakePersistence::new(state));
    ctx.provide::<SessionPersistenceService>(persistence)
        .await
        .unwrap();
    mount_session_query(
        &ctx,
        ctx.get::<Sessions>().unwrap(),
        Arc::new(SearchDisabled),
        Config::default(),
    )
    .await
    .unwrap();
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    let observation = ctx
        .get::<SessionQuery>()
        .unwrap()
        .observe_session(&SessionId::from("prepared-cache"), Default::default())
        .await
        .unwrap();
    assert_eq!(
        observation.projections().unwrap().values["query/prepared-count"].as_u64(),
        Some(1)
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn observation_reloads_when_revision_changes() {
    let state = FakeState::new("stored", Vec::new());
    let (kernel, _store) = mounted(state.clone()).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    drop(
        query
            .observe_session(
                &SessionId::from("stored"),
                SessionObservationOptions {
                    projection_mode: ProjectionMode::None,
                    ..Default::default()
                },
            )
            .await
            .unwrap(),
    );
    *state.revision.lock().unwrap() = SessionPersistenceRevision::from("r2");
    drop(
        query
            .observe_session(
                &SessionId::from("stored"),
                SessionObservationOptions {
                    projection_mode: ProjectionMode::None,
                    ..Default::default()
                },
            )
            .await
            .unwrap(),
    );
    assert_eq!(state.counts.reads.load(Ordering::SeqCst), 2);
    kernel.shutdown().await;
}

#[tokio::test]
async fn observation_prefers_live_session_attached_while_cold_log_is_read() {
    let state = FakeState::new("attached-during-read", Vec::new());
    let (kernel, store) = mounted(state.clone()).await;
    let root = kernel.root();
    let attach_store = store.clone();
    *state.on_read.lock().unwrap() = Some(Arc::new(move || {
        attach_store
            .create(
                &root,
                Some(SessionId::from("attached-during-read")),
                CreateSessionOptions {
                    meta: Some(json!({"createdAt":1})),
                    ..Default::default()
                },
            )
            .unwrap();
    }));
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let observed = query
        .observe_session(
            &SessionId::from("attached-during-read"),
            SessionObservationOptions {
                projection_mode: ProjectionMode::None,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(observed.source(), SessionObservationSource::Live);
    kernel.shutdown().await;
}

#[tokio::test]
async fn observation_prefers_live_session_attached_while_source_is_stated() {
    let state = FakeState::new("attached-during-stat", Vec::new());
    let (kernel, store) = mounted(state.clone()).await;
    let root = kernel.root();
    let attach_store = store.clone();
    *state.on_stat.lock().unwrap() = Some(Arc::new(move || {
        attach_store
            .create(
                &root,
                Some(SessionId::from("attached-during-stat")),
                CreateSessionOptions {
                    meta: Some(json!({"createdAt":1})),
                    ..Default::default()
                },
            )
            .unwrap();
    }));
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let observed = query
        .observe_session(
            &SessionId::from("attached-during-stat"),
            SessionObservationOptions {
                projection_mode: ProjectionMode::None,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(observed.source(), SessionObservationSource::Live);
    assert_eq!(state.counts.opens.load(Ordering::SeqCst), 0);
    kernel.shutdown().await;
}

#[tokio::test]
async fn observation_maps_restore_validation_failure_to_corrupt_session() {
    let malformed = raw_event(
        "user/message",
        5,
        json!({"id":"bad","role":"user","content":[],"source":{"kind":"user"}}),
    );
    let state = FakeState::new("bad-restore", vec![malformed]);
    let (kernel, _store) = mounted(state).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let error = match query
        .observe_session(&SessionId::from("bad-restore"), Default::default())
        .await
    {
        Ok(_) => panic!("malformed restored log unexpectedly observed"),
        Err(error) => error,
    };
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        SessionQueryErrorCode::CorruptSession.as_str()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn observation_maps_stat_read_corruption_and_source_conflict_failures() {
    let state = FakeState::new("taxonomy", Vec::new());
    let (kernel, _store) = mounted(state.clone()).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();

    state.fail_stat.store(true, Ordering::SeqCst);
    let error = query
        .observe_session(&SessionId::from("taxonomy"), Default::default())
        .await
        .err()
        .unwrap();
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        SessionQueryErrorCode::PersistenceFailed.as_str()
    );
    state.fail_stat.store(false, Ordering::SeqCst);

    *state.read_error.lock().unwrap() = Some(Box::new(
        hoh_session_persistence::SessionPersistenceCorruptionError::new(
            "torn record",
            failure("torn cause"),
        ),
    ));
    let error = query
        .observe_session(&SessionId::from("taxonomy"), Default::default())
        .await
        .err()
        .unwrap();
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        SessionQueryErrorCode::CorruptSession.as_str()
    );

    let mut wrong = SessionHeader::new(SessionId::from("other"), 1);
    wrong.is_seeded = false;
    *state.stat_header.lock().unwrap() = Some(wrong);
    let error = query
        .observe_session(&SessionId::from("taxonomy"), Default::default())
        .await
        .err()
        .unwrap();
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        SessionQueryErrorCode::SourceConflict.as_str()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn observation_maps_deletion_between_stat_and_open_to_not_found() {
    let state = FakeState::new("deleted-after-stat", Vec::new());
    let delete = state.clone();
    *state.on_stat.lock().unwrap() = Some(Arc::new(move || {
        delete.missing_open.store(true, Ordering::SeqCst);
    }));
    let (kernel, _store) = mounted(state.clone()).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let error = query
        .observe_session(&SessionId::from("deleted-after-stat"), Default::default())
        .await
        .err()
        .unwrap();
    let harness = hoh_error::HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(
        harness.code(),
        SessionQueryErrorCode::SessionNotFound.as_str()
    );
    assert!(
        harness
            .cause()
            .unwrap()
            .downcast_ref::<hoh_session_persistence::SessionPersistenceNotFoundError>()
            .is_some()
    );
    assert_eq!(state.counts.stats.load(Ordering::SeqCst), 1);
    assert_eq!(state.counts.opens.load(Ordering::SeqCst), 1);
    kernel.shutdown().await;
}

#[tokio::test]
async fn observation_maps_plain_read_failure_to_persistence_failed() {
    let state = FakeState::new("read-failed", Vec::new());
    state.fail_read.store(true, Ordering::SeqCst);
    let (kernel, _store) = mounted(state).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let error = query
        .observe_session(&SessionId::from("read-failed"), Default::default())
        .await
        .err()
        .unwrap();
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        SessionQueryErrorCode::PersistenceFailed.as_str()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn inspect_session_reads_a_cold_session_without_attaching_it() {
    let state = FakeState::new("inspect-cold", Vec::new());
    let (kernel, store) = mounted(state).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let id = query
        .inspect_session(
            &SessionId::from("inspect-cold"),
            |session| session.id().clone(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(id, SessionId::from("inspect-cold"));
    assert!(store.get(&id).is_none());
    kernel.shutdown().await;
}

async fn assert_inspection_release(after_panic: bool) {
    let state = FakeState::new("inspect-first", Vec::new());
    let (kernel, _store) = mounted_with_config(
        state.clone(),
        Config {
            prepared_session_cache_size: 1,
            ..Default::default()
        },
    )
    .await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    if after_panic {
        let first = SessionId::from("inspect-first");
        let pending = AssertUnwindSafe(query.inspect_session(
            &first,
            |_| -> () { panic!("getter failed") },
            None,
        ))
        .catch_unwind();
        assert!(pending.await.is_err());
    } else {
        assert_eq!(
            query
                .inspect_session(
                    &SessionId::from("inspect-first"),
                    |session| session.id().clone(),
                    None,
                )
                .await
                .unwrap(),
            SessionId::from("inspect-first")
        );
    }
    state.header.lock().unwrap().id = SessionId::from("inspect-second");
    drop(
        query
            .observe_session(
                &SessionId::from("inspect-second"),
                SessionObservationOptions {
                    projection_mode: ProjectionMode::None,
                    ..Default::default()
                },
            )
            .await
            .unwrap(),
    );
    state.header.lock().unwrap().id = SessionId::from("inspect-first");
    drop(
        query
            .observe_session(
                &SessionId::from("inspect-first"),
                SessionObservationOptions {
                    projection_mode: ProjectionMode::None,
                    ..Default::default()
                },
            )
            .await
            .unwrap(),
    );
    assert_eq!(state.counts.reads.load(Ordering::SeqCst), 3);
    kernel.shutdown().await;
}

#[tokio::test]
async fn inspect_session_releases_a_cold_lease_after_return() {
    assert_inspection_release(false).await;
}

#[tokio::test]
async fn inspect_session_releases_a_cold_lease_after_panic() {
    assert_inspection_release(true).await;
}

#[tokio::test]
async fn observation_maps_stat_absence_to_not_found() {
    let state = FakeState::new("stored", Vec::new());
    state.missing_stat.store(true, Ordering::SeqCst);
    let (kernel, _store) = mounted(state).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let error = match query
        .observe_session(&SessionId::from("stored"), Default::default())
        .await
    {
        Ok(_) => panic!("missing stat unexpectedly observed"),
        Err(error) => error,
    };
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        SessionQueryErrorCode::SessionNotFound.as_str()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn corpus_lists_persisted_sessions_and_reads_them_without_attachment() {
    let state = FakeState::new("stored", Vec::new());
    let (kernel, store) = mounted(state).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let listed = query.list_sessions(None).await.unwrap();
    assert_eq!(listed[0].header.id, SessionId::from("stored"));
    assert!(!listed[0].live && listed[0].persisted);
    assert!(
        query
            .read_session(&SessionId::from("stored"))
            .await
            .unwrap()
            .events
            .is_empty()
    );
    assert!(store.get(&SessionId::from("stored")).is_none());
    kernel.shutdown().await;
}

#[tokio::test]
async fn known_live_reads_do_not_consult_unhealthy_persistence() {
    let state = FakeState::new("stored", Vec::new());
    state.fail_list.store(true, Ordering::SeqCst);
    let (kernel, store) = mounted(state.clone()).await;
    let live = store
        .create(
            &kernel.root(),
            Some(SessionId::from("live")),
            Default::default(),
        )
        .unwrap();
    let query = kernel.root().get::<SessionQuery>().unwrap();
    assert!(query.read_session(live.id()).await.is_ok());
    assert_eq!(state.counts.lists.load(Ordering::SeqCst), 0);
    kernel.shutdown().await;
}

#[tokio::test]
async fn empty_text_filter_loads_and_validates_persisted_surface_first() {
    let event = Arc::new(SessionEvent::from_value(json!({
        "type":"user/message", "seq":0, "time":1,
        "data":{"id":"user-bad","role":"user","content":[{"type":"text","text":"bad"}],"source":{"kind":"user"}},
        "surfaceOp":{"op":"replace","startSeq":9,"endSeq":9}
    })));
    let state = FakeState::new("stored", vec![event]);
    let (kernel, _store) = mounted(state.clone()).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let error = query
        .filter_events(
            &SessionId::from("stored"),
            &[SessionEventResultFilter::Text("  ".into())],
        )
        .await
        .unwrap_err();
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        SessionQueryErrorCode::InvalidSurface.as_str()
    );
    assert!(state.counts.lists.load(Ordering::SeqCst) > 0);
    assert!(state.counts.opens.load(Ordering::SeqCst) > 0);
    assert!(state.counts.reads.load(Ordering::SeqCst) > 0);
    kernel.shutdown().await;
}

#[tokio::test]
async fn persistence_listing_failures_use_query_taxonomy() {
    let state = FakeState::new("stored", Vec::new());
    state.fail_list.store(true, Ordering::SeqCst);
    let (kernel, _store) = mounted(state).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let error = query.list_sessions(None).await.unwrap_err();
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        SessionQueryErrorCode::PersistenceFailed.as_str()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn title_batches_deduplicate_list_once_and_isolate_missing_ids() {
    let state = FakeState::new("stored", Vec::new());
    let (kernel, _store) = mounted(state.clone()).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let results = query
        .read_title_snapshots(
            &[
                SessionId::from("stored"),
                SessionId::from("stored"),
                SessionId::from("missing"),
            ],
            None,
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 2);
    assert!(matches!(
        results[0],
        SessionTitleObservationResult::Fulfilled { .. }
    ));
    assert!(matches!(
        results[1],
        SessionTitleObservationResult::Rejected { .. }
    ));
    assert_eq!(state.counts.lists.load(Ordering::SeqCst), 1);
    kernel.shutdown().await;
}

#[tokio::test]
async fn title_batch_preserves_live_result_when_persistence_listing_fails() {
    let state = FakeState::new("stored", Vec::new());
    state.fail_list.store(true, Ordering::SeqCst);
    let (kernel, store) = mounted(state).await;
    let live = store
        .create(
            &kernel.root(),
            Some(SessionId::from("live")),
            Default::default(),
        )
        .unwrap();
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let results = query
        .read_title_snapshots(&[live.id().clone(), SessionId::from("missing")], None)
        .await
        .unwrap();
    assert!(matches!(
        results[0],
        SessionTitleObservationResult::Fulfilled { .. }
    ));
    assert!(matches!(
        results[1],
        SessionTitleObservationResult::Rejected { .. }
    ));
    kernel.shutdown().await;
}

fn multi_state(count: usize) -> (Arc<FakeState>, Vec<Arc<FakeState>>, Vec<SessionId>) {
    let states = (0..count)
        .map(|index| FakeState::new(&format!("title-{index}"), Vec::new()))
        .collect::<Vec<_>>();
    let ids = states
        .iter()
        .map(|state| state.header.lock().unwrap().id.clone())
        .collect::<Vec<_>>();
    let primary = states[0].clone();
    *primary.additional.lock().unwrap() = states[1..].to_vec();
    (primary, states, ids)
}

#[tokio::test]
async fn title_batch_bounds_persisted_read_concurrency_and_preserves_order() {
    let (primary, states, ids) = multi_state(6);
    let gate = Gate::new();
    for state in &states {
        *state.read_gate.lock().unwrap() = Some(gate.clone());
    }
    let (kernel, _store) = mounted_with_config(
        primary,
        Config {
            persisted_read_concurrency: 2,
            ..Default::default()
        },
    )
    .await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let requested = ids.clone();
    let task = tokio::spawn(async move { query.read_title_snapshots(&requested, None).await });
    for _ in 0..3 {
        gate.started.acquire_many(2).await.unwrap().forget();
        assert!(gate.started.try_acquire().is_err());
        gate.release.add_permits(2);
    }
    let results = task.await.unwrap().unwrap();
    let actual = results
        .iter()
        .map(|result| match result {
            SessionTitleObservationResult::Fulfilled { session_id, .. }
            | SessionTitleObservationResult::Rejected { session_id, .. } => session_id.clone(),
        })
        .collect::<Vec<_>>();
    assert_eq!(actual, ids);
    kernel.shutdown().await;
}

#[tokio::test]
async fn title_batch_discards_each_full_log_before_dequeuing_the_next_read() {
    let marker = raw_event(
        "session/title",
        0,
        json!({"title":"First","messageSeqs":[],"source":{"kind":"fallback"}}),
    );
    let first = FakeState::new("title-release-first", vec![marker.clone()]);
    let second = FakeState::new("title-release-second", Vec::new());
    *first.additional.lock().unwrap() = vec![second.clone()];
    let weak_marker = Arc::downgrade(&marker);
    *second.on_read.lock().unwrap() = Some(Arc::new(move || {
        let marker = weak_marker
            .upgrade()
            .expect("marker remains in fake storage");
        assert_eq!(
            Arc::strong_count(&marker),
            3,
            "the first worker must release its detached full-log clone before the next read"
        );
    }));
    let (kernel, _store) = mounted_with_config(
        first,
        Config {
            persisted_read_concurrency: 1,
            ..Default::default()
        },
    )
    .await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let results = query
        .read_title_snapshots(
            &[
                SessionId::from("title-release-first"),
                SessionId::from("title-release-second"),
            ],
            None,
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 2);
    assert!(
        results
            .iter()
            .all(|result| matches!(result, SessionTitleObservationResult::Fulfilled { .. }))
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn title_batch_passes_signal_into_a_stalled_read() {
    let state = FakeState::new("title-stalled-read", Vec::new());
    state.abort_read_with_signal.store(true, Ordering::SeqCst);
    let (kernel, _store) = mounted(state.clone()).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let controller = AbortController::new();
    let signal = controller.signal();
    let task = tokio::spawn(async move {
        query
            .read_title_snapshots(&[SessionId::from("title-stalled-read")], Some(signal))
            .await
    });
    while state.counts.reads.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    assert_eq!(state.read_signals.load(Ordering::SeqCst), 1);
    controller.abort_with(std::io::Error::other("title read deadline"));
    let error = task.await.unwrap().unwrap_err();
    let expected = controller.signal().reason().unwrap();
    assert!(
        hoh_error::SharedError::of(error.as_ref())
            .unwrap()
            .ptr_eq(&expected)
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn title_batch_drains_started_reads_after_cancel_without_starting_queue() {
    let (primary, states, ids) = multi_state(6);
    let gate = Gate::new();
    for state in &states {
        *state.read_gate.lock().unwrap() = Some(gate.clone());
    }
    let (kernel, _store) = mounted_with_config(
        primary,
        Config {
            persisted_read_concurrency: 2,
            ..Default::default()
        },
    )
    .await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let controller = AbortController::new();
    let signal = controller.signal();
    let task = tokio::spawn(async move { query.read_title_snapshots(&ids, Some(signal)).await });
    gate.started.acquire_many(2).await.unwrap().forget();
    controller.abort_with(std::io::Error::other("cancel title queue"));
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    gate.release.add_permits(2);
    let error = task.await.unwrap().unwrap_err();
    let expected = controller.signal().reason().unwrap();
    assert!(
        hoh_error::SharedError::of(error.as_ref())
            .unwrap()
            .ptr_eq(&expected)
    );
    assert_eq!(
        states
            .iter()
            .map(|state| state.counts.reads.load(Ordering::SeqCst))
            .sum::<usize>(),
        2
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn title_batch_stalled_listing_preserves_abort_reason_after_cleanup() {
    let state = FakeState::new("title-list", Vec::new());
    let gate = Gate::new();
    *state.list_gate.lock().unwrap() = Some(gate.clone());
    let (kernel, _store) = mounted(state).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let controller = AbortController::new();
    let signal = controller.signal();
    let task = tokio::spawn(async move {
        query
            .read_title_snapshots(&[SessionId::from("title-list")], Some(signal))
            .await
    });
    gate.started.acquire().await.unwrap().forget();
    controller.abort_with(std::io::Error::other("cancel title listing"));
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    gate.release.add_permits(1);
    let error = task.await.unwrap().unwrap_err();
    let expected = controller.signal().reason().unwrap();
    assert!(
        hoh_error::SharedError::of(error.as_ref())
            .unwrap()
            .ptr_eq(&expected)
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn title_batch_passes_signal_into_a_stalled_listing() {
    let state = FakeState::new("title-stalled-list", Vec::new());
    state.abort_list_with_signal.store(true, Ordering::SeqCst);
    let (kernel, _store) = mounted(state.clone()).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let controller = AbortController::new();
    let signal = controller.signal();
    let task = tokio::spawn(async move {
        query
            .read_title_snapshots(&[SessionId::from("title-stalled-list")], Some(signal))
            .await
    });
    while state.counts.lists.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    assert_eq!(state.list_signals.load(Ordering::SeqCst), 1);
    assert_eq!(state.counts.reads.load(Ordering::SeqCst), 0);
    controller.abort_with(std::io::Error::other("title listing deadline"));
    let error = task.await.unwrap().unwrap_err();
    let expected = controller.signal().reason().unwrap();
    assert!(
        hoh_error::SharedError::of(error.as_ref())
            .unwrap()
            .ptr_eq(&expected)
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn title_batch_isolates_read_failure_and_prefers_late_live_attachment() {
    let (primary, states, ids) = multi_state(2);
    states[1].fail_read.store(true, Ordering::SeqCst);
    let (kernel, store) = mounted(primary).await;
    let root = kernel.root();
    let attach_store = store.clone();
    let attached_id = ids[0].clone();
    *states[0].on_read.lock().unwrap() = Some(Arc::new(move || {
        attach_store
            .create(
                &root,
                Some(attached_id.clone()),
                CreateSessionOptions {
                    meta: Some(json!({"createdAt":1})),
                    ..Default::default()
                },
            )
            .unwrap();
    }));
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let results = query.read_title_snapshots(&ids, None).await.unwrap();
    assert!(matches!(
        results[0],
        SessionTitleObservationResult::Fulfilled { .. }
    ));
    assert!(matches!(
        results[1],
        SessionTitleObservationResult::Rejected { .. }
    ));
    kernel.shutdown().await;
}

#[tokio::test]
async fn title_batch_prefers_a_live_owner_attached_after_a_listing_miss() {
    let state = FakeState::new("title-listed", Vec::new());
    let (kernel, store) = mounted(state.clone()).await;
    let root = kernel.root();
    let late_store = store.clone();
    *state.on_list.lock().unwrap() = Some(Arc::new(move || {
        late_store
            .create(
                &root,
                Some(SessionId::from("title-late-after-list")),
                CreateSessionOptions {
                    meta: Some(json!({"createdAt":2})),
                    ..Default::default()
                },
            )
            .unwrap();
    }));
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let results = query
        .read_title_snapshots(
            &[
                SessionId::from("title-listed"),
                SessionId::from("title-late-after-list"),
            ],
            None,
        )
        .await
        .unwrap();
    assert!(matches!(
        results[0],
        SessionTitleObservationResult::Fulfilled { .. }
    ));
    assert!(matches!(
        results[1],
        SessionTitleObservationResult::Fulfilled { .. }
    ));
    assert_eq!(state.counts.reads.load(Ordering::SeqCst), 1);
    kernel.shutdown().await;
}

#[tokio::test]
async fn exact_corpus_pre_abort_preserves_reason_identity() {
    let state = FakeState::new("stored", Vec::new());
    let (kernel, _store) = mounted(state).await;
    let controller = AbortController::new();
    controller.abort_with(std::io::Error::other("stop now"));
    let signal = controller.signal();
    let expected = signal.reason().unwrap();
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let error = query.list_sessions(Some(signal)).await.unwrap_err();
    assert!(
        hoh_error::SharedError::of(error.as_ref())
            .unwrap()
            .ptr_eq(&expected)
    );
    kernel.shutdown().await;
}

#[derive(Clone, Copy)]
enum CancelOperation {
    ListSessions,
    FilterSessions,
    TraceSession,
    TraceEvent,
    ReadEvent,
}

async fn assert_in_flight_cancellation_waits_for_backend(operation: CancelOperation) {
    let event = Arc::new(SessionEvent::from_value(json!({
        "type":"user/message", "seq":0, "time":1,
        "data":{"id":"user-cancel","role":"user","content":[{"type":"text","text":"x"}],"source":{"kind":"user"}},
        "surfaceOp":"append"
    })));
    let state = FakeState::new("cancelled", vec![event]);
    let gate = Gate::new();
    if matches!(
        operation,
        CancelOperation::TraceEvent | CancelOperation::ReadEvent
    ) {
        *state.read_gate.lock().unwrap() = Some(gate.clone());
    } else {
        *state.list_gate.lock().unwrap() = Some(gate.clone());
    }
    let (kernel, _store) = mounted(state).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let controller = AbortController::new();
    let signal = controller.signal();
    let task = tokio::spawn(async move {
        match operation {
            CancelOperation::ListSessions => query.list_sessions(Some(signal)).await.map(|_| ()),
            CancelOperation::FilterSessions => {
                query.filter_sessions(&[], Some(signal)).await.map(|_| ())
            }
            CancelOperation::TraceSession => query
                .trace_session(&SessionId::from("cancelled"), Some(signal))
                .await
                .map(|_| ()),
            CancelOperation::TraceEvent => query
                .trace_event(
                    &SessionEventTraceRequest {
                        session_id: SessionId::from("cancelled"),
                        seq: hoh_session::SessionSeq::new(0).unwrap(),
                    },
                    Some(signal),
                )
                .await
                .map(|_| ()),
            CancelOperation::ReadEvent => query
                .read_event(
                    &SessionEventReadRequest {
                        session_id: SessionId::from("cancelled"),
                        seq: hoh_session::SessionSeq::new(0).unwrap(),
                        before: None,
                        after: None,
                    },
                    Some(signal),
                )
                .await
                .map(|_| ()),
        }
    });
    gate.started.acquire().await.unwrap().forget();
    controller.abort_with(std::io::Error::other("cancelled in flight"));
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    gate.release.add_permits(1);
    let error = task.await.unwrap().unwrap_err();
    let expected = controller.signal().reason().unwrap();
    assert!(
        hoh_error::SharedError::of(error.as_ref())
            .unwrap()
            .ptr_eq(&expected)
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn list_sessions_in_flight_cancellation_waits_for_backend_cleanup() {
    assert_in_flight_cancellation_waits_for_backend(CancelOperation::ListSessions).await;
}

#[tokio::test]
async fn filter_sessions_in_flight_cancellation_waits_for_backend_cleanup() {
    assert_in_flight_cancellation_waits_for_backend(CancelOperation::FilterSessions).await;
}

#[tokio::test]
async fn trace_session_in_flight_cancellation_waits_for_backend_cleanup() {
    assert_in_flight_cancellation_waits_for_backend(CancelOperation::TraceSession).await;
}

#[tokio::test]
async fn trace_event_in_flight_cancellation_waits_for_backend_cleanup() {
    assert_in_flight_cancellation_waits_for_backend(CancelOperation::TraceEvent).await;
}

#[tokio::test]
async fn read_event_in_flight_cancellation_waits_for_backend_cleanup() {
    assert_in_flight_cancellation_waits_for_backend(CancelOperation::ReadEvent).await;
}

#[tokio::test]
async fn observation_pre_abort_maps_reason_to_aborted_harness_error() {
    let state = FakeState::new("stored", Vec::new());
    let (kernel, _store) = mounted(state).await;
    let controller = AbortController::new();
    controller.abort_with(std::io::Error::other("stop observation"));
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let error = match query
        .observe_session(
            &SessionId::from("stored"),
            SessionObservationOptions {
                signal: Some(controller.signal()),
                projection_mode: ProjectionMode::None,
            },
        )
        .await
    {
        Ok(_) => panic!("aborted observation unexpectedly succeeded"),
        Err(error) => error,
    };
    let error = hoh_error::HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(error.code(), SessionQueryErrorCode::Aborted.as_str());
    assert!(error.cause().is_some());
    kernel.shutdown().await;
}

#[tokio::test]
async fn observation_maps_stat_and_post_read_abort_stages_to_aborted_error() {
    let state = FakeState::new("abort-stages", Vec::new());
    let (kernel, _store) = mounted(state.clone()).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();

    let stat_controller = Arc::new(AbortController::new());
    let abort_stat = stat_controller.clone();
    *state.on_stat.lock().unwrap() = Some(Arc::new(move || {
        abort_stat.abort_with(std::io::Error::other("aborted during stat"));
    }));
    let error = query
        .observe_session(
            &SessionId::from("abort-stages"),
            SessionObservationOptions {
                signal: Some(stat_controller.signal()),
                projection_mode: ProjectionMode::None,
            },
        )
        .await
        .err()
        .unwrap();
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        SessionQueryErrorCode::Aborted.as_str()
    );

    let read_controller = Arc::new(AbortController::new());
    let abort_read = read_controller.clone();
    *state.on_read.lock().unwrap() = Some(Arc::new(move || {
        abort_read.abort_with(std::io::Error::other("aborted after read"));
    }));
    let error = query
        .observe_session(
            &SessionId::from("abort-stages"),
            SessionObservationOptions {
                signal: Some(read_controller.signal()),
                projection_mode: ProjectionMode::None,
            },
        )
        .await
        .err()
        .unwrap();
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        SessionQueryErrorCode::Aborted.as_str()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn read_session_rejects_a_corrupt_persisted_seed() {
    let state = FakeState::new(
        "corrupt-seed",
        vec![raw_event(
            "user/message",
            5,
            json!({"id":"bad","role":"user","content":[],"source":{"kind":"user"}}),
        )],
    );
    let (kernel, _store) = mounted(state).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let error = query
        .read_session(&SessionId::from("corrupt-seed"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("seq 5"));
    kernel.shutdown().await;
}

#[tokio::test]
async fn read_surface_rejects_a_malformed_persisted_surface() {
    let state = FakeState::new(
        "bad-surface",
        vec![Arc::new(SessionEvent::from_value(json!({
            "type":"user/message", "seq":0, "time":1,
            "data":{"id":"bad","role":"user","content":[],"source":{"kind":"user"}},
            "surfaceOp":{"op":"replace","startSeq":9,"endSeq":9}
        })))],
    );
    let (kernel, _store) = mounted(state).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let error = query
        .read_surface(&SessionId::from("bad-surface"))
        .await
        .unwrap_err();
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        SessionQueryErrorCode::InvalidSurface.as_str()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn trace_event_inspects_persisted_once_then_prefers_live() {
    let event = Arc::new(SessionEvent::from_value(json!({
        "type":"user/message", "seq":0, "time":1,
        "data":{"id":"user-trace","role":"user","content":[{"type":"text","text":"persisted"}],"source":{"kind":"user"}},
        "surfaceOp":"append"
    })));
    let state = FakeState::new("trace-source", vec![event]);
    let (kernel, store) = mounted(state.clone()).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let request = SessionEventTraceRequest {
        session_id: SessionId::from("trace-source"),
        seq: hoh_session::SessionSeq::new(0).unwrap(),
    };
    query.trace_event(&request, None).await.unwrap();
    assert_eq!(state.counts.lists.load(Ordering::SeqCst), 1);
    assert_eq!(state.counts.reads.load(Ordering::SeqCst), 1);

    let live = store
        .create(
            &kernel.root(),
            Some(SessionId::from("trace-source")),
            CreateSessionOptions {
                meta: Some(json!({"createdAt":1})),
                ..Default::default()
            },
        )
        .unwrap();
    live.append(
        "user/message",
        json!({"id":"user-live","role":"user","content":[{"type":"text","text":"live"}],"source":{"kind":"user"}}),
        Some(hoh_session::SurfaceIntent::append()),
    )
    .unwrap();
    state.fail_list.store(true, Ordering::SeqCst);
    state.fail_read.store(true, Ordering::SeqCst);
    query.trace_event(&request, None).await.unwrap();
    assert_eq!(state.counts.lists.load(Ordering::SeqCst), 1);
    assert_eq!(state.counts.reads.load(Ordering::SeqCst), 1);
    kernel.shutdown().await;
}

#[tokio::test]
async fn list_and_trace_reject_surface_op_on_non_surface_event() {
    let events = vec![
        Arc::new(SessionEvent::from_value(json!({
            "type":"turn/start", "seq":0, "time":1, "data":{"turn":1}, "surfaceOp":"append"
        }))),
        raw_event(
            "turn/end",
            1,
            json!({"turn":1,"reason":{"kind":"completed"}}),
        ),
    ];
    let state = FakeState::new("non-surface-op", events);
    let (kernel, _store) = mounted(state).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    for error in [
        query
            .list_events(&SessionId::from("non-surface-op"))
            .await
            .unwrap_err(),
        query
            .trace_event(
                &SessionEventTraceRequest {
                    session_id: SessionId::from("non-surface-op"),
                    seq: hoh_session::SessionSeq::new(0).unwrap(),
                },
                None,
            )
            .await
            .unwrap_err(),
    ] {
        assert_eq!(
            hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
            SessionQueryErrorCode::InvalidSurface.as_str()
        );
    }
    kernel.shutdown().await;
}

#[tokio::test]
async fn prepared_cache_evicts_oldest_unpinned_entry_at_capacity() {
    let state = FakeState::new("a", Vec::new());
    let config = Config {
        prepared_session_cache_size: 1,
        ..Default::default()
    };
    let (kernel, _store) = mounted_with_config(state.clone(), config).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    drop(
        query
            .observe_session(
                &SessionId::from("a"),
                SessionObservationOptions {
                    projection_mode: ProjectionMode::None,
                    ..Default::default()
                },
            )
            .await
            .unwrap(),
    );
    state.header.lock().unwrap().id = SessionId::from("b");
    drop(
        query
            .observe_session(
                &SessionId::from("b"),
                SessionObservationOptions {
                    projection_mode: ProjectionMode::None,
                    ..Default::default()
                },
            )
            .await
            .unwrap(),
    );
    state.header.lock().unwrap().id = SessionId::from("a");
    drop(
        query
            .observe_session(
                &SessionId::from("a"),
                SessionObservationOptions {
                    projection_mode: ProjectionMode::None,
                    ..Default::default()
                },
            )
            .await
            .unwrap(),
    );
    assert_eq!(state.counts.reads.load(Ordering::SeqCst), 3);
    kernel.shutdown().await;
}

#[tokio::test]
async fn prepared_cache_never_evicts_a_pinned_entry() {
    let state = FakeState::new("a", Vec::new());
    let config = Config {
        prepared_session_cache_size: 1,
        ..Default::default()
    };
    let (kernel, _store) = mounted_with_config(state.clone(), config).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let mut pinned = query
        .observe_session(
            &SessionId::from("a"),
            SessionObservationOptions {
                projection_mode: ProjectionMode::None,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    state.header.lock().unwrap().id = SessionId::from("b");
    drop(
        query
            .observe_session(
                &SessionId::from("b"),
                SessionObservationOptions {
                    projection_mode: ProjectionMode::None,
                    ..Default::default()
                },
            )
            .await
            .unwrap(),
    );
    state.header.lock().unwrap().id = SessionId::from("a");
    drop(
        query
            .observe_session(
                &SessionId::from("a"),
                SessionObservationOptions {
                    projection_mode: ProjectionMode::None,
                    ..Default::default()
                },
            )
            .await
            .unwrap(),
    );
    assert_eq!(state.counts.reads.load(Ordering::SeqCst), 2);
    pinned.dispose();
    kernel.shutdown().await;
}

#[tokio::test]
async fn prepared_cache_sweeps_released_entries_until_it_reaches_capacity() {
    let state = FakeState::new("a", Vec::new());
    let config = Config {
        prepared_session_cache_size: 1,
        ..Default::default()
    };
    let (kernel, _store) = mounted_with_config(state.clone(), config).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let mut a = query
        .observe_session(
            &SessionId::from("a"),
            SessionObservationOptions {
                projection_mode: ProjectionMode::None,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    state.header.lock().unwrap().id = SessionId::from("b");
    let mut b = query
        .observe_session(
            &SessionId::from("b"),
            SessionObservationOptions {
                projection_mode: ProjectionMode::None,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    state.header.lock().unwrap().id = SessionId::from("c");
    let mut c = query
        .observe_session(
            &SessionId::from("c"),
            SessionObservationOptions {
                projection_mode: ProjectionMode::None,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    b.dispose();
    c.dispose();
    state.header.lock().unwrap().id = SessionId::from("b");
    drop(
        query
            .observe_session(
                &SessionId::from("b"),
                SessionObservationOptions {
                    projection_mode: ProjectionMode::None,
                    ..Default::default()
                },
            )
            .await
            .unwrap(),
    );
    assert_eq!(state.counts.reads.load(Ordering::SeqCst), 4);
    a.dispose();
    kernel.shutdown().await;
}

#[tokio::test]
async fn prepared_cache_discards_a_revision_from_a_replaced_persistence_instance() {
    let state = FakeState::new("stored", Vec::new());
    let kernel = Kernel::start();
    let store = SessionStore::new();
    kernel
        .root()
        .provide::<Sessions>(Arc::new(store.clone()))
        .await
        .unwrap();
    let first: Arc<dyn SessionPersistence> = Arc::new(FakePersistence::new(state.clone()));
    let first_binding = kernel
        .root()
        .provide::<SessionPersistenceService>(first)
        .await
        .unwrap();
    let sessions = kernel.root().get::<Sessions>().unwrap();
    mount_session_query(
        &kernel.root(),
        sessions,
        Arc::new(SearchDisabled),
        Config::default(),
    )
    .await
    .unwrap();
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    let query = kernel.root().get::<SessionQuery>().unwrap();
    drop(
        query
            .observe_session(
                &SessionId::from("stored"),
                SessionObservationOptions {
                    projection_mode: ProjectionMode::None,
                    ..Default::default()
                },
            )
            .await
            .unwrap(),
    );
    first_binding.dispose().await;
    let second: Arc<dyn SessionPersistence> = Arc::new(FakePersistence::new(state.clone()));
    kernel
        .root()
        .provide::<SessionPersistenceService>(second)
        .await
        .unwrap();
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    drop(
        query
            .observe_session(
                &SessionId::from("stored"),
                SessionObservationOptions {
                    projection_mode: ProjectionMode::None,
                    ..Default::default()
                },
            )
            .await
            .unwrap(),
    );
    assert_eq!(state.counts.reads.load(Ordering::SeqCst), 2);
    kernel.shutdown().await;
}

#[tokio::test]
async fn listing_preserves_format_status_and_live_precedence_and_detects_conflicts() {
    let state = FakeState::new("shared", Vec::new());
    *state.format_status.lock().unwrap() = Some(SessionFormatStatus::MigrationRequired);
    let (kernel, store) = mounted(state.clone()).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let persisted = query.list_sessions(None).await.unwrap();
    assert_eq!(
        persisted[0].format_status,
        Some(SessionFormatStatus::MigrationRequired)
    );

    store
        .create(
            &kernel.root(),
            Some(SessionId::from("shared")),
            hoh_session::CreateSessionOptions {
                meta: Some(json!({"createdAt": 1})),
                ..Default::default()
            },
        )
        .unwrap();
    let live = query.list_sessions(None).await.unwrap();
    assert!(live[0].live && live[0].persisted);
    assert_eq!(live[0].format_status, Some(SessionFormatStatus::Current));

    state.header.lock().unwrap().cwd = Some("/conflict".into());
    let error = query.list_sessions(None).await.unwrap_err();
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        SessionQueryErrorCode::SourceConflict.as_str()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn title_batch_shares_one_listing_failure_across_rejected_ids() {
    let state = FakeState::new("stored", Vec::new());
    state.fail_list.store(true, Ordering::SeqCst);
    let (kernel, _store) = mounted(state).await;
    let query = kernel.root().get::<SessionQuery>().unwrap();
    let results = query
        .read_title_snapshots(
            &[SessionId::from("missing-a"), SessionId::from("missing-b")],
            None,
        )
        .await
        .unwrap();
    let reasons = results
        .iter()
        .map(|result| match result {
            SessionTitleObservationResult::Rejected { reason, .. } => {
                hoh_error::SharedError::of(reason.as_ref()).unwrap()
            }
            SessionTitleObservationResult::Fulfilled { .. } => panic!("missing id was fulfilled"),
        })
        .collect::<Vec<_>>();
    assert!(reasons[0].ptr_eq(reasons[1]));
    assert_eq!(
        reasons[0].to_string(),
        "session persistence listing failed: list failed"
    );
    kernel.shutdown().await;
}
