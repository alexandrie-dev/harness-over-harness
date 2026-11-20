use futures_util::future::BoxFuture;
use hoh_kernel::Kernel;
use hoh_session::{CreateSessionOptions, SessionId, SessionStore, Sessions, SurfaceIntent};
use hoh_session_projection::{
    ProjectionDefinition, ProjectionWire, SessionProjectionRegistry, SessionProjections,
};
use hoh_session_query::*;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn rejects_concurrency_above_javascript_safe_integer_without_spinning() {
    let kernel = Kernel::start();
    let config = Config {
        persisted_read_concurrency: hoh_session::MAX_SAFE_INTEGER as usize + 1,
        ..Default::default()
    };
    let error = match SessionQueryEngine::new(
        kernel.root(),
        SessionStore::new(),
        Arc::new(ScanProvider),
        config,
    ) {
        Ok(_) => panic!("unsafe concurrency unexpectedly accepted"),
        Err(error) => error,
    };
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        SessionQueryErrorCode::InvalidConfig.as_str()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_cache_capacity_above_javascript_safe_integer() {
    let kernel = Kernel::start();
    let config = Config {
        prepared_session_cache_size: hoh_session::MAX_SAFE_INTEGER as usize + 1,
        ..Default::default()
    };
    let error = match SessionQueryEngine::new(
        kernel.root(),
        SessionStore::new(),
        Arc::new(ScanProvider),
        config,
    ) {
        Ok(_) => panic!("unsafe cache capacity unexpectedly accepted"),
        Err(error) => error,
    };
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        SessionQueryErrorCode::InvalidConfig.as_str()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_zero_concurrency_and_cache_capacity() {
    for config in [
        Config {
            persisted_read_concurrency: 0,
            ..Default::default()
        },
        Config {
            prepared_session_cache_size: 0,
            ..Default::default()
        },
    ] {
        let kernel = Kernel::start();
        let error = match SessionQueryEngine::new(
            kernel.root(),
            SessionStore::new(),
            Arc::new(ScanProvider),
            config,
        ) {
            Ok(_) => panic!("zero-valued config unexpectedly accepted"),
            Err(error) => error,
        };
        assert_eq!(
            hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
            SessionQueryErrorCode::InvalidConfig.as_str()
        );
        kernel.shutdown().await;
    }
}

struct ScanProvider;

impl SessionSearchProvider for ScanProvider {
    fn search_sessions<'a>(
        &'a self,
        _request: SessionSearchRequest,
        _exec: SessionSearchExecContext,
    ) -> BoxFuture<'a, Result<SessionSearchPage<SessionSearchHit>, hoh_error::BoxError>> {
        Box::pin(async {
            Ok(SessionSearchPage {
                items: Vec::new(),
                next_cursor: None,
            })
        })
    }

    fn search_events<'a>(
        &'a self,
        request: SessionEventSearchRequest,
        _exec: SessionSearchExecContext,
    ) -> BoxFuture<'a, Result<SessionEventSearchPage, hoh_error::BoxError>> {
        Box::pin(async move {
            let mut header = hoh_session::SessionHeader::new(request.session_id, 0);
            header.is_seeded = false;
            Ok(SessionEventSearchPage {
                session: header,
                items: Vec::new(),
                next_cursor: None,
            })
        })
    }
}

struct Fixture {
    kernel: Kernel,
    store: SessionStore,
}

impl Fixture {
    async fn new() -> Self {
        let kernel = Kernel::start();
        let store = SessionStore::new();
        kernel
            .root()
            .provide::<Sessions>(Arc::new(store.clone()))
            .await
            .unwrap();
        let sessions = kernel.root().get::<Sessions>().unwrap();
        mount_session_query(
            &kernel.root(),
            sessions,
            Arc::new(ScanProvider),
            Config::default(),
        )
        .await
        .unwrap();
        Self { kernel, store }
    }
}

fn user_data(text: &str) -> serde_json::Value {
    json!({"id":format!("user-{text}"),"role":"user","content":[{"type":"text","text":text}],"source":{"kind":"user"}})
}

