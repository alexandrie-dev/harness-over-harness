use futures_util::future::BoxFuture;
use hoh_error::BoxError;
use hoh_kernel::{Kernel, PluginDef};
use hoh_session::{
    CreateSessionOptions, PrepareSessionOptions, SessionId, SessionPlugin, SessionSeq, Sessions,
    SurfaceIntent,
};
use hoh_session::{SessionEvent, SessionHeader, SessionLogOffset, SessionSeedEventState};
use hoh_session_persistence::{
    SessionAccess, SessionHandle, SessionHandleAppendOptions, SessionHandleFlushOptions,
    SessionHandleReadOptions, SessionHandleReadResult, SessionPersistence,
    SessionPersistenceCreateOptions, SessionPersistenceIdentity, SessionPersistenceListOptions,
    SessionPersistenceOpenOptions, SessionPersistenceRevision, SessionPersistenceService,
    SessionPersistenceSnapshot, SessionPersistenceStatOptions,
};
use hoh_session_persistence_jsonl::JsonlSessionPersistencePlugin;
use hoh_session_query::{
    SessionEventMetadataFilter, SessionEventSearchRequest, SessionEventSurface, SessionQuery,
    SessionResultRange, SessionSearchExecContext, SessionSearchRequest,
};
use hoh_session_query_sqlite::SessionQuerySqlite;
use hoh_timeout::AbortController;
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone)]
struct FakeEntry {
    header: SessionHeader,
    revision: SessionPersistenceRevision,
    events: Vec<Arc<SessionEvent>>,
}

struct FakeHandle {
    entry: FakeEntry,
    read_started: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    read_gate: Arc<Mutex<Option<Arc<tokio::sync::Notify>>>>,
    failure_stage: Option<FailureStage>,
    failure_abort: Option<Arc<AbortController>>,
}

impl SessionHandle for FakeHandle {
    fn id(&self) -> &SessionId {
        &self.entry.header.id
    }

    fn header(&self) -> &SessionHeader {
        &self.entry.header
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
        _options: Option<&'a SessionHandleReadOptions>,
    ) -> BoxFuture<'a, Result<SessionHandleReadResult, BoxError>> {
        Box::pin(async move {
            if let Some(started) = self.read_started.lock().unwrap().take() {
                let _ = started.send(());
            }
            let gate = self.read_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                gate.notified().await;
            }
            if self.failure_stage == Some(FailureStage::Read) {
                if let Some(controller) = &self.failure_abort {
                    controller.abort();
                }
                return Err(Box::new(std::io::Error::other("offline")) as BoxError);
            }
            Ok(SessionHandleReadResult {
                event_state: SessionSeedEventState::Detached,
                events: self.entry.events.clone(),
            })
        })
    }

    fn append<'a>(
        &'a self,
        _events: &'a [Arc<SessionEvent>],
        _options: Option<&'a SessionHandleAppendOptions>,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async { Err(Box::new(std::io::Error::other("read only")) as BoxError) })
    }

    fn flush<'a>(
        &'a self,
        _options: Option<&'a SessionHandleFlushOptions>,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async { Ok(()) })
    }

    fn close(&self) -> BoxFuture<'_, Result<(), BoxError>> {
        Box::pin(async move {
            if self.failure_stage == Some(FailureStage::Close) {
                if let Some(controller) = &self.failure_abort {
                    controller.abort();
                }
                return Err(Box::new(std::io::Error::other("offline")) as BoxError);
            }
            Ok(())
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FailureStage {
    Open,
    Read,
    Close,
}

struct FakePersistence {
    identity: SessionPersistenceIdentity,
    entry: FakeEntry,
    abort_on_list: Mutex<Option<(usize, Arc<AbortController>)>>,
    fail_next_list: std::sync::atomic::AtomicBool,
    fail_after_gate: std::sync::atomic::AtomicBool,
    list_started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    list_gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
    list_started_call: AtomicUsize,
    list_gate_call: AtomicUsize,
    revision_per_list: std::sync::atomic::AtomicBool,
    duplicate_on_list_call: AtomicUsize,
    read_started: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    read_gate: Arc<Mutex<Option<Arc<tokio::sync::Notify>>>>,
    failure_stage: Option<FailureStage>,
    failure_abort: Option<Arc<AbortController>>,
    reads: Arc<AtomicUsize>,
    lists: AtomicUsize,
}

impl FakePersistence {
    fn new(text: &str) -> Self {
        let header = SessionHeader::new(SessionId::from("stored"), 1);
        let event = SessionEvent::from_value(json!({
            "type":"user/message",
            "seq":0,
            "time":1,
            "data":{
                "id":"stored-message",
                "role":"user",
                "content":[{"type":"text","text":text}],
                "source":{"kind":"user"}
            },
            "surfaceOp":"append"
        }));
        Self {
            identity: SessionPersistenceIdentity::new(),
            entry: FakeEntry {
                header,
                revision: SessionPersistenceRevision::from("same-revision"),
                events: vec![Arc::new(event)],
            },
            abort_on_list: Mutex::new(None),
            fail_next_list: std::sync::atomic::AtomicBool::new(false),
            fail_after_gate: std::sync::atomic::AtomicBool::new(false),
            list_started: Mutex::new(None),
            list_gate: Mutex::new(None),
            list_started_call: AtomicUsize::new(1),
            list_gate_call: AtomicUsize::new(1),
            revision_per_list: std::sync::atomic::AtomicBool::new(false),
            duplicate_on_list_call: AtomicUsize::new(0),
            read_started: Arc::new(Mutex::new(None)),
            read_gate: Arc::new(Mutex::new(None)),
            failure_stage: None,
            failure_abort: None,
            reads: Arc::new(AtomicUsize::new(0)),
            lists: AtomicUsize::new(0),
        }
    }

    fn aborting(text: &str, controller: Arc<AbortController>) -> Self {
        let value = Self::new(text);
        *value.abort_on_list.lock().unwrap() = Some((1, controller));
        value
    }

    fn aborting_on_second_list(text: &str, controller: Arc<AbortController>) -> Self {
        let value = Self::new(text);
        *value.abort_on_list.lock().unwrap() = Some((2, controller));
        value
    }

    fn failing_cold_read(stage: FailureStage, abort: Option<Arc<AbortController>>) -> Self {
        let mut value = Self::new("persisted needle");
        value.failure_stage = Some(stage);
        value.failure_abort = abort;
        value
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
        Box::pin(async { Err(Box::new(std::io::Error::other("unused create")) as BoxError) })
    }

    fn open<'a>(
        &'a self,
        id: &'a SessionId,
        _access: SessionAccess,
        _options: Option<&'a SessionPersistenceOpenOptions>,
    ) -> BoxFuture<'a, Result<Arc<dyn SessionHandle>, BoxError>> {
        Box::pin(async move {
            if id != &self.entry.header.id {
                return Err(Box::new(std::io::Error::other("not found")) as BoxError);
            }
            if self.failure_stage == Some(FailureStage::Open) {
                if let Some(controller) = &self.failure_abort {
                    controller.abort();
                }
                return Err(Box::new(std::io::Error::other("offline")) as BoxError);
            }
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(FakeHandle {
                entry: self.entry.clone(),
                read_started: self.read_started.clone(),
                read_gate: self.read_gate.clone(),
                failure_stage: self.failure_stage,
                failure_abort: self.failure_abort.clone(),
            }) as Arc<dyn SessionHandle>)
        })
    }

    fn flush(&self) -> BoxFuture<'_, Result<(), BoxError>> {
        Box::pin(async { Ok(()) })
    }

    fn stat<'a>(
        &'a self,
        _id: &'a SessionId,
        _options: Option<&'a SessionPersistenceStatOptions>,
    ) -> BoxFuture<'a, Result<Option<SessionPersistenceSnapshot>, BoxError>> {
        Box::pin(async { Ok(None) })
    }

    fn list<'a>(
        &'a self,
        _options: Option<&'a SessionPersistenceListOptions>,
    ) -> BoxFuture<'a, Result<Vec<SessionPersistenceSnapshot>, BoxError>> {
        Box::pin(async move {
            let call = self.lists.fetch_add(1, Ordering::SeqCst) + 1;
            if self.list_started_call.load(Ordering::SeqCst) == call
                && let Some(started) = self.list_started.lock().unwrap().take()
            {
                let _ = started.send(());
            }
            let gate = if self.list_gate_call.load(Ordering::SeqCst) == call {
                self.list_gate.lock().unwrap().take()
            } else {
                None
            };
            if let Some(gate) = gate {
                gate.notified().await;
            }
            if self.fail_after_gate.swap(false, Ordering::SeqCst) {
                return Err(Box::new(std::io::Error::other("stale list failure")) as BoxError);
            }
            if self.fail_next_list.swap(false, Ordering::SeqCst) {
                return Err(Box::new(std::io::Error::other("temporary list failure")) as BoxError);
            }
            let abort = {
                let mut configured = self.abort_on_list.lock().unwrap();
                if configured
                    .as_ref()
                    .is_some_and(|(target, _)| *target == call)
                {
                    configured.take().map(|(_, controller)| controller)
                } else {
                    None
                }
            };
            if let Some(controller) = abort {
                controller.abort();
                return Err(Box::new(std::io::Error::other("list cancelled")) as BoxError);
            }
            let snapshot = SessionPersistenceSnapshot {
                header: self.entry.header.clone(),
                format_status: None,
                revision: if self.revision_per_list.load(Ordering::SeqCst) {
                    SessionPersistenceRevision::from(format!("revision-{call}"))
                } else {
                    self.entry.revision.clone()
                },
                event_count: None,
                size_bytes: None,
                location: None,
            };
            if self.duplicate_on_list_call.load(Ordering::SeqCst) == call {
                Ok(vec![snapshot.clone(), snapshot])
            } else {
                Ok(vec![snapshot])
            }
        })
    }
}

async fn context(config: serde_json::Value) -> (Kernel, hoh_kernel::Context) {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    root.plugin::<SessionQuerySqlite>(config)
        .unwrap()
        .settled()
        .await
        .unwrap();
    (kernel, root)
}

fn request(query: &str) -> SessionSearchRequest {
    SessionSearchRequest {
        query: query.into(),
        session_filters: vec![],
        event_filters: vec![],
        limit: None,
        cursor: None,
    }
}

fn error_code(error: &BoxError) -> &str {
    hoh_error::HarnessError::of(error.as_ref())
        .expect("session query error")
        .code()
}

fn append_user(session: &hoh_session::Session, text: &str) {
    session
        .append(
            "user/message",
            json!({
                "id":format!("message-{}", session.seq().get()),
                "role":"user",
                "content":[{"type":"text","text":text}],
                "source":{"kind":"user"}
            }),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
}

fn stored_user_event(id: &str, text: &str) -> Arc<SessionEvent> {
    Arc::new(SessionEvent::from_value(json!({
        "type":"user/message",
        "seq":0,
        "time":1,
        "data":{
            "id":id,
            "role":"user",
            "content":[{"type":"text","text":text}],
            "source":{"kind":"user"}
        },
        "surfaceOp":"append"
    })))
}

#[test]
fn config_schema_matches_safe_integer_bounds_and_defaults() {
    let schema = PluginDef::of::<SessionQuerySqlite>().schema().clone();
    assert_eq!(schema["required"], json!(["path"]));
    assert_eq!(schema["properties"]["openAt"]["default"], "startup");
    assert_eq!(schema["properties"]["journalMode"]["default"], "wal");
    assert_eq!(schema["properties"]["defaultLimit"]["default"], 20);
    assert_eq!(schema["properties"]["maxLimit"]["default"], 100);
    assert_eq!(schema["properties"]["snippetChars"]["default"], 240);
    assert_eq!(
        schema["properties"]["persistedReadConcurrency"]["maximum"],
        9_007_199_254_740_991_u64
    );
    assert_eq!(
        schema["properties"]["preparedSessionCacheSize"]["maximum"],
        9_007_199_254_740_991_u64
    );
    assert!(serde_json::from_value::<hoh_session_query_sqlite::Config>(json!({})).is_err());
}

#[tokio::test]
async fn rejects_missing_path_and_invalid_direct_config_values() {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let missing = root.plugin::<SessionQuerySqlite>(json!({})).unwrap();
    assert!(missing.settled().await.is_err());
    for config in [
        json!({"path":":memory:","persistedReadConcurrency":0}),
        json!({"path":":memory:","preparedSessionCacheSize":0}),
        json!({"path":":memory:","defaultLimit":2,"maxLimit":1}),
        json!({"path":":memory:","openAt":"later"}),
    ] {
        let invalid = root.plugin::<SessionQuerySqlite>(config).unwrap();
        assert!(invalid.settled().await.is_err());
    }
    kernel.shutdown().await;
}

#[tokio::test]
async fn searches_unicode61_live_sessions_and_paginates() {
    let (kernel, root) =
        context(json!({"path": ":memory:", "defaultLimit": 1, "maxLimit": 5})).await;
    let sessions = root.get::<Sessions>().unwrap();
    for (id, text) in [("a", "An AI helper"), ("b", "AI peer")] {
        let session = sessions
            .create(
                &root,
                Some(SessionId::from(id)),
                CreateSessionOptions::default(),
            )
            .unwrap();
        session
            .append(
                "user/message",
                json!({"id":format!("message-{id}"),"role":"user","content":[{"type":"text","text":text}],"source":{"kind":"user"}}),
                Some(SurfaceIntent::append()),
            )
            .unwrap();
    }
    let query = root.get::<SessionQuery>().unwrap();
    let first = query
        .search_sessions(request("AI"), SessionSearchExecContext::default())
        .await
        .unwrap();
    assert_eq!(first.items.len(), 1);
    let second = query
        .search_sessions(
            SessionSearchRequest {
                cursor: first.next_cursor,
                ..request("AI")
            },
            SessionSearchExecContext::default(),
        )
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert_ne!(
        first.items[0].session.header.id,
        second.items[0].session.header.id
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn searches_events_and_preserves_header_fields() {
    let (kernel, root) = context(json!({"path": ":memory:", "snippetChars": 20})).await;
    let sessions = root.get::<Sessions>().unwrap();
    let session = sessions
        .create(
            &root,
            Some(SessionId::from("live")),
            CreateSessionOptions { meta: Some(json!({"cwd":"/work","createdAt":10,"delegationDepth":2,"agentPreset":"minimal"})), ..Default::default() },
        )
        .unwrap();
    session
        .append(
            "user/message",
            json!({"id":"message-live","role":"user","content":[{"type":"text","text":"An AI helper"}],"source":{"kind":"user"}}),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    let page = root
        .get::<SessionQuery>()
        .unwrap()
        .search_events(
            SessionEventSearchRequest {
                session_id: session.id().clone(),
                query: "AI".into(),
                filters: vec![],
                limit: None,
                cursor: None,
            },
            SessionSearchExecContext::default(),
        )
        .await
        .unwrap();
    assert_eq!(page.session.cwd.as_deref(), Some("/work"));
    assert_eq!(page.session.delegation_depth, Some(2));
    assert_eq!(page.session.agent_preset.as_deref(), Some("minimal"));
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].snippet, "An AI helper");
    kernel.shutdown().await;
}

#[tokio::test]
async fn never_mode_leaves_exact_reads_available() {
    let (kernel, root) = context(json!({"path": ":memory:", "openAt": "never"})).await;
    let query = root.get::<SessionQuery>().unwrap();
    let error = query
        .search_sessions(request("needle"), SessionSearchExecContext::default())
        .await
        .unwrap_err();
    assert_eq!(
        hoh_error::HarnessError::of(&*error).unwrap().code(),
        "SESSION_QUERY_SEARCH_DISABLED"
    );
    assert!(query.list_sessions(None).await.unwrap().is_empty());
    kernel.shutdown().await;
}

#[tokio::test]
async fn first_search_mode_defers_open_and_reuses_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deferred/search.db");
    let (kernel, root) = context(json!({"path":path,"openAt":"first-search"})).await;
    assert!(!path.exists());
    let query = root.get::<SessionQuery>().unwrap();
    assert!(
        query
            .search_sessions(request("needle"), SessionSearchExecContext::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert!(path.exists());
    assert!(
        query
            .search_sessions(request("needle"), SessionSearchExecContext::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn first_search_mode_disposes_without_ever_opening_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("unopened/search.db");
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let fiber = root
        .plugin::<SessionQuerySqlite>(json!({"path":path,"openAt":"first-search"}))
        .unwrap();
    fiber.settled().await.unwrap();
    assert!(!path.exists());
    fiber.dispose().await;
    assert!(!path.exists());
    kernel.shutdown().await;
}

#[tokio::test]
async fn startup_mode_opens_the_database_before_plugin_settlement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("startup/search.db");
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    root.plugin::<SessionQuerySqlite>(json!({"path":path}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    assert!(path.exists());
    kernel.shutdown().await;
}

#[tokio::test]
async fn concurrent_first_searches_share_one_initialized_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("concurrent/search.db");
    let (kernel, root) = context(json!({"path":path,"openAt":"first-search"})).await;
    let query = root.get::<SessionQuery>().unwrap();
    let (first, second) = tokio::join!(
        query.search_sessions(request("first"), SessionSearchExecContext::default()),
        query.search_sessions(request("second"), SessionSearchExecContext::default())
    );
    assert!(first.unwrap().items.is_empty());
    assert!(second.unwrap().items.is_empty());
    assert!(path.exists());
    kernel.shutdown().await;
}

#[tokio::test]
async fn excludes_assistant_reasoning_but_indexes_visible_answer_text() {
    let (kernel, root) = context(json!({"path":":memory:"})).await;
    let session = root
        .get::<Sessions>()
        .unwrap()
        .create(
            &root,
            Some(SessionId::from("reasoning")),
            CreateSessionOptions::default(),
        )
        .unwrap();
    session
        .append(
            "assistant/message",
            json!({
                "stream":[],
                "turn":1,
                "step":1,
                "message":{
                    "id":"assistant-message",
                    "role":"assistant",
                    "content":[
                        {"type":"reasoning","text":"private-chain-marker"},
                        {"type":"text","text":"visible-answer-marker"}
                    ],
                    "source":{"kind":"model","provider":"mock","model":"mock"}
                }
            }),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    let query = root.get::<SessionQuery>().unwrap();
    assert!(
        query
            .search_sessions(
                request("private-chain-marker"),
                SessionSearchExecContext::default(),
            )
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let visible = query
        .search_sessions(
            request("visible-answer-marker"),
            SessionSearchExecContext::default(),
        )
        .await
        .unwrap();
    assert_eq!(visible.items.len(), 1);
    assert_eq!(visible.items[0].best_match.snippet, "visible-answer-marker");
    kernel.shutdown().await;
}

#[tokio::test]
async fn treats_fts_syntax_as_literal_and_bounds_unicode_snippets() {
    let (kernel, root) = context(json!({
        "path":":memory:","defaultLimit":10,"maxLimit":10,"snippetChars":14
    }))
    .await;
    let sessions = root.get::<Sessions>().unwrap();
    for (id, text) in [
        ("operator", "needle OR absent"),
        ("only", "needle only"),
        ("quote", "say \"needle\" exactly"),
        ("accent", "long long long—café,\nnext value"),
    ] {
        let session = sessions
            .create(
                &root,
                Some(SessionId::from(id)),
                CreateSessionOptions::default(),
            )
            .unwrap();
        append_user(&session, text);
    }
    let query = root.get::<SessionQuery>().unwrap();
    let operator = query
        .search_sessions(
            request("needle OR absent"),
            SessionSearchExecContext::default(),
        )
        .await
        .unwrap();
    assert_eq!(operator.items[0].session.header.id.as_str(), "operator");
    let quoted = query
        .search_sessions(
            request("say \"needle\""),
            SessionSearchExecContext::default(),
        )
        .await
        .unwrap();
    assert_eq!(quoted.items[0].session.header.id.as_str(), "quote");
    assert!(
        query
            .search_sessions(request("*"), SessionSearchExecContext::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let accent = query
        .search_events(
            SessionEventSearchRequest {
                session_id: SessionId::from("accent"),
                query: "CAFE".into(),
                filters: vec![],
                limit: None,
                cursor: None,
            },
            SessionSearchExecContext::default(),
        )
        .await
        .unwrap();
    assert!(accent.items[0].snippet.contains("café"));
    assert!(accent.items[0].snippet.contains('—'));
    assert!(!accent.items[0].snippet.contains('\n'));
    assert!(accent.items[0].snippet.chars().count() <= 14);
    kernel.shutdown().await;
}

#[tokio::test]
async fn searches_all_surfaces_and_applies_event_metadata_before_ranking() {
    let (kernel, root) = context(json!({"path":":memory:","defaultLimit":10})).await;
    let session = root
        .get::<Sessions>()
        .unwrap()
        .create(
            &root,
            Some(SessionId::from("surfaces")),
            CreateSessionOptions::default(),
        )
        .unwrap();
    append_user(&session, "needle original");
    session
        .append(
            "assistant/attempt",
            json!({"turn":1,"step":1,"stream":[{"type":"text-chunks","time0":11,"index":0,"dt":[],"texts":["needle raw"]}]}),
            None,
        )
        .unwrap();
    session
        .append(
            "user/message",
            json!({"id":"summary","role":"user","content":[{"type":"text","text":"needle summary"}],"source":{"kind":"test"}}),
            Some(SurfaceIntent::replace(
                SessionSeq::new(0).unwrap(),
                SessionSeq::new(0).unwrap(),
                vec![SessionSeq::new(0).unwrap()],
            )),
        )
        .unwrap();
    session
        .append(
            "turn/end",
            json!({"turn":1,"reason":{"kind":"error","error":{"message":"needle failure","code":"UNKNOWN"}}}),
            None,
        )
        .unwrap();
    let query = root.get::<SessionQuery>().unwrap();
    let all = query
        .search_events(
            SessionEventSearchRequest {
                session_id: session.id().clone(),
                query: "needle".into(),
                filters: vec![],
                limit: None,
                cursor: None,
            },
            SessionSearchExecContext::default(),
        )
        .await
        .unwrap();
    let surfaces = all
        .items
        .iter()
        .map(|item| item.record.surface)
        .collect::<Vec<_>>();
    assert!(surfaces.contains(&SessionEventSurface::Current));
    assert!(surfaces.contains(&SessionEventSurface::Shadowed));
    assert!(surfaces.contains(&SessionEventSurface::LogOnly));
    let filtered = query
        .search_events(
            SessionEventSearchRequest {
                session_id: session.id().clone(),
                query: "needle".into(),
                filters: vec![
                    SessionEventMetadataFilter::Seq(SessionResultRange {
                        from: Some(2.0),
                        to: Some(2.0),
                    }),
                    SessionEventMetadataFilter::Surface(vec![SessionEventSurface::Current]),
                ],
                limit: None,
                cursor: None,
            },
            SessionSearchExecContext::default(),
        )
        .await
        .unwrap();
    assert_eq!(filtered.items.len(), 1);
    assert_eq!(filtered.items[0].record.seq.get(), 2);
    kernel.shutdown().await;
}

#[tokio::test]
async fn cursors_bind_requests_and_track_only_the_relevant_corpus() {
    let (kernel, root) = context(json!({"path":":memory:","defaultLimit":1,"maxLimit":5})).await;
    let sessions = root.get::<Sessions>().unwrap();
    let target = sessions
        .create(
            &root,
            Some(SessionId::from("target")),
            CreateSessionOptions::default(),
        )
        .unwrap();
    append_user(&target, "needle one");
    append_user(&target, "needle two");
    let other = sessions
        .create(
            &root,
            Some(SessionId::from("other")),
            CreateSessionOptions::default(),
        )
        .unwrap();
    append_user(&other, "needle other");
    let query = root.get::<SessionQuery>().unwrap();
    let event_page = query
        .search_events(
            SessionEventSearchRequest {
                session_id: target.id().clone(),
                query: "needle".into(),
                filters: vec![],
                limit: Some(1),
                cursor: None,
            },
            SessionSearchExecContext::default(),
        )
        .await
        .unwrap();
    let event_cursor = event_page.next_cursor.unwrap();
    let session_page = query
        .search_sessions(
            SessionSearchRequest {
                limit: Some(1),
                ..request("needle")
            },
            SessionSearchExecContext::default(),
        )
        .await
        .unwrap();
    let session_cursor = session_page.next_cursor.unwrap();

    let unrelated = sessions
        .create(
            &root,
            Some(SessionId::from("unrelated")),
            CreateSessionOptions::default(),
        )
        .unwrap();
    append_user(&unrelated, "needle unrelated");
    assert!(
        query
            .search_events(
                SessionEventSearchRequest {
                    session_id: target.id().clone(),
                    query: "needle".into(),
                    filters: vec![],
                    limit: Some(1),
                    cursor: Some(event_cursor.clone()),
                },
                SessionSearchExecContext::default(),
            )
            .await
            .is_ok()
    );
    let error = query
        .search_sessions(
            SessionSearchRequest {
                limit: Some(1),
                cursor: Some(session_cursor),
                ..request("needle")
            },
            SessionSearchExecContext::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(error_code(&error), "SESSION_QUERY_STALE_CURSOR");
    let error = query
        .search_events(
            SessionEventSearchRequest {
                session_id: target.id().clone(),
                query: "different".into(),
                filters: vec![],
                limit: Some(1),
                cursor: Some(event_cursor.clone()),
            },
            SessionSearchExecContext::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(error_code(&error), "SESSION_QUERY_INVALID_CURSOR");
    append_user(&target, "needle three");
    let error = query
        .search_events(
            SessionEventSearchRequest {
                session_id: target.id().clone(),
                query: "needle".into(),
                filters: vec![],
                limit: Some(1),
                cursor: Some(event_cursor),
            },
            SessionSearchExecContext::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(error_code(&error), "SESSION_QUERY_STALE_CURSOR");
    kernel.shutdown().await;
}

#[tokio::test]
async fn replacement_persistence_reloads_equal_opaque_revisions() {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let first: Arc<dyn SessionPersistence> = Arc::new(FakePersistence::new("first text"));
    let first_binding = root
        .provide::<SessionPersistenceService>(first)
        .await
        .unwrap();
    root.plugin::<SessionQuerySqlite>(json!({"path":":memory:"}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let query = root.get::<SessionQuery>().unwrap();
    assert_eq!(
        query
            .search_sessions(request("first"), SessionSearchExecContext::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );

    first_binding.dispose().await;
    let replacement: Arc<dyn SessionPersistence> = Arc::new(FakePersistence::new("second text"));
    root.provide::<SessionPersistenceService>(replacement)
        .await
        .unwrap();
    assert_eq!(
        query
            .search_sessions(request("second"), SessionSearchExecContext::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn discards_a_stale_list_rejection_when_persistence_unmounts() {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let fake = Arc::new(FakePersistence::new("persisted needle"));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    *fake.list_started.lock().unwrap() = Some(started_tx);
    let gate = Arc::new(tokio::sync::Notify::new());
    *fake.list_gate.lock().unwrap() = Some(gate.clone());
    let binding = root
        .provide::<SessionPersistenceService>(fake.clone())
        .await
        .unwrap();
    root.plugin::<SessionQuerySqlite>(json!({"path":":memory:"}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let query = root.get::<SessionQuery>().unwrap();
    let pending_query = query.clone();
    let pending = tokio::spawn(async move {
        pending_query
            .search_sessions(request("needle"), SessionSearchExecContext::default())
            .await
    });
    started_rx.await.unwrap();
    binding.dispose().await;
    fake.fail_after_gate.store(true, Ordering::SeqCst);
    gate.notify_one();
    assert!(pending.await.unwrap().unwrap().items.is_empty());
    kernel.shutdown().await;
}

#[tokio::test]
async fn retries_against_a_replacement_after_the_prior_binding_rejects() {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let prior = Arc::new(FakePersistence::new("prior needle"));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    *prior.list_started.lock().unwrap() = Some(started_tx);
    let gate = Arc::new(tokio::sync::Notify::new());
    *prior.list_gate.lock().unwrap() = Some(gate.clone());
    let binding = root
        .provide::<SessionPersistenceService>(prior.clone())
        .await
        .unwrap();
    root.plugin::<SessionQuerySqlite>(json!({"path":":memory:"}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let query = root.get::<SessionQuery>().unwrap();
    let pending_query = query.clone();
    let pending = tokio::spawn(async move {
        pending_query
            .search_sessions(request("replacement"), SessionSearchExecContext::default())
            .await
    });
    started_rx.await.unwrap();
    binding.dispose().await;
    root.provide::<SessionPersistenceService>(Arc::new(FakePersistence::new("replacement needle")))
        .await
        .unwrap();
    prior.fail_after_gate.store(true, Ordering::SeqCst);
    gate.notify_one();
    assert_eq!(pending.await.unwrap().unwrap().items.len(), 1);
    kernel.shutdown().await;
}

#[tokio::test]
async fn persistence_failure_after_signal_abort_has_the_aborted_code() {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let controller = Arc::new(AbortController::new());
    let persistence: Arc<dyn SessionPersistence> = Arc::new(FakePersistence::aborting(
        "cancelled text",
        controller.clone(),
    ));
    root.provide::<SessionPersistenceService>(persistence)
        .await
        .unwrap();
    root.plugin::<SessionQuerySqlite>(json!({"path":":memory:"}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let error = root
        .get::<SessionQuery>()
        .unwrap()
        .search_sessions(
            request("cancelled"),
            SessionSearchExecContext {
                signal: Some(controller.signal()),
            },
        )
        .await
        .unwrap_err();
    let error = hoh_error::HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(error.code(), "SESSION_QUERY_ABORTED");
    assert_eq!(error.message(), "session-search aborted");
    assert!(error.cause().is_some());
    kernel.shutdown().await;
}

async fn cold_stage_failure(
    stage: FailureStage,
    controller: Option<Arc<AbortController>>,
) -> BoxError {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    root.provide::<SessionPersistenceService>(Arc::new(FakePersistence::failing_cold_read(
        stage,
        controller.clone(),
    )))
    .await
    .unwrap();
    root.plugin::<SessionQuerySqlite>(json!({"path":":memory:"}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let error = root
        .get::<SessionQuery>()
        .unwrap()
        .search_sessions(
            request("needle"),
            SessionSearchExecContext {
                signal: controller.map(|value| value.signal()),
            },
        )
        .await
        .unwrap_err();
    kernel.shutdown().await;
    error
}

#[tokio::test]
async fn wraps_open_read_and_close_failures_from_cold_inspection_exactly() {
    for stage in [FailureStage::Open, FailureStage::Read, FailureStage::Close] {
        let error = cold_stage_failure(stage, None).await;
        let error = hoh_error::HarnessError::of(error.as_ref()).unwrap();
        assert_eq!(error.code(), "SESSION_QUERY_PERSISTENCE_FAILED");
        assert_eq!(
            error.message(),
            "session-search persistence observation failed: offline"
        );
        assert_eq!(error.cause().unwrap().to_string(), "offline");
    }
}

#[tokio::test]
async fn maps_open_read_and_close_abort_failures_to_the_query_abort_contract() {
    for stage in [FailureStage::Open, FailureStage::Read, FailureStage::Close] {
        let controller = Arc::new(AbortController::new());
        let error = cold_stage_failure(stage, Some(controller)).await;
        let error = hoh_error::HarnessError::of(error.as_ref()).unwrap();
        assert_eq!(error.code(), "SESSION_QUERY_ABORTED");
        assert_eq!(error.message(), "session-search aborted");
        assert_eq!(error.cause().unwrap().to_string(), "offline");
    }
}

#[tokio::test]
async fn abort_after_the_second_snapshot_list_does_not_commit_observation() {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let controller = Arc::new(AbortController::new());
    let fake = Arc::new(FakePersistence::aborting_on_second_list(
        "persisted needle",
        controller.clone(),
    ));
    root.provide::<SessionPersistenceService>(fake.clone())
        .await
        .unwrap();
    root.plugin::<SessionQuerySqlite>(json!({"path":":memory:"}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let query = root.get::<SessionQuery>().unwrap();
    let error = query
        .search_sessions(
            request("needle"),
            SessionSearchExecContext {
                signal: Some(controller.signal()),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error_code(&error), "SESSION_QUERY_ABORTED");
    assert_eq!(fake.reads.load(Ordering::SeqCst), 1);
    assert_eq!(
        query
            .search_sessions(request("needle"), SessionSearchExecContext::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(fake.reads.load(Ordering::SeqCst), 2);
    kernel.shutdown().await;
}

#[tokio::test]
async fn abort_while_an_ignoring_second_list_succeeds_prevents_commit() {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let fake = Arc::new(FakePersistence::new("persisted needle"));
    fake.list_started_call.store(2, Ordering::SeqCst);
    fake.list_gate_call.store(2, Ordering::SeqCst);
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    *fake.list_started.lock().unwrap() = Some(started_tx);
    let gate = Arc::new(tokio::sync::Notify::new());
    *fake.list_gate.lock().unwrap() = Some(gate.clone());
    root.provide::<SessionPersistenceService>(fake.clone())
        .await
        .unwrap();
    root.plugin::<SessionQuerySqlite>(json!({"path":":memory:"}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let query = root.get::<SessionQuery>().unwrap();
    let controller = Arc::new(AbortController::new());
    let pending_query = query.clone();
    let signal = controller.signal();
    let pending = tokio::spawn(async move {
        pending_query
            .search_sessions(
                request("needle"),
                SessionSearchExecContext {
                    signal: Some(signal),
                },
            )
            .await
    });
    started_rx.await.unwrap();
    controller.abort();
    gate.notify_one();
    let error = pending.await.unwrap().unwrap_err();
    assert_eq!(error_code(&error), "SESSION_QUERY_ABORTED");
    assert_eq!(fake.reads.load(Ordering::SeqCst), 1);

    assert_eq!(
        query
            .search_sessions(request("needle"), SessionSearchExecContext::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(fake.reads.load(Ordering::SeqCst), 2);
    kernel.shutdown().await;
}

#[tokio::test]
async fn duplicate_ids_in_the_second_snapshot_list_fail_without_committing() {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let fake = Arc::new(FakePersistence::new("persisted needle"));
    fake.duplicate_on_list_call.store(2, Ordering::SeqCst);
    root.provide::<SessionPersistenceService>(fake.clone())
        .await
        .unwrap();
    root.plugin::<SessionQuerySqlite>(json!({"path":":memory:"}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let query = root.get::<SessionQuery>().unwrap();
    let error = query
        .search_sessions(request("needle"), SessionSearchExecContext::default())
        .await
        .unwrap_err();
    let error = hoh_error::HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(error.code(), "SESSION_QUERY_PERSISTENCE_FAILED");
    assert_eq!(
        error.message(),
        "session-search persistence observation failed: persistence listed duplicate session \"stored\""
    );
    assert_eq!(fake.reads.load(Ordering::SeqCst), 1);

    fake.duplicate_on_list_call.store(0, Ordering::SeqCst);
    assert_eq!(
        query
            .search_sessions(request("needle"), SessionSearchExecContext::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(fake.reads.load(Ordering::SeqCst), 2);
    kernel.shutdown().await;
}

#[tokio::test]
async fn recovers_on_the_next_search_after_a_source_failure() {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let fake = Arc::new(FakePersistence::new("persisted needle"));
    fake.fail_next_list.store(true, Ordering::SeqCst);
    root.provide::<SessionPersistenceService>(fake)
        .await
        .unwrap();
    root.plugin::<SessionQuerySqlite>(json!({"path":":memory:"}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let query = root.get::<SessionQuery>().unwrap();
    let error = query
        .search_sessions(request("needle"), SessionSearchExecContext::default())
        .await
        .unwrap_err();
    assert_eq!(error_code(&error), "SESSION_QUERY_PERSISTENCE_FAILED");
    assert_eq!(
        query
            .search_sessions(request("needle"), SessionSearchExecContext::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn fails_after_one_retry_when_snapshots_keep_changing() {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let fake = Arc::new(FakePersistence::new("persisted needle"));
    fake.revision_per_list.store(true, Ordering::SeqCst);
    root.provide::<SessionPersistenceService>(fake.clone())
        .await
        .unwrap();
    root.plugin::<SessionQuerySqlite>(json!({"path":":memory:"}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let error = root
        .get::<SessionQuery>()
        .unwrap()
        .search_sessions(request("needle"), SessionSearchExecContext::default())
        .await
        .unwrap_err();
    assert_eq!(error_code(&error), "SESSION_QUERY_PERSISTENCE_FAILED");
    assert_eq!(fake.lists.load(Ordering::SeqCst), 4);
    kernel.shutdown().await;
}

#[tokio::test]
async fn dynamically_mounts_shadows_reveals_and_unmounts_persistence() {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    root.plugin::<SessionQuerySqlite>(json!({"path":":memory:"}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let query = root.get::<SessionQuery>().unwrap();
    assert!(
        query
            .search_sessions(request("first"), SessionSearchExecContext::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );

    let fake = Arc::new(FakePersistence::new("first persisted needle"));
    let binding = root
        .provide::<SessionPersistenceService>(fake.clone())
        .await
        .unwrap();
    let persisted = query
        .search_sessions(request("persisted"), SessionSearchExecContext::default())
        .await
        .unwrap();
    assert_eq!(persisted.items.len(), 1);
    assert!(!persisted.items[0].session.live);
    assert!(persisted.items[0].session.persisted);

    let sessions = root.get::<Sessions>().unwrap();
    let live = sessions
        .prepare(
            Some(SessionId::from("stored")),
            PrepareSessionOptions::Create(CreateSessionOptions {
                meta: Some(json!({"createdAt":1})),
                ..CreateSessionOptions::default()
            }),
        )
        .unwrap();
    let detach = sessions.enter(&root, live.clone()).unwrap();
    sessions.announce(&live).unwrap();
    append_user(&live, "live needle");
    assert!(
        query
            .search_sessions(request("persisted"), SessionSearchExecContext::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let live_page = query
        .search_sessions(request("live"), SessionSearchExecContext::default())
        .await
        .unwrap();
    assert!(live_page.items[0].session.live);
    assert!(live_page.items[0].session.persisted);

    detach.detach();
    assert_eq!(
        query
            .search_sessions(request("persisted"), SessionSearchExecContext::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    binding.dispose().await;
    assert!(
        query
            .search_sessions(request("persisted"), SessionSearchExecContext::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn does_not_read_a_persisted_log_while_the_same_session_is_live() {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let sessions = root.get::<Sessions>().unwrap();
    let live = sessions
        .prepare(
            Some(SessionId::from("stored")),
            PrepareSessionOptions::Create(CreateSessionOptions {
                meta: Some(json!({"createdAt":1})),
                ..CreateSessionOptions::default()
            }),
        )
        .unwrap();
    let detach = sessions.enter(&root, live.clone()).unwrap();
    sessions.announce(&live).unwrap();
    append_user(&live, "live needle");
    let fake = Arc::new(FakePersistence::new("persisted needle"));
    root.provide::<SessionPersistenceService>(fake.clone())
        .await
        .unwrap();
    root.plugin::<SessionQuerySqlite>(json!({"path":":memory:"}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let query = root.get::<SessionQuery>().unwrap();
    assert_eq!(
        query
            .search_sessions(request("live"), SessionSearchExecContext::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(fake.reads.load(Ordering::SeqCst), 0);
    detach.detach();
    assert_eq!(
        query
            .search_sessions(request("persisted"), SessionSearchExecContext::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(fake.reads.load(Ordering::SeqCst), 1);
    kernel.shutdown().await;
}

#[tokio::test]
async fn prefers_a_live_owner_that_attaches_during_a_persisted_read() {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let fake = Arc::new(FakePersistence::new("persisted needle"));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    *fake.read_started.lock().unwrap() = Some(started_tx);
    let gate = Arc::new(tokio::sync::Notify::new());
    *fake.read_gate.lock().unwrap() = Some(gate.clone());
    root.provide::<SessionPersistenceService>(fake.clone())
        .await
        .unwrap();
    root.plugin::<SessionQuerySqlite>(json!({"path":":memory:"}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let query = root.get::<SessionQuery>().unwrap();
    let pending_query = query.clone();
    let pending = tokio::spawn(async move {
        pending_query
            .search_sessions(request("live"), SessionSearchExecContext::default())
            .await
    });
    started_rx.await.unwrap();
    let live = root
        .get::<Sessions>()
        .unwrap()
        .create(
            &root,
            Some(SessionId::from("stored")),
            CreateSessionOptions {
                meta: Some(json!({"createdAt":1})),
                ..CreateSessionOptions::default()
            },
        )
        .unwrap();
    append_user(&live, "live needle");
    gate.notify_one();
    let page = pending.await.unwrap().unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(page.items[0].session.live);
    assert!(page.items[0].session.persisted);
    assert_eq!(fake.reads.load(Ordering::SeqCst), 1);
    kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_immutable_header_conflicts_between_live_and_persisted_sources() {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let fake = Arc::new(FakePersistence::new("persisted needle"));
    root.provide::<SessionPersistenceService>(fake)
        .await
        .unwrap();
    let live = root
        .get::<Sessions>()
        .unwrap()
        .create(
            &root,
            Some(SessionId::from("stored")),
            CreateSessionOptions {
                meta: Some(json!({"createdAt":2})),
                ..CreateSessionOptions::default()
            },
        )
        .unwrap();
    append_user(&live, "live needle");
    root.plugin::<SessionQuerySqlite>(json!({"path":":memory:"}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let error = root
        .get::<SessionQuery>()
        .unwrap()
        .search_sessions(request("needle"), SessionSearchExecContext::default())
        .await
        .unwrap_err();
    assert_eq!(error_code(&error), "SESSION_QUERY_SOURCE_CONFLICT");
    kernel.shutdown().await;
}

#[tokio::test]
async fn pre_aborted_searches_start_no_persistence_observation() {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let fake = Arc::new(FakePersistence::new("persisted needle"));
    root.provide::<SessionPersistenceService>(fake.clone())
        .await
        .unwrap();
    root.plugin::<SessionQuerySqlite>(json!({"path":":memory:"}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let controller = AbortController::new();
    controller.abort();
    let error = root
        .get::<SessionQuery>()
        .unwrap()
        .search_sessions(
            request("needle"),
            SessionSearchExecContext {
                signal: Some(controller.signal()),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error_code(&error), "SESSION_QUERY_ABORTED");
    assert_eq!(fake.lists.load(Ordering::SeqCst), 0);
    assert_eq!(fake.reads.load(Ordering::SeqCst), 0);
    kernel.shutdown().await;
}

#[tokio::test]
async fn pre_aborted_event_search_starts_no_persistence_observation() {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let fake = Arc::new(FakePersistence::new("persisted needle"));
    root.provide::<SessionPersistenceService>(fake.clone())
        .await
        .unwrap();
    root.plugin::<SessionQuerySqlite>(json!({"path":":memory:"}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let controller = AbortController::new();
    controller.abort();
    let error = root
        .get::<SessionQuery>()
        .unwrap()
        .search_events(
            SessionEventSearchRequest {
                session_id: SessionId::from("stored"),
                query: "needle".into(),
                filters: vec![],
                limit: None,
                cursor: None,
            },
            SessionSearchExecContext {
                signal: Some(controller.signal()),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error_code(&error), "SESSION_QUERY_ABORTED");
    assert_eq!(fake.lists.load(Ordering::SeqCst), 0);
    assert_eq!(fake.reads.load(Ordering::SeqCst), 0);
    kernel.shutdown().await;
}

#[tokio::test]
async fn event_search_rejects_an_absent_target_with_the_typed_code() {
    let (kernel, root) = context(json!({"path":":memory:"})).await;
    let error = root
        .get::<SessionQuery>()
        .unwrap()
        .search_events(
            SessionEventSearchRequest {
                session_id: SessionId::from("absent"),
                query: "needle".into(),
                filters: vec![],
                limit: None,
                cursor: None,
            },
            SessionSearchExecContext::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(error_code(&error), "SESSION_QUERY_SESSION_NOT_FOUND");
    kernel.shutdown().await;
}

#[tokio::test]
async fn transient_persistence_topology_changes_stale_session_cursors() {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let binding = root
        .provide::<SessionPersistenceService>(Arc::new(FakePersistence::new("persisted needle")))
        .await
        .unwrap();
    let live = root
        .get::<Sessions>()
        .unwrap()
        .create(
            &root,
            Some(SessionId::from("live")),
            CreateSessionOptions::default(),
        )
        .unwrap();
    append_user(&live, "live needle");
    root.plugin::<SessionQuerySqlite>(json!({"path":":memory:","defaultLimit":1}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let query = root.get::<SessionQuery>().unwrap();
    let first = query
        .search_sessions(request("needle"), SessionSearchExecContext::default())
        .await
        .unwrap();
    let cursor = first.next_cursor.unwrap();
    binding.dispose().await;
    let error = query
        .search_sessions(
            SessionSearchRequest {
                cursor: Some(cursor),
                ..request("needle")
            },
            SessionSearchExecContext::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(error_code(&error), "SESSION_QUERY_STALE_CURSOR");
    kernel.shutdown().await;
}

#[tokio::test]
async fn invalid_database_fails_startup_but_is_deferred_in_first_search_mode() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("foreign.db");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE foreign_table (value TEXT);")
        .unwrap();
    drop(db);

    let eager = Kernel::start();
    let eager_root = eager.root();
    eager_root
        .plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let fiber = eager_root
        .plugin::<SessionQuerySqlite>(json!({"path":path}))
        .unwrap();
    let error = fiber.settled().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("not an empty or recognized derived index")
    );
    assert!(eager_root.get::<SessionQuery>().is_none());
    eager.shutdown().await;

    let lazy = Kernel::start();
    let lazy_root = lazy.root();
    lazy_root
        .plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    lazy_root
        .plugin::<SessionQuerySqlite>(json!({"path":path,"openAt":"first-search"}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let error = lazy_root
        .get::<SessionQuery>()
        .unwrap()
        .search_sessions(request("needle"), SessionSearchExecContext::default())
        .await
        .unwrap_err();
    assert_eq!(error_code(&error), "SESSION_QUERY_INDEX_FAILED");
    lazy.shutdown().await;
}

#[tokio::test]
async fn close_waits_for_accepted_work_and_rejects_queued_and_future_searches() {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let fake = Arc::new(FakePersistence::new("persisted needle"));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    *fake.list_started.lock().unwrap() = Some(started_tx);
    let gate = Arc::new(tokio::sync::Notify::new());
    *fake.list_gate.lock().unwrap() = Some(gate.clone());
    root.provide::<SessionPersistenceService>(fake)
        .await
        .unwrap();
    let fiber = root
        .plugin::<SessionQuerySqlite>(json!({"path":":memory:"}))
        .unwrap();
    fiber.settled().await.unwrap();
    let query = root.get::<SessionQuery>().unwrap();
    let accepted_query = query.clone();
    let accepted = tokio::spawn(async move {
        accepted_query
            .search_sessions(request("needle"), SessionSearchExecContext::default())
            .await
    });
    started_rx.await.unwrap();
    let queued_query = query.clone();
    let queued = tokio::spawn(async move {
        queued_query
            .search_sessions(request("needle"), SessionSearchExecContext::default())
            .await
    });
    let disposing = tokio::spawn(async move { fiber.dispose().await });
    tokio::task::yield_now().await;
    gate.notify_one();

    assert!(accepted.await.unwrap().is_ok());
    let error = queued.await.unwrap().unwrap_err();
    assert_eq!(error_code(&error), "SESSION_QUERY_INDEX_FAILED");
    disposing.await.unwrap();
    let error = query
        .search_sessions(request("needle"), SessionSearchExecContext::default())
        .await
        .unwrap_err();
    assert_eq!(error_code(&error), "SESSION_QUERY_INDEX_FAILED");
    kernel.shutdown().await;
}

#[tokio::test]
async fn combines_the_real_jsonl_backend_with_search_keylessly() {
    let persistence_root = tempfile::tempdir().unwrap();
    let derived_root = tempfile::tempdir().unwrap();
    let derived = derived_root.path().join("derived.db");
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    root.plugin::<JsonlSessionPersistencePlugin>(json!({
        "root": persistence_root.path(),
        "compression": "none"
    }))
    .unwrap()
    .settled()
    .await
    .unwrap();
    root.plugin::<SessionQuerySqlite>(json!({"path":derived}))
        .unwrap()
        .settled()
        .await
        .unwrap();

    let persistence = root.get::<SessionPersistenceService>().unwrap();
    let header = SessionHeader::new(SessionId::from("real"), 10);
    let writer = persistence.create(&header, None).await.unwrap();
    writer
        .append(
            &[stored_user_event("real-message", "real JSONL needle")],
            None,
        )
        .await
        .unwrap();
    writer.close().await.unwrap();

    let query = root.get::<SessionQuery>().unwrap();
    let sessions = query
        .search_sessions(request("JSONL needle"), SessionSearchExecContext::default())
        .await
        .unwrap();
    assert_eq!(sessions.items.len(), 1);
    assert_eq!(sessions.items[0].session.header.id, header.id);
    assert_eq!(
        sessions.items[0].session.header.created_at,
        header.created_at
    );
    assert!(sessions.items[0].session.persisted);
    assert!(!sessions.items[0].session.live);
    let events = query
        .search_events(
            SessionEventSearchRequest {
                session_id: header.id.clone(),
                query: "JSONL needle".into(),
                filters: vec![],
                limit: None,
                cursor: None,
            },
            SessionSearchExecContext::default(),
        )
        .await
        .unwrap();
    assert_eq!(events.session.id, header.id);
    assert_eq!(events.session.created_at, header.created_at);
    assert_eq!(events.items.len(), 1);
    assert_eq!(events.items[0].record.seq.get(), 0);
    kernel.shutdown().await;
}

async fn populate_and_search_jsonl(
    persistence_root: &std::path::Path,
    derived: &std::path::Path,
    text: &str,
) -> usize {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    root.plugin::<JsonlSessionPersistencePlugin>(json!({
        "root": persistence_root,
        "compression": "none"
    }))
    .unwrap()
    .settled()
    .await
    .unwrap();
    let persistence = root.get::<SessionPersistenceService>().unwrap();
    if persistence.list(None).await.unwrap().is_empty() {
        let header = SessionHeader::new(SessionId::from("same-id"), 10);
        let writer = persistence.create(&header, None).await.unwrap();
        writer
            .append(&[stored_user_event("same-message", text)], None)
            .await
            .unwrap();
        writer.close().await.unwrap();
    }
    root.plugin::<SessionQuerySqlite>(json!({"path":derived}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let count = root
        .get::<SessionQuery>()
        .unwrap()
        .search_sessions(request(text), SessionSearchExecContext::default())
        .await
        .unwrap()
        .items
        .len();
    kernel.shutdown().await;
    count
}

#[tokio::test]
async fn reconciles_a_reopened_index_and_reloads_another_jsonl_store() {
    let persistence_a = tempfile::tempdir().unwrap();
    let persistence_b = tempfile::tempdir().unwrap();
    let derived_root = tempfile::tempdir().unwrap();
    let derived = derived_root.path().join("reopened.db");
    assert_eq!(
        populate_and_search_jsonl(persistence_a.path(), &derived, "alpha source").await,
        1
    );
    assert_eq!(
        populate_and_search_jsonl(persistence_a.path(), &derived, "alpha source").await,
        1
    );
    assert_eq!(
        populate_and_search_jsonl(persistence_b.path(), &derived, "bravo source").await,
        1
    );

    let kernel = Kernel::start();
    let root = kernel.root();
    root.plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    root.plugin::<JsonlSessionPersistencePlugin>(json!({
        "root": persistence_b.path(),
        "compression":"none"
    }))
    .unwrap()
    .settled()
    .await
    .unwrap();
    root.plugin::<SessionQuerySqlite>(json!({"path":derived}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    assert!(
        root.get::<SessionQuery>()
            .unwrap()
            .search_sessions(request("alpha"), SessionSearchExecContext::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn drops_connection_local_live_overlays_when_the_index_reopens() {
    let derived_root = tempfile::tempdir().unwrap();
    let derived = derived_root.path().join("live-overlay.db");
    let first = Kernel::start();
    let first_root = first.root();
    first_root
        .plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    first_root
        .plugin::<SessionQuerySqlite>(json!({"path":derived}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let live = first_root
        .get::<Sessions>()
        .unwrap()
        .create(
            &first_root,
            Some(SessionId::from("transient-live")),
            CreateSessionOptions::default(),
        )
        .unwrap();
    append_user(&live, "transient needle");
    assert_eq!(
        first_root
            .get::<SessionQuery>()
            .unwrap()
            .search_sessions(request("transient"), SessionSearchExecContext::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    first.shutdown().await;

    let reopened = Kernel::start();
    let reopened_root = reopened.root();
    reopened_root
        .plugin::<SessionPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    reopened_root
        .plugin::<SessionQuerySqlite>(json!({"path":derived}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    assert!(
        reopened_root
            .get::<SessionQuery>()
            .unwrap()
            .search_sessions(request("transient"), SessionSearchExecContext::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    reopened.shutdown().await;
}