fn counting_projection(fail: bool) -> ProjectionDefinition<u64> {
    ProjectionDefinition {
        key: if fail { "query/failing" } else { "query/count" }.into(),
        state_version: 1,
        state_schema: Arc::new(|value| {
            value
                .and_then(serde_json::Value::as_u64)
                .map(Arc::new)
                .ok_or_else(|| {
                    Box::new(std::io::Error::other("invalid count")) as hoh_error::BoxError
                })
        }),
        init: Arc::new(|_, _| Ok(Arc::new(0))),
        apply: Arc::new(move |state, _| {
            if fail {
                Err(Box::new(std::io::Error::other("projection failed")) as hoh_error::BoxError)
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

#[tokio::test]
async fn registers_exact_and_search_behavior_under_one_key() {
    let fixture = Fixture::new().await;
    let query = fixture.kernel.root().get::<SessionQuery>().unwrap();
    assert!(query.list_sessions(None).await.unwrap().is_empty());
    let page = query
        .search_sessions(
            SessionSearchRequest {
                query: "x".into(),
                session_filters: Vec::new(),
                event_filters: Vec::new(),
                limit: None,
                cursor: None,
            },
            SessionSearchExecContext::default(),
        )
        .await
        .unwrap();
    assert!(page.items.is_empty());
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn duplicate_service_registration_fails_loudly() {
    let fixture = Fixture::new().await;
    let sessions = fixture.kernel.root().get::<Sessions>().unwrap();
    let error = match mount_session_query(
        &fixture.kernel.root(),
        sessions,
        Arc::new(ScanProvider),
        Config::default(),
    )
    .await
    {
        Ok(_) => panic!("duplicate registration unexpectedly succeeded"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("sessionQuery"));
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn session_query_keeps_persistence_optional_for_live_reads() {
    let kernel = Kernel::start();
    let store = SessionStore::new();
    kernel
        .root()
        .provide::<Sessions>(Arc::new(store.clone()))
        .await
        .unwrap();
    let sessions = kernel.root().get::<Sessions>().unwrap();
    let mounted = mount_session_query(
        &kernel.root(),
        sessions,
        Arc::new(ScanProvider),
        Config::default(),
    )
    .await
    .unwrap();
    store
        .create(
            &kernel.root(),
            Some(SessionId::from("live-only")),
            Default::default(),
        )
        .unwrap();
    let query = kernel.root().get::<SessionQuery>().unwrap();
    assert_eq!(query.list_sessions(None).await.unwrap().len(), 1);
    mounted.dispose().await;
    assert!(kernel.root().get::<SessionQuery>().is_none());
    kernel.shutdown().await;
}

#[tokio::test]
async fn mount_disposal_quiesces_the_optional_persistence_child() {
    struct EmptyPersistence(hoh_session_persistence::SessionPersistenceIdentity);
    impl hoh_session_persistence::SessionPersistence for EmptyPersistence {
        fn identity(&self) -> hoh_session_persistence::SessionPersistenceIdentity {
            self.0.clone()
        }
        fn create<'a>(
            &'a self,
            _header: &'a hoh_session::SessionHeader,
            _options: Option<&'a hoh_session_persistence::SessionPersistenceCreateOptions>,
        ) -> BoxFuture<
            'a,
            Result<Arc<dyn hoh_session_persistence::SessionHandle>, hoh_error::BoxError>,
        > {
            Box::pin(async {
                Err(Box::new(std::io::Error::other("unused")) as hoh_error::BoxError)
            })
        }
        fn open<'a>(
            &'a self,
            _id: &'a SessionId,
            _access: hoh_session_persistence::SessionAccess,
            _options: Option<&'a hoh_session_persistence::SessionPersistenceOpenOptions>,
        ) -> BoxFuture<
            'a,
            Result<Arc<dyn hoh_session_persistence::SessionHandle>, hoh_error::BoxError>,
        > {
            Box::pin(async {
                Err(Box::new(std::io::Error::other("unused")) as hoh_error::BoxError)
            })
        }
        fn flush(&self) -> BoxFuture<'_, Result<(), hoh_error::BoxError>> {
            Box::pin(async { Ok(()) })
        }
        fn stat<'a>(
            &'a self,
            _id: &'a SessionId,
            _options: Option<&'a hoh_session_persistence::SessionPersistenceStatOptions>,
        ) -> BoxFuture<
            'a,
            Result<
                Option<hoh_session_persistence::SessionPersistenceSnapshot>,
                hoh_error::BoxError,
            >,
        > {
            Box::pin(async { Ok(None) })
        }
        fn list<'a>(
            &'a self,
            _options: Option<&'a hoh_session_persistence::SessionPersistenceListOptions>,
        ) -> BoxFuture<
            'a,
            Result<Vec<hoh_session_persistence::SessionPersistenceSnapshot>, hoh_error::BoxError>,
        > {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    let kernel = Kernel::start();
    let store = SessionStore::new();
    kernel
        .root()
        .provide::<Sessions>(Arc::new(store))
        .await
        .unwrap();
    let persistence: Arc<dyn hoh_session_persistence::SessionPersistence> = Arc::new(
        EmptyPersistence(hoh_session_persistence::SessionPersistenceIdentity::new()),
    );
    let persistence_binding = kernel
        .root()
        .provide::<hoh_session_persistence::SessionPersistenceService>(persistence)
        .await
        .unwrap();
    let mounted = mount_session_query(
        &kernel.root(),
        kernel.root().get::<Sessions>().unwrap(),
        Arc::new(ScanProvider),
        Config::default(),
    )
    .await
    .unwrap();
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    mounted.dispose().await;
    assert!(kernel.root().get::<SessionQuery>().is_none());
    persistence_binding.dispose().await;
    assert!(
        kernel
            .snapshot()
            .await
            .provisions
            .iter()
            .all(|provision| provision.name.as_ref() != "sessionPersistence")
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn concurrent_duplicate_mounts_leave_exactly_one_owned_service() {
    let kernel = Kernel::start();
    let store = SessionStore::new();
    kernel
        .root()
        .provide::<Sessions>(Arc::new(store))
        .await
        .unwrap();
    let root = kernel.root();
    let sessions = root.get::<Sessions>().unwrap();
    let first = mount_session_query(
        &root,
        sessions.clone(),
        Arc::new(ScanProvider),
        Config::default(),
    );
    let second = mount_session_query(&root, sessions, Arc::new(ScanProvider), Config::default());
    let (first, second) = tokio::join!(first, second);
    assert_ne!(first.is_ok(), second.is_ok());
    assert!(kernel.root().get::<SessionQuery>().is_some());
    let owner = first.ok().or_else(|| second.ok()).unwrap();
    owner.dispose().await;
    assert!(kernel.root().get::<SessionQuery>().is_none());
    kernel.shutdown().await;
}

#[tokio::test]
async fn lists_live_sessions_deterministically_and_filters_them() {
    let fixture = Fixture::new().await;
    fixture
        .store
        .create(
            &fixture.kernel.root(),
            Some(SessionId::from("older")),
            CreateSessionOptions {
                meta: Some(json!({"createdAt":1})),
                ..Default::default()
            },
        )
        .unwrap();
    fixture
        .store
        .create(
            &fixture.kernel.root(),
            Some(SessionId::from("newer")),
            CreateSessionOptions {
                meta: Some(json!({"createdAt":2,"cwd":"/work"})),
                ..Default::default()
            },
        )
        .unwrap();
    let query = fixture.kernel.root().get::<SessionQuery>().unwrap();
    let listed = query.list_sessions(None).await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|r| r.header.id.as_ref())
            .collect::<Vec<_>>(),
        vec!["newer", "older"]
    );
    let filtered = query
        .filter_sessions(
            &[SessionResultFilter::Cwd(vec![Some("/work".into())])],
            None,
        )
        .await
        .unwrap();
    assert_eq!(filtered[0].header.id, SessionId::from("newer"));
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn reads_replay_valid_full_live_log_and_title() {
    let fixture = Fixture::new().await;
    let session = fixture
        .store
        .create(
            &fixture.kernel.root(),
            Some(SessionId::from("s")),
            Default::default(),
        )
        .unwrap();
    session
        .append(
            "user/message",
            user_data("hello"),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    session
        .append(
            "session/title",
            json!({"title":"Greeting","messageSeqs":[0],"source":{"kind":"user"}}),
            None,
        )
        .unwrap();
    let query = fixture.kernel.root().get::<SessionQuery>().unwrap();
    let log = query.read_session(session.id()).await.unwrap();
    assert_eq!(log.events.len(), 2);
    assert_eq!(
        query
            .read_title(session.id(), None)
            .await
            .unwrap()
            .unwrap()
            .title,
        "Greeting"
    );
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn lists_filters_and_reads_event_windows() {
    let fixture = Fixture::new().await;
    let session = fixture
        .store
        .create(
            &fixture.kernel.root(),
            Some(SessionId::from("events")),
            Default::default(),
        )
        .unwrap();
    for text in ["zero", "one", "two"] {
        session
            .append(
                "user/message",
                user_data(text),
                Some(SurfaceIntent::append()),
            )
            .unwrap();
    }
    let query = fixture.kernel.root().get::<SessionQuery>().unwrap();
    assert_eq!(query.list_events(session.id()).await.unwrap().len(), 3);
    let filtered = query
        .filter_events(
            session.id(),
            &[SessionEventResultFilter::Text("ONE".into())],
        )
        .await
        .unwrap();
    assert_eq!(filtered[0].record.seq.get(), 1);
    let window = query
        .read_event(
            &SessionEventReadRequest {
                session_id: session.id().clone(),
                seq: hoh_session::SessionSeq::new(1).unwrap(),
                before: Some(1),
                after: Some(1),
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(window.events.len(), 3);
    assert!(Arc::ptr_eq(&window.target, &window.events[1]));
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn empty_text_filter_checks_missing_session_before_filter_text() {
    let fixture = Fixture::new().await;
    let query = fixture.kernel.root().get::<SessionQuery>().unwrap();
    let error = query
        .filter_events(
            &SessionId::from("missing"),
            &[SessionEventResultFilter::Text("  ".into())],
        )
        .await
        .unwrap_err();
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        SessionQueryErrorCode::SessionNotFound.as_str()
    );
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_event_windows_beyond_configured_limit() {
    let fixture = Fixture::new().await;
    let query = fixture.kernel.root().get::<SessionQuery>().unwrap();
    let error = query
        .read_event(
            &SessionEventReadRequest {
                session_id: SessionId::from("missing"),
                seq: hoh_session::SessionSeq::new(0).unwrap(),
                before: Some(51),
                after: None,
            },
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        SessionQueryErrorCode::InvalidWindow.as_str()
    );
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn reads_current_surface_with_capture_boundary() {
    let fixture = Fixture::new().await;
    let session = fixture
        .store
        .create(
            &fixture.kernel.root(),
            Some(SessionId::from("surface")),
            Default::default(),
        )
        .unwrap();
    session
        .append(
            "user/message",
            user_data("original"),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    session
        .append(
            "user/message",
            user_data("replacement"),
            Some(SurfaceIntent::replace(
                hoh_session::SessionSeq::new(0).unwrap(),
                hoh_session::SessionSeq::new(0).unwrap(),
                vec![hoh_session::SessionSeq::new(0).unwrap()],
            )),
        )
        .unwrap();
    let query = fixture.kernel.root().get::<SessionQuery>().unwrap();
    let surface = query.read_surface(session.id()).await.unwrap();
    assert_eq!(surface.captured_through_seq.unwrap().get(), 1);
    assert_eq!(surface.events.len(), 1);
    assert_eq!(
        extract_session_event_text(&surface.events[0]),
        "replacement"
    );
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn live_observations_capture_a_fixed_lazy_cut_and_independent_leases() {
    let fixture = Fixture::new().await;
    let session = fixture
        .store
        .create(
            &fixture.kernel.root(),
            Some(SessionId::from("observe")),
            Default::default(),
        )
        .unwrap();
    session
        .append(
            "user/message",
            user_data("first"),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    let query = fixture.kernel.root().get::<SessionQuery>().unwrap();
    let mut observation = query
        .observe_session(
            session.id(),
            SessionObservationOptions {
                projection_mode: ProjectionMode::None,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mut retained = observation.retain().unwrap();
    session
        .append(
            "user/message",
            user_data("second"),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    assert_eq!(observation.events().len(), 1);
    assert!(std::ptr::eq(observation.events(), retained.events()));
    observation.dispose();
    assert!(observation.retain().is_err());
    retained.dispose();
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn missing_live_session_without_persistence_is_not_found() {
    let fixture = Fixture::new().await;
    let query = fixture.kernel.root().get::<SessionQuery>().unwrap();
    let error = match query
        .observe_session(&SessionId::from("missing"), Default::default())
        .await
    {
        Ok(_) => panic!("missing observation unexpectedly succeeded"),
        Err(error) => error,
    };
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        SessionQueryErrorCode::SessionNotFound.as_str()
    );
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn live_observation_computes_mounted_projection_snapshot() {
    let fixture = Fixture::new().await;
    let registry = SessionProjectionRegistry::new(&fixture.kernel.root()).unwrap();
    fixture
        .kernel
        .root()
        .provide::<SessionProjections>(Arc::new(registry))
        .await
        .unwrap();
    let session = fixture
        .store
        .create(
            &fixture.kernel.root(),
            Some(SessionId::from("projected")),
            Default::default(),
        )
        .unwrap();
    let query = fixture.kernel.root().get::<SessionQuery>().unwrap();
    let observation = query
        .observe_session(session.id(), SessionObservationOptions::default())
        .await
        .unwrap();
    assert!(observation.projections().is_some());
    fixture.kernel.shutdown().await;
}

#[tokio::test]
async fn live_observation_wraps_projection_failure_as_corrupt_session() {
    let fixture = Fixture::new().await;
    let registry = SessionProjectionRegistry::new(&fixture.kernel.root()).unwrap();
    registry
        .register(&fixture.kernel.root(), counting_projection(true))
        .await
        .unwrap();
    fixture
        .kernel
        .root()
        .provide::<SessionProjections>(Arc::new(registry))
        .await
        .unwrap();
    let session = fixture
        .store
        .create(
            &fixture.kernel.root(),
            Some(SessionId::from("projection-failure")),
            Default::default(),
        )
        .unwrap();
    session
        .append(
            "user/message",
            user_data("trigger"),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    let query = fixture.kernel.root().get::<SessionQuery>().unwrap();
    let error = match query
        .observe_session(session.id(), SessionObservationOptions::default())
        .await
    {
        Ok(_) => panic!("failing projection unexpectedly succeeded"),
        Err(error) => error,
    };
    let harness = hoh_error::HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(
        harness.code(),
        SessionQueryErrorCode::CorruptSession.as_str()
    );
    assert!(harness.to_string().contains("failed to project session"));
    fixture.kernel.shutdown().await;
}
