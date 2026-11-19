use hoh_error::BoxError;
use hoh_kernel::{Context, Kernel};
use hoh_session::{
    CreateSessionOptions, SESSION_FORMAT_VERSION, Session, SessionEvent, SessionHeader,
    SessionLogOffset, SessionPlugin, SessionSeq, SessionSeqCursor, SessionStore, Sessions,
};
use hoh_session_projection::{
    ProjectionDefinition, ProjectionKeySelection, ProjectionWire, SessionProjectionRegistry,
    SessionProjections,
};
use hoh_session_projection_cache::{
    CheckpointIdentity, CheckpointRecord, SessionProjectionCache, SessionProjectionCachePlugin,
    checkpoint_record, checkpoint_row, projection_cache_domain_spec,
};
use hoh_storage::StoragePlugin;
use hoh_storage_domain::{DomainChanged, StorageDomain, StorageDomainPlugin};
use hoh_storage_json::StorageJsonPlugin;
use indexmap::{IndexMap, IndexSet};
use serde::{Serialize, Serializer, ser::Error as _};
use serde_json::{Value, json};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::task::{Context as TaskContext, Poll, Waker};
use std::time::Duration;
use tempfile::TempDir;

fn failure(message: impl Into<String>) -> BoxError {
    Box::new(std::io::Error::other(message.into()))
}

fn cursor(value: i64) -> SessionSeqCursor {
    if value < 0 {
        SessionSeqCursor::BeforeFirst
    } else {
        SessionSeqCursor::Seq(SessionSeq::new(value as u64).unwrap())
    }
}

fn row(ver: u64, seq: i64, val: Value) -> hoh_session_projection::ProjectionCheckpointRow {
    hoh_session_projection::ProjectionCheckpointRow {
        ver,
        seq: cursor(seq),
        val: Some(val),
    }
}

fn identity(created_at: u64) -> CheckpointIdentity {
    CheckpointIdentity {
        format_version: Some(SESSION_FORMAT_VERSION),
        created_at,
        cwd: None,
        is_seeded: Some(false),
        inherited_event_count: Some(SessionLogOffset::new(0).unwrap()),
    }
}

fn header(id: &str, created_at: i64) -> SessionHeader {
    SessionHeader {
        version: SESSION_FORMAT_VERSION,
        id: id.into(),
        created_at,
        cwd: None,
        parent_session: None,
        is_seeded: false,
        origin: None,
        delegation_depth: None,
        agent_preset: None,
        extra: Default::default(),
    }
}

fn marks_schema(value: Option<&Value>) -> Result<Arc<Value>, BoxError> {
    let value = value.ok_or_else(|| failure("missing marks state"))?;
    if value.is_null()
        || value
            .get("marks")
            .and_then(Value::as_array)
            .is_some_and(|marks| marks.iter().all(Value::is_string))
    {
        Ok(Arc::new(value.clone()))
    } else {
        Err(failure("invalid marks state"))
    }
}

fn marks_view_schema(value: Option<&Value>) -> Result<Arc<Value>, BoxError> {
    let value = value.ok_or_else(|| failure("missing marks view"))?;
    if value
        .get("marks")
        .and_then(Value::as_array)
        .is_some_and(|marks| marks.iter().all(Value::is_string))
    {
        Ok(Arc::new(value.clone()))
    } else {
        Err(failure("invalid marks view"))
    }
}

fn marks_definition(key: &str, version: u64, summary: bool) -> ProjectionDefinition<Value> {
    ProjectionDefinition {
        key: key.into(),
        state_schema: Arc::new(marks_schema),
        init: Arc::new(|_, _| Ok(Arc::new(Value::Null))),
        apply: Arc::new(|state, event| {
            if event.event_type() == "cache-test/mark" {
                Ok(Arc::new(event.data().clone()))
            } else {
                Ok(state.clone())
            }
        }),
        wire: Some(ProjectionWire {
            view_schema: Arc::new(marks_view_schema),
            view: Arc::new(|state| {
                Ok(Some(if state.is_null() {
                    Arc::new(json!({"marks": []}))
                } else {
                    state.clone()
                }))
            }),
            summary,
        }),
        state_version: version,
    }
}

fn title_definition() -> ProjectionDefinition<Value> {
    ProjectionDefinition {
        key: "title".into(),
        state_schema: Arc::new(|value| match value {
            Some(value) if value.is_null() || value.is_string() => Ok(Arc::new(value.clone())),
            _ => Err(failure("invalid title")),
        }),
        init: Arc::new(|_, _| Ok(Arc::new(Value::Null))),
        apply: Arc::new(|state, event| {
            if event.event_type() == "fixtures-test/set-title" {
                Ok(Arc::new(event.data()["title"].clone()))
            } else {
                Ok(state.clone())
            }
        }),
        wire: Some(ProjectionWire {
            view_schema: Arc::new(|value| match value {
                Some(value) if value.is_null() || value.is_string() => Ok(Arc::new(value.clone())),
                _ => Err(failure("invalid title view")),
            }),
            view: Arc::new(|state| Ok(Some(state.clone()))),
            summary: true,
        }),
        state_version: 1,
    }
}

struct FailingCheckpoint;

impl Serialize for FailingCheckpoint {
    fn serialize<S>(&self, _: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Err(S::Error::custom("checkpoint failed"))
    }
}

fn failing_checkpoint_definition() -> ProjectionDefinition<FailingCheckpoint> {
    ProjectionDefinition {
        key: "cache-test/failing-checkpoint".into(),
        state_schema: Arc::new(|_| Ok(Arc::new(FailingCheckpoint))),
        init: Arc::new(|_, _| Ok(Arc::new(FailingCheckpoint))),
        apply: Arc::new(|state, _| Ok(state.clone())),
        wire: None,
        state_version: 1,
    }
}

struct Harness {
    kernel: Kernel,
    ctx: Context,
    root: TempDir,
    cache_fiber: hoh_kernel::Fiber,
}

impl Harness {
    async fn new(config: Value) -> Self {
        Self::at(TempDir::new().unwrap(), config).await
    }

    async fn at(root: TempDir, config: Value) -> Self {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<StoragePlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<StorageJsonPlugin>(json!({"root": root.path().to_string_lossy()}))
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
            .register(&ctx, marks_definition("cache-test/marks", 1, true))
            .await
            .unwrap();
        let cache_fiber = ctx.plugin::<SessionProjectionCachePlugin>(config).unwrap();
        cache_fiber.settled().await.unwrap();
        Self {
            kernel,
            ctx,
            root,
            cache_fiber,
        }
    }

    fn cache(&self) -> hoh_kernel::Service<SessionProjectionCache> {
        self.ctx.get::<SessionProjectionCache>().unwrap()
    }

    fn sessions(&self) -> hoh_kernel::Service<Sessions> {
        self.ctx.get::<Sessions>().unwrap()
    }

    fn projections(&self) -> hoh_kernel::Service<SessionProjections> {
        self.ctx.get::<SessionProjections>().unwrap()
    }

    fn table(&self) -> Arc<hoh_storage_domain::KvTable<hoh_session::SessionId, CheckpointRecord>> {
        self.ctx
            .get::<StorageDomain>()
            .unwrap()
            .get("session_projcache")
            .unwrap()
            .table("sessions")
            .unwrap()
    }

    async fn stop(self) {
        self.kernel.shutdown().await;
    }

    async fn stop_keep(self) -> TempDir {
        self.kernel.shutdown().await;
        self.root
    }
}

fn config(every: u64, interval: u64) -> Value {
    json!({"writeEveryEvents":every,"writeIntervalMs":interval})
}

fn mark(session: &Session, values: &[&str]) -> Arc<SessionEvent> {
    session
        .append("cache-test/mark", json!({"marks":values}), None)
        .unwrap()
}

fn end_turn(session: &Session) -> Arc<SessionEvent> {
    session
        .append(
            "turn/end",
            json!({"turn":1,"reason":{"kind":"completed"}}),
            None,
        )
        .unwrap()
}

async fn record(h: &Harness, id: &str) -> Arc<CheckpointRecord> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(record) = h.table().get(&id.into()).unwrap() {
                return record;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

async fn row_at(h: &Harness, id: &str, seq: i64) -> Arc<CheckpointRecord> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(record) = h.table().get(&id.into()).unwrap()
                && record
                    .rows
                    .get("cache-test/marks")
                    .is_some_and(|row| row.seq == cursor(seq))
            {
                return record;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

fn raw(kind: &str, seq: u64, data: Value) -> Arc<SessionEvent> {
    Arc::new(SessionEvent::from_value(
        json!({"type":kind,"seq":seq,"time":seq,"data":data}),
    ))
}

#[test]
fn domain_declaration_and_checkpoint_schemas_match_the_source() {
    let spec = projection_cache_domain_spec().unwrap();
    assert_eq!(spec.name, "session_projcache");
    assert_eq!(spec.version, 7);
    assert_eq!(spec.compatible_versions, Some(vec![3, 4, 5, 6]));
    assert_eq!(spec.layout, Some(hoh_storage::KvLayout::PerRecord));
    assert_eq!(
        spec.invalid_records,
        Some(hoh_storage_domain::InvalidRecords::BackupAndSkip)
    );
    let parsed = checkpoint_record(&json!({
        "identity":{"formatVersion":4,"createdAt":9,"unknown":true},
        "rows":{"opaque":{"ver":1,"seq":-1,"val":{"__proto__":{"kept":true}},"extra":1}},
        "extra":"stripped"
    }))
    .unwrap();
    assert_eq!(parsed.identity.is_seeded, None);
    assert_eq!(
        parsed.rows["opaque"].val,
        Some(json!({"__proto__":{"kept":true}}))
    );
    assert_eq!(
        serde_json::to_value(parsed).unwrap(),
        json!({
            "identity":{"formatVersion":4,"createdAt":9},
            "rows":{"opaque":{"ver":1,"seq":-1,"val":{"__proto__":{"kept":true}}}}
        })
    );
}

#[test]
fn checkpoint_row_rejects_every_json_lossy_shape_representable_as_json_value() {
    for value in [
        json!({"ver":-1,"seq":0,"val":null}),
        json!({"ver":1.5,"seq":0,"val":null}),
        json!({"ver":1,"seq":-2,"val":null}),
        json!({"ver":1,"seq":0}),
        json!({"ver":1,"seq":0,"val":-0.0}),
        json!({"ver":1,"seq":0,"val":{"nested":-0.0}}),
    ] {
        assert!(checkpoint_row(&value).is_err(), "accepted {value}");
    }
    let valid = json!({"ver":1,"seq":0,"val":{"constructor":false,"nested":[null,true,4,"text"]}});
    assert_eq!(
        checkpoint_row(&valid).unwrap().val,
        valid.get("val").cloned()
    );
}

#[tokio::test]
async fn writes_creation_turn_end_and_direct_clean_checkpoints() {
    let h = Harness::new(config(100, 60_000)).await;
    let session = h
        .sessions()
        .create(&h.ctx, Some("mandatory".into()), Default::default())
        .unwrap();
    assert_eq!(
        record(&h, "mandatory").await.rows["cache-test/marks"].seq,
        SessionSeqCursor::BeforeFirst
    );
    mark(&session, &["a"]);
    let end = end_turn(&session);
    let stored = row_at(&h, "mandatory", end.seq().get() as i64).await;
    assert_eq!(
        stored.rows["cache-test/marks"].val,
        Some(json!({"marks":["a"]}))
    );
    let clean = h
        .sessions()
        .create(&h.ctx, Some("clean".into()), Default::default())
        .unwrap();
    h.cache().write(&clean).await.unwrap();
    assert_eq!(
        h.table().get(&"clean".into()).unwrap().unwrap().rows["cache-test/marks"].seq,
        SessionSeqCursor::BeforeFirst
    );
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rapid_creation_and_turn_write_cannot_be_overwritten_by_the_older_cut() {
    let h = Harness::new(config(100, 60_000)).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let entered_tx = Arc::new(Mutex::new(Some(entered_tx)));
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let release_rx = Arc::new(Mutex::new(Some(release_rx)));
    SessionStore::on_flush(&h.ctx, {
        let calls = calls.clone();
        let entered_tx = entered_tx.clone();
        let release_rx = release_rx.clone();
        move |_| {
            let first = calls.fetch_add(1, Ordering::SeqCst) == 0;
            let entered = first.then(|| Mutex::lock(&entered_tx).unwrap().take().unwrap());
            let release = first.then(|| Mutex::lock(&release_rx).unwrap().take().unwrap());
            async move {
                if let (Some(entered), Some(release)) = (entered, release) {
                    entered.send(()).unwrap();
                    release.await.unwrap();
                }
                Ok(())
            }
        }
    })
    .unwrap();
    let session = h
        .sessions()
        .create(&h.ctx, Some("rapid".into()), Default::default())
        .unwrap();
    entered_rx.await.unwrap();
    mark(&session, &["latest"]);
    let end = end_turn(&session);
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    release_tx.send(()).unwrap();
    row_at(&h, "rapid", end.seq().get() as i64).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let stored = h.table().get(&"rapid".into()).unwrap().unwrap();
    assert_eq!(stored.rows["cache-test/marks"].seq, end.seq().into());
    assert_eq!(
        stored.rows["cache-test/marks"].val,
        Some(json!({"marks":["latest"]}))
    );
    h.stop().await;
}

#[tokio::test]
async fn integral_float_session_header_is_a_valid_checkpoint_identity() {
    let h = Harness::new(config(100, 60_000)).await;
    let session = Session::create(
        "float-header".into(),
        None,
        Some(json!({
            "version": SESSION_FORMAT_VERSION,
            "id": "float-header",
            "createdAt": 1.0,
            "isSeeded": false
        })),
        Some(SessionLogOffset::new(0).unwrap()),
        None,
    )
    .unwrap();
    h.cache().write(&session).await.unwrap();
    assert_eq!(
        h.table()
            .get(&"float-header".into())
            .unwrap()
            .unwrap()
            .identity
            .created_at,
        1
    );
    h.stop().await;
}

#[tokio::test]
async fn creation_captures_a_seed_derived_cut() {
    let h = Harness::new(config(100, 60_000)).await;
    let seed = vec![raw("cache-test/mark", 0, json!({"marks":["seed"]}))];
    h.sessions()
        .create(
            &h.ctx,
            Some("seeded".into()),
            CreateSessionOptions {
                seed: Some(seed),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        record(&h, "seeded").await.rows["cache-test/marks"].val,
        Some(json!({"marks":["seed"]}))
    );
    h.stop().await;
}

#[tokio::test]
async fn disposal_writes_the_live_to_cold_cut() {
    let h = Harness::new(config(100, 60_000)).await;
    let session = h
        .sessions()
        .prepare(Some("detach".into()), Default::default())
        .unwrap();
    let detach = h.sessions().enter(&h.ctx, session.clone()).unwrap();
    h.sessions().announce(&session).unwrap();
    record(&h, "detach").await;
    mark(&session, &["live"]);
    detach.detach();
    let stored = row_at(&h, "detach", 0).await;
    assert_eq!(
        stored.rows["cache-test/marks"].val,
        Some(json!({"marks":["live"]}))
    );
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn count_threshold_and_interval_each_flush_dirty_state() {
    let h = Harness::new(config(3, 30)).await;
    let count = h
        .sessions()
        .create(&h.ctx, Some("count".into()), Default::default())
        .unwrap();
    record(&h, "count").await;
    mark(&count, &["1"]);
    mark(&count, &["2"]);
    assert_eq!(
        h.table().get(&"count".into()).unwrap().unwrap().rows["cache-test/marks"].seq,
        cursor(-1)
    );
    mark(&count, &["3"]);
    assert_eq!(
        row_at(&h, "count", 2).await.rows["cache-test/marks"].val,
        Some(json!({"marks":["3"]}))
    );

    let interval = h
        .sessions()
        .create(&h.ctx, Some("interval".into()), Default::default())
        .unwrap();
    record(&h, "interval").await;
    mark(&interval, &["slow"]);
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_millis(29)).await;
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        h.table().get(&"interval".into()).unwrap().unwrap().rows["cache-test/marks"].seq,
        cursor(-1)
    );
    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(
        row_at(&h, "interval", 0).await.rows["cache-test/marks"].val,
        Some(json!({"marks":["slow"]}))
    );
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn failed_interval_checkpoint_keeps_the_fired_timer_slot_occupied() {
    let h = Harness::new(config(100, 30)).await;
    h.projections()
        .register(&h.ctx, failing_checkpoint_definition())
        .await
        .unwrap();
    let session = h
        .sessions()
        .create(&h.ctx, Some("timer-failure".into()), Default::default())
        .unwrap();
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    let warning_count = || {
        h.ctx
            .logger()
            .buffer()
            .iter()
            .filter(|entry| {
                entry
                    .message
                    .contains("interval write for \"timer-failure\" failed")
            })
            .count()
    };
    mark(&session, &["first"]);
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_millis(30)).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert_eq!(warning_count(), 1);
    mark(&session, &["second"]);
    tokio::time::advance(Duration::from_millis(60)).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert_eq!(warning_count(), 1);
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reentrant_session_flush_can_poll_a_nested_write_without_deadlock() {
    let h = Harness::new(config(100, 60_000)).await;
    let session = h
        .sessions()
        .create(&h.ctx, Some("flush-reentry".into()), Default::default())
        .unwrap();
    record(&h, "flush-reentry").await;
    let cache = h.cache().clone();
    SessionStore::on_flush(&h.ctx, move |session| {
        let cache = cache.clone();
        async move {
            let mut nested = Box::pin(cache.write(&session));
            let mut cx = TaskContext::from_waker(Waker::noop());
            assert!(matches!(nested.as_mut().poll(&mut cx), Poll::Pending));
            Ok(())
        }
    })
    .unwrap();
    h.cache().write(&session).await.unwrap();
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reentrant_domain_change_can_poll_a_nested_write_without_deadlock() {
    let h = Harness::new(config(100, 60_000)).await;
    let session = h
        .sessions()
        .create(&h.ctx, Some("domain-reentry".into()), Default::default())
        .unwrap();
    record(&h, "domain-reentry").await;
    let cache = h.cache().clone();
    let nested_session = session.clone();
    let seen = Arc::new(AtomicUsize::new(0));
    let observed = seen.clone();
    h.ctx
        .on::<DomainChanged>(move |change| {
            let DomainChanged::Put(change) = change else {
                return;
            };
            if change.domain != "session_projcache"
                || change.table != "sessions"
                || change.key != "domain-reentry"
                || observed.fetch_add(1, Ordering::SeqCst) != 0
            {
                return;
            }
            let mut nested = Box::pin(cache.write(&nested_session));
            let mut cx = TaskContext::from_waker(Waker::noop());
            assert!(matches!(nested.as_mut().poll(&mut cx), Poll::Pending));
        })
        .unwrap();
    h.cache().write(&session).await.unwrap();
    assert_eq!(seen.load(Ordering::SeqCst), 1);
    h.stop().await;
}

#[tokio::test]
async fn released_unit_rows_stop_at_their_last_observed_event_and_are_retained() {
    let h = Harness::new(config(100, 60_000)).await;
    let registration = h
        .projections()
        .register(
            &h.ctx,
            marks_definition("cache-test/secondary-marks", 1, true),
        )
        .await
        .unwrap();
    let session = h
        .sessions()
        .prepare(Some("released".into()), Default::default())
        .unwrap();
    let detach = h.sessions().enter(&h.ctx, session.clone()).unwrap();
    h.sessions().announce(&session).unwrap();
    record(&h, "released").await;
    mark(&session, &["a"]);
    let end = end_turn(&session);
    row_at(&h, "released", end.seq().get() as i64).await;
    registration.dispose().await;
    let later = mark(&session, &["b"]);
    detach.detach();
    let stored = row_at(&h, "released", later.seq().get() as i64).await;
    assert_eq!(stored.rows["cache-test/marks"].seq, later.seq().into());
    assert_eq!(
        stored.rows["cache-test/secondary-marks"].seq,
        end.seq().into()
    );
    assert_eq!(
        stored.rows["cache-test/secondary-marks"].val,
        Some(json!({"marks":["a"]}))
    );
    h.stop().await;
}

#[tokio::test]
async fn listing_uses_the_lowest_served_watermark_and_lifecycle_identity() {
    let h = Harness::new(config(100, 60_000)).await;
    h.projections()
        .register(
            &h.ctx,
            marks_definition("cache-test/secondary-marks", 1, true),
        )
        .await
        .unwrap();
    h.table()
        .put(
            "listed".into(),
            CheckpointRecord {
                identity: identity(0),
                rows: IndexMap::from([
                    (
                        "cache-test/marks".into(),
                        row(1, 4, json!({"marks":["primary"]})),
                    ),
                    (
                        "cache-test/secondary-marks".into(),
                        row(1, 2, json!({"marks":["secondary"]})),
                    ),
                ]),
            },
        )
        .await
        .unwrap();
    let snapshot = h
        .cache()
        .cached_snapshot(&header("listed", 0), None)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.as_of_seq, cursor(2));
    assert_eq!(
        serde_json::to_value(snapshot.values).unwrap(),
        json!({
            "cache-test/marks":{"marks":["primary"]},
            "cache-test/secondary-marks":{"marks":["secondary"]}
        })
    );
    assert!(
        h.cache()
            .cached_snapshot(&header("listed", 1), None)
            .unwrap()
            .is_none()
    );
    assert!(
        h.cache()
            .cached_snapshot(&header("missing", 0), None)
            .unwrap()
            .is_none()
    );
    h.table()
        .put(
            "future".into(),
            CheckpointRecord {
                identity: CheckpointIdentity {
                    format_version: Some(SESSION_FORMAT_VERSION + 1),
                    ..identity(0)
                },
                rows: IndexMap::from([(
                    "cache-test/marks".into(),
                    row(1, 0, json!({"marks":["future"]})),
                )]),
            },
        )
        .await
        .unwrap();
    assert!(
        h.cache()
            .cached_snapshot(&header("future", 0), None)
            .unwrap()
            .is_none()
    );
    h.stop().await;
}

#[tokio::test]
async fn listing_matches_seed_lineage_by_header_but_fold_faces_require_the_exact_cut() {
    let h = Harness::new(config(100, 60_000)).await;
    h.table()
        .put(
            "cut".into(),
            CheckpointRecord {
                identity: CheckpointIdentity {
                    format_version: Some(SESSION_FORMAT_VERSION),
                    created_at: 0,
                    cwd: None,
                    is_seeded: Some(true),
                    inherited_event_count: Some(SessionLogOffset::new(2).unwrap()),
                },
                rows: IndexMap::from([(
                    "cache-test/marks".into(),
                    row(1, 1, json!({"marks":["seed"]})),
                )]),
            },
        )
        .await
        .unwrap();
    let mut seeded = header("cut", 0);
    seeded.is_seeded = true;
    assert_eq!(
        h.cache()
            .cached_snapshot(&seeded, None)
            .unwrap()
            .unwrap()
            .values["cache-test/marks"]
            .as_ref(),
        &json!({"marks":["seed"]})
    );
    assert!(
        h.cache()
            .cached_snapshot(&header("cut", 0), None)
            .unwrap()
            .is_none()
    );
    let events = vec![
        raw("turn/start", 0, json!({"turn":1})),
        raw("cache-test/mark", 1, json!({"marks":["logged"]})),
    ];
    assert_eq!(
        h.cache()
            .cold_snapshot(&seeded, SessionLogOffset::new(2).unwrap(), &events)
            .unwrap()
            .values["cache-test/marks"]
            .as_ref(),
        &json!({"marks":["seed"]})
    );
    assert_eq!(
        h.cache()
            .cold_snapshot(&seeded, SessionLogOffset::new(1).unwrap(), &events)
            .unwrap()
            .values["cache-test/marks"]
            .as_ref(),
        &json!({"marks":["logged"]})
    );
    h.stop().await;
}

#[tokio::test]
async fn unseeded_nonzero_cut_is_rejected_with_the_source_text() {
    let h = Harness::new(config(100, 60_000)).await;
    let error = h
        .cache()
        .cold_snapshot(
            &header("bad-cut", 0),
            SessionLogOffset::new(1).unwrap(),
            &[],
        )
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "unseeded projection-cache identity inherited event count must be 0"
    );
    h.stop().await;
}

#[tokio::test]
async fn predecessor_path_serves_only_an_older_matching_title() {
    let h = Harness::new(config(100, 60_000)).await;
    h.projections()
        .register(&h.ctx, title_definition())
        .await
        .unwrap();
    for (id, format, version) in [
        ("older", Some(SESSION_FORMAT_VERSION - 1), 1),
        ("absent", None, 1),
        ("current", Some(SESSION_FORMAT_VERSION), 1),
        ("newer", Some(SESSION_FORMAT_VERSION + 1), 1),
        ("stale-title", Some(SESSION_FORMAT_VERSION - 1), 2),
    ] {
        h.table()
            .put(
                id.into(),
                CheckpointRecord {
                    identity: CheckpointIdentity {
                        format_version: format,
                        created_at: 0,
                        cwd: None,
                        is_seeded: None,
                        inherited_event_count: None,
                    },
                    rows: IndexMap::from([(
                        "title".into(),
                        row(version, 2, json!(format!("{id} title"))),
                    )]),
                },
            )
            .await
            .unwrap();
    }
    for id in ["older", "absent"] {
        assert_eq!(
            h.cache()
                .cached_predecessor_title(&header(id, 0))
                .unwrap()
                .unwrap()
                .as_of_seq,
            cursor(2)
        );
    }
    for id in ["current", "newer", "stale-title", "missing"] {
        assert!(
            h.cache()
                .cached_predecessor_title(&header(id, 0))
                .unwrap()
                .is_none()
        );
    }
    h.table()
        .put(
            "seeded-older".into(),
            CheckpointRecord {
                identity: CheckpointIdentity {
                    format_version: Some(SESSION_FORMAT_VERSION - 1),
                    created_at: 0,
                    cwd: None,
                    is_seeded: Some(true),
                    inherited_event_count: Some(SessionLogOffset::new(5).unwrap()),
                },
                rows: IndexMap::from([("title".into(), row(1, 3, json!("seed title")))]),
            },
        )
        .await
        .unwrap();
    let mut seeded = header("seeded-older", 0);
    seeded.is_seeded = true;
    let title = h
        .cache()
        .cached_predecessor_title(&seeded)
        .unwrap()
        .unwrap();
    assert_eq!(title.as_of_seq, cursor(3));
    assert_eq!(title.values["title"].as_ref(), &json!("seed title"));
    assert!(
        h.cache()
            .cached_predecessor_title(&header("seeded-older", 0))
            .unwrap()
            .is_none()
    );
    h.stop().await;
}

#[tokio::test]
async fn cached_views_exclude_host_only_and_version_mismatched_rows() {
    let h = Harness::new(config(100, 60_000)).await;
    let host = ProjectionDefinition {
        key: "cache-test/secret".into(),
        state_schema: Arc::new(|value| Ok(Arc::new(value.cloned().unwrap_or(Value::Null)))),
        init: Arc::new(|_, _| Ok(Arc::new(json!("")))),
        apply: Arc::new(|state, _| Ok(state.clone())),
        wire: None,
        state_version: 1,
    };
    h.projections().register(&h.ctx, host).await.unwrap();
    h.table()
        .put(
            "host".into(),
            CheckpointRecord {
                identity: identity(0),
                rows: IndexMap::from([
                    (
                        "cache-test/marks".into(),
                        row(1, 4, json!({"marks":["wire"]})),
                    ),
                    (
                        "cache-test/secret".into(),
                        row(1, 1, json!("private prompt text")),
                    ),
                    ("unknown".into(), row(1, 0, json!("opaque"))),
                ]),
            },
        )
        .await
        .unwrap();
    let snapshot = h
        .cache()
        .cached_snapshot(&header("host", 0), None)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.as_of_seq, cursor(4));
    let text = serde_json::to_string(&snapshot.values).unwrap();
    assert!(!text.contains("private prompt text"));
    assert!(!text.contains("unknown"));

    h.table()
        .put(
            "stale".into(),
            CheckpointRecord {
                identity: identity(0),
                rows: IndexMap::from([(
                    "cache-test/marks".into(),
                    row(99, 4, json!({"marks":["old"]})),
                )]),
            },
        )
        .await
        .unwrap();
    assert!(
        h.cache()
            .cached_snapshot(&header("stale", 0), None)
            .unwrap()
            .is_none()
    );
    h.stop().await;
}

#[tokio::test]
async fn key_selection_and_summary_watermarks_follow_only_served_rows() {
    let h = Harness::new(config(100, 60_000)).await;
    h.projections()
        .register(
            &h.ctx,
            marks_definition("cache-test/summary-excluded", 1, false),
        )
        .await
        .unwrap();
    h.table()
        .put(
            "summary".into(),
            CheckpointRecord {
                identity: identity(0),
                rows: IndexMap::from([
                    (
                        "cache-test/marks".into(),
                        row(1, 5, json!({"marks":["primary"]})),
                    ),
                    (
                        "cache-test/summary-excluded".into(),
                        row(1, 1, json!({"marks":["hidden"]})),
                    ),
                ]),
            },
        )
        .await
        .unwrap();
    let summary = h.projections().summary_keys();
    let selected = h
        .cache()
        .cached_snapshot(
            &header("summary", 0),
            Some(ProjectionKeySelection::Set(&summary)),
        )
        .unwrap()
        .unwrap();
    assert_eq!(selected.as_of_seq, cursor(5));
    assert_eq!(selected.values.len(), 1);
    assert_eq!(
        h.cache()
            .cached_snapshot(&header("summary", 0), None)
            .unwrap()
            .unwrap()
            .as_of_seq,
        cursor(1)
    );
    let none = IndexSet::new();
    assert!(
        h.cache()
            .cached_snapshot(
                &header("summary", 0),
                Some(ProjectionKeySelection::Set(&none))
            )
            .unwrap()
            .is_none()
    );
    h.stop().await;
}

#[tokio::test]
async fn cwd_format_and_absent_lineage_have_source_identity_semantics() {
    let h = Harness::new(config(100, 60_000)).await;
    h.table()
        .put(
            "homed".into(),
            CheckpointRecord {
                identity: CheckpointIdentity {
                    format_version: Some(SESSION_FORMAT_VERSION),
                    created_at: 0,
                    cwd: Some("/work".into()),
                    is_seeded: None,
                    inherited_event_count: None,
                },
                rows: IndexMap::from([(
                    "cache-test/marks".into(),
                    row(1, 4, json!({"marks":["kept"]})),
                )]),
            },
        )
        .await
        .unwrap();
    let mut matching = header("homed", 0);
    matching.cwd = Some("/work".into());
    assert!(
        h.cache()
            .cached_snapshot(&matching, None)
            .unwrap()
            .is_some()
    );
    matching.cwd = Some("/elsewhere".into());
    assert!(
        h.cache()
            .cached_snapshot(&matching, None)
            .unwrap()
            .is_none()
    );
    assert!(
        h.cache()
            .cached_snapshot(&header("homed", 0), None)
            .unwrap()
            .is_none()
    );
    let mut seeded = header("homed", 0);
    seeded.cwd = Some("/work".into());
    seeded.is_seeded = true;
    assert!(h.cache().cached_snapshot(&seeded, None).unwrap().is_none());

    h.table()
        .put(
            "pre-lineage".into(),
            CheckpointRecord {
                identity: CheckpointIdentity {
                    format_version: Some(SESSION_FORMAT_VERSION),
                    created_at: 0,
                    cwd: None,
                    is_seeded: None,
                    inherited_event_count: None,
                },
                rows: IndexMap::from([(
                    "cache-test/marks".into(),
                    row(1, 4, json!({"marks":["kept"]})),
                )]),
            },
        )
        .await
        .unwrap();
    let events = (0..5)
        .map(|seq| raw("cache-test/mark", seq, json!({"marks":[format!("m{seq}")]})))
        .collect::<Vec<_>>();
    assert_eq!(
        h.cache()
            .cold_snapshot(
                &header("pre-lineage", 0),
                SessionLogOffset::new(0).unwrap(),
                &events,
            )
            .unwrap()
            .values["cache-test/marks"]
            .as_ref(),
        &json!({"marks":["kept"]})
    );
    let mut seeded_pre_lineage = header("pre-lineage", 0);
    seeded_pre_lineage.is_seeded = true;
    assert_eq!(
        h.cache()
            .cold_snapshot(
                &seeded_pre_lineage,
                SessionLogOffset::new(1).unwrap(),
                &events,
            )
            .unwrap()
            .values["cache-test/marks"]
            .as_ref(),
        &json!({"marks":["m4"]})
    );

    h.table()
        .put(
            "unbound".into(),
            CheckpointRecord {
                identity: CheckpointIdentity {
                    format_version: None,
                    created_at: 0,
                    cwd: None,
                    is_seeded: None,
                    inherited_event_count: None,
                },
                rows: IndexMap::from([(
                    "cache-test/marks".into(),
                    row(1, 4, json!({"marks":["old"]})),
                )]),
            },
        )
        .await
        .unwrap();
    assert!(
        h.cache()
            .cached_snapshot(&header("unbound", 0), None)
            .unwrap()
            .is_none()
    );
    h.stop().await;
}

#[tokio::test]
async fn hydrate_prepared_seeds_matching_rows_and_refolds_malformed_or_absent_rows() {
    let h = Harness::new(config(100, 60_000)).await;
    for (id, value) in [
        ("prepared-seeded", json!({"marks":["cached"]})),
        ("prepared-fallback", json!({"marks":"malformed"})),
    ] {
        h.table()
            .put(
                id.into(),
                CheckpointRecord {
                    identity: identity(0),
                    rows: IndexMap::from([("cache-test/marks".into(), row(1, 1, value))]),
                },
            )
            .await
            .unwrap();
    }
    let events = vec![
        raw("turn/start", 0, json!({"turn":1})),
        raw("cache-test/mark", 1, json!({"marks":["fresh"]})),
        raw(
            "turn/end",
            2,
            json!({"turn":1,"reason":{"kind":"completed"}}),
        ),
    ];
    for (id, expected) in [
        ("prepared-seeded", "cached"),
        ("prepared-fallback", "fresh"),
        ("prepared-bare", "fresh"),
    ] {
        let session = Session::create(
            id.into(),
            Some(&events),
            Some(serde_json::to_value(header(id, 0)).unwrap()),
            None,
            None,
        )
        .unwrap();
        let snapshot = h.cache().hydrate_prepared(&session, &events).unwrap();
        assert_eq!(snapshot.as_of_seq, cursor(2));
        assert_eq!(
            snapshot.values["cache-test/marks"].as_ref(),
            &json!({"marks":[expected]})
        );
    }
    h.stop().await;
}

#[tokio::test]
async fn cold_snapshot_applies_only_the_tail_and_writes_a_refreshed_complete_checkpoint() {
    let h = Harness::new(config(100, 60_000)).await;
    let applies = Arc::new(AtomicUsize::new(0));
    let count = applies.clone();
    h.projections()
        .register(
            &h.ctx,
            ProjectionDefinition {
                key: "cache-test/count".into(),
                state_schema: Arc::new(|value| {
                    value
                        .and_then(Value::as_u64)
                        .map(Arc::new)
                        .ok_or_else(|| failure("invalid count"))
                }),
                init: Arc::new(|_, _| Ok(Arc::new(0_u64))),
                apply: Arc::new(move |_, _| {
                    count.fetch_add(1, Ordering::SeqCst);
                    Ok(Arc::new(1))
                }),
                wire: None,
                state_version: 1,
            },
        )
        .await
        .unwrap();
    h.table()
        .put(
            "cold".into(),
            CheckpointRecord {
                identity: identity(9),
                rows: IndexMap::from([("cache-test/count".into(), row(1, 2, json!(3)))]),
            },
        )
        .await
        .unwrap();
    let events = (0..5)
        .map(|seq| raw("cache-test/mark", seq, json!({"marks":[format!("m{seq}")]})))
        .collect::<Vec<_>>();
    let snapshot = h
        .cache()
        .cold_snapshot(
            &header("cold", 9),
            SessionLogOffset::new(0).unwrap(),
            &events,
        )
        .unwrap();
    assert_eq!(applies.load(Ordering::SeqCst), 2);
    assert_eq!(snapshot.as_of_seq, cursor(4));
    assert!(!snapshot.values.contains_key("cache-test/count"));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if h.table().get(&"cold".into()).unwrap().unwrap().rows["cache-test/count"].seq
                == cursor(4)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    h.cache()
        .cold_snapshot(
            &header("cold-fresh", 10),
            SessionLogOffset::new(0).unwrap(),
            &events,
        )
        .unwrap();
    assert_eq!(applies.load(Ordering::SeqCst), 7);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if h.table()
                .get(&"cold-fresh".into())
                .unwrap()
                .is_some_and(|record| record.rows["cache-test/count"].seq == cursor(4))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    h.stop().await;
}

#[tokio::test]
async fn durable_failures_are_warnings_and_do_not_escape_the_event_path() {
    let h = Harness::new(config(100, 60_000)).await;
    let blocker = h
        .root
        .path()
        .join("session_projcache/sessions/fail-soft.json");
    tokio::fs::create_dir_all(&blocker).await.unwrap();
    let session = h
        .sessions()
        .create(&h.ctx, Some("fail-soft".into()), Default::default())
        .unwrap();
    mark(&session, &["x"]);
    end_turn(&session);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if h.ctx.logger().buffer().iter().any(|record| {
                record.message.contains(
                    "session projection cache: turn/end write for \"fail-soft\" failed (cache stays stale)",
                )
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(h.table().get(&"fail-soft".into()).unwrap().is_none());
    tokio::fs::remove_dir_all(&blocker).await.unwrap();
    mark(&session, &["y"]);
    let end = end_turn(&session);
    let stored = row_at(&h, "fail-soft", end.seq().get() as i64).await;
    assert_eq!(
        stored.rows["cache-test/marks"].val,
        Some(json!({"marks":["y"]}))
    );
    h.stop().await;
}

#[tokio::test]
async fn write_rejects_a_projection_state_that_is_not_lossless_json() {
    let h = Harness::new(config(100, 60_000)).await;
    h.projections()
        .register(
            &h.ctx,
            ProjectionDefinition {
                key: "cache-test/negative-zero".into(),
                state_schema: Arc::new(|value| {
                    value
                        .and_then(Value::as_f64)
                        .map(Arc::new)
                        .ok_or_else(|| failure("invalid number"))
                }),
                init: Arc::new(|_, _| Ok(Arc::new(-0.0_f64))),
                apply: Arc::new(|state, _| Ok(state.clone())),
                wire: None,
                state_version: 1,
            },
        )
        .await
        .unwrap();
    let session = h
        .sessions()
        .create(&h.ctx, Some("bad-json".into()), Default::default())
        .unwrap();
    let error = h.cache().write(&session).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("not losslessly JSON-serializable")
    );
    h.stop().await;
}

#[tokio::test]
async fn cold_read_write_back_failure_is_warning_only() {
    let h = Harness::new(config(100, 60_000)).await;
    tokio::fs::create_dir_all(
        h.root
            .path()
            .join("session_projcache/sessions/cold-fail.json"),
    )
    .await
    .unwrap();
    let snapshot = h
        .cache()
        .cold_snapshot(
            &header("cold-fail", 0),
            SessionLogOffset::new(0).unwrap(),
            &[],
        )
        .unwrap();
    assert_eq!(snapshot.as_of_seq, cursor(-1));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if h.ctx.logger().buffer().iter().any(|record| {
                record
                    .message
                    .contains("cold-read write-back for \"cold-fail\" failed (cache stays stale)")
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    h.stop().await;
}

#[tokio::test]
async fn current_write_replaces_unknown_and_foreign_lifecycle_rows() {
    let h = Harness::new(config(100, 60_000)).await;
    let session = h
        .sessions()
        .create(&h.ctx, Some("recreated".into()), Default::default())
        .unwrap();
    record(&h, "recreated").await;
    let current_header: SessionHeader =
        serde_json::from_value(session.header().as_ref().clone()).unwrap();
    h.table()
        .put(
            "recreated".into(),
            CheckpointRecord {
                identity: CheckpointIdentity {
                    format_version: Some(SESSION_FORMAT_VERSION),
                    created_at: u64::try_from(current_header.created_at - 1).unwrap(),
                    cwd: None,
                    is_seeded: Some(false),
                    inherited_event_count: Some(SessionLogOffset::new(0).unwrap()),
                },
                rows: IndexMap::from([
                    ("cache-test/marks".into(), row(1, -1, Value::Null)),
                    ("cache-test/secret".into(), row(1, -1, json!("foreign"))),
                ]),
            },
        )
        .await
        .unwrap();
    h.cache().write(&session).await.unwrap();
    let stored = h.table().get(&"recreated".into()).unwrap().unwrap();
    assert_eq!(stored.identity.created_at, current_header.created_at as u64);
    assert_eq!(stored.rows.keys().collect::<Vec<_>>(), ["cache-test/marks"]);
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn plugin_disposal_cancels_armed_interval_writes() {
    let h = Harness::new(config(100, 40)).await;
    let session = h
        .sessions()
        .create(&h.ctx, Some("armed".into()), Default::default())
        .unwrap();
    record(&h, "armed").await;
    mark(&session, &["pending"]);
    let cleaned = h
        .sessions()
        .create(&h.ctx, Some("cleaned".into()), Default::default())
        .unwrap();
    record(&h, "cleaned").await;
    mark(&cleaned, &["durable"]);
    h.cache().write(&cleaned).await.unwrap();
    let table = h.table();
    h.cache_fiber.dispose().await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(
        table.get(&"armed".into()).unwrap_err().to_string(),
        "domain 'session_projcache' is closed"
    );
    let path = h.root.path().join("session_projcache/sessions/armed.json");
    let doc: Value = serde_json::from_slice(&tokio::fs::read(path).await.unwrap()).unwrap();
    assert_eq!(doc["record"]["rows"]["cache-test/marks"]["seq"], -1);
    let cleaned_path = h
        .root
        .path()
        .join("session_projcache/sessions/cleaned.json");
    let cleaned_doc: Value =
        serde_json::from_slice(&tokio::fs::read(cleaned_path).await.unwrap()).unwrap();
    assert_eq!(
        cleaned_doc["record"]["rows"]["cache-test/marks"]["val"],
        json!({"marks":["durable"]})
    );
    h.stop().await;
}

#[tokio::test]
async fn zero_throttle_values_are_rejected() {
    for config in [config(0, 1), config(1, 0)] {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<StoragePlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let root = TempDir::new().unwrap();
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
        let fiber = ctx.plugin::<SessionProjectionCachePlugin>(config).unwrap();
        assert!(fiber.settled().await.is_err());
        kernel.shutdown().await;
    }
    let h = Harness::new(json!({"writeEveryEvents":1.0,"writeIntervalMs":1.0})).await;
    h.stop().await;
    let h = Harness::new(config(u64::MAX, u64::MAX)).await;
    h.stop().await;
}

fn source_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

async fn place_fixture(root: &Path, id: &str, name: &str) {
    let dir = root.join("session_projcache/sessions");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    tokio::fs::copy(source_fixture(name), dir.join(format!("{id}.json")))
        .await
        .unwrap();
}

#[tokio::test]
async fn accepted_v4_v5_and_lineageless_archives_open_only_as_predecessor_title_hints() {
    for name in [
        "v4-session-doc.json",
        "v5-session-doc.json",
        "v5-lineageless-doc.json",
    ] {
        let root = TempDir::new().unwrap();
        place_fixture(root.path(), "fixture-session", name).await;
        let h = Harness::at(root, config(100, 60_000)).await;
        h.projections()
            .register(&h.ctx, title_definition())
            .await
            .unwrap();
        let doc: Value =
            serde_json::from_slice(&tokio::fs::read(source_fixture(name)).await.unwrap()).unwrap();
        let created = doc["record"]["identity"]["createdAt"].as_i64().unwrap();
        let mut meta = header("fixture-session", created);
        meta.cwd = doc["record"]["identity"]["cwd"].as_str().map(str::to_owned);
        assert!(h.cache().cached_snapshot(&meta, None).unwrap().is_none());
        let predecessor = h.cache().cached_predecessor_title(&meta).unwrap().unwrap();
        assert_eq!(
            predecessor.values["title"].as_ref(),
            &doc["record"]["rows"]["title"]["val"]
        );
        let title_keys = ["title".to_owned()];
        let selected = h
            .cache()
            .cached_snapshot(&meta, Some(ProjectionKeySelection::List(&title_keys)))
            .unwrap();
        assert!(selected.is_none());
        if name == "v5-lineageless-doc.json" {
            let mut seeded = meta.clone();
            seeded.is_seeded = true;
            assert!(h.cache().cached_snapshot(&seeded, None).unwrap().is_none());
            assert!(
                h.cache()
                    .cached_predecessor_title(&seeded)
                    .unwrap()
                    .is_none()
            );
        }
        let session = h
            .sessions()
            .create(&h.ctx, Some("fixture-session".into()), Default::default())
            .unwrap();
        session
            .append("fixtures-test/set-title", json!({"title":"重写标题"}), None)
            .unwrap();
        h.cache().write(&session).await.unwrap();
        let rewritten = h.table().get(&"fixture-session".into()).unwrap().unwrap();
        assert_eq!(
            rewritten.identity.format_version,
            Some(SESSION_FORMAT_VERSION)
        );
        assert_eq!(rewritten.identity.is_seeded, Some(false));
        assert_eq!(
            rewritten.identity.inherited_event_count,
            Some(SessionLogOffset::new(0).unwrap())
        );
        assert_eq!(rewritten.rows["title"].val, Some(json!("重写标题")));
        let current: SessionHeader =
            serde_json::from_value(session.header().as_ref().clone()).unwrap();
        let title_keys = ["title".to_owned()];
        let selected = h
            .cache()
            .cached_snapshot(&current, Some(ProjectionKeySelection::List(&title_keys)))
            .unwrap()
            .unwrap();
        assert_eq!(selected.values.len(), 1);
        assert_eq!(selected.values["title"].as_ref(), &json!("重写标题"));
        let on_disk: Value = serde_json::from_slice(
            &tokio::fs::read(
                h.root
                    .path()
                    .join("session_projcache/sessions/fixture-session.json"),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(on_disk["version"], 7);
        h.stop().await;
    }
}

#[tokio::test]
async fn opaque_v7_fixture_preserves_every_json_key_through_domain_read_and_put() {
    let root = TempDir::new().unwrap();
    place_fixture(root.path(), "opaque-fixture", "v7-opaque-session-doc.json").await;
    let archived: Value = serde_json::from_slice(
        &tokio::fs::read(source_fixture("v7-opaque-session-doc.json"))
            .await
            .unwrap(),
    )
    .unwrap();
    let h = Harness::at(root, config(100, 60_000)).await;
    let record = h.table().get(&"opaque-fixture".into()).unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(&record.rows).unwrap(),
        archived["record"]["rows"]
    );
    let mut rewritten = record.as_ref().clone();
    rewritten.identity.format_version = Some(SESSION_FORMAT_VERSION);
    h.table()
        .put("opaque-fixture".into(), rewritten.clone())
        .await
        .unwrap();
    let on_disk: Value = serde_json::from_slice(
        &tokio::fs::read(
            h.root
                .path()
                .join("session_projcache/sessions/opaque-fixture.json"),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(on_disk["record"], serde_json::to_value(&rewritten).unwrap());
    assert_eq!(on_disk["record"]["rows"], archived["record"]["rows"]);
    let root = h.stop_keep().await;
    let reopened = Harness::at(root, config(100, 60_000)).await;
    let restored = reopened
        .table()
        .get(&"opaque-fixture".into())
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(restored.as_ref()).unwrap(),
        serde_json::to_value(&rewritten).unwrap()
    );
    reopened.stop().await;
}

#[tokio::test]
async fn unsupported_or_malformed_documents_read_as_absent() {
    for (id, bytes) in [
        (
            "unsupported",
            br#"{"version":2,"record":{"identity":{"formatVersion":4,"createdAt":0},"rows":{"cache-test/marks":{"ver":1,"seq":0,"val":{"marks":["old"]}}}}}"#.as_slice(),
        ),
        ("malformed", b"not json at all".as_slice()),
    ] {
        let root = TempDir::new().unwrap();
        let dir = root.path().join("session_projcache/sessions");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join(format!("{id}.json")), bytes)
            .await
            .unwrap();
        let h = Harness::at(root, config(100, 60_000)).await;
        assert!(h.cache().cached_snapshot(&header(id, 0), None).unwrap().is_none());
        h.stop().await;
    }
}

#[tokio::test]
async fn v3_whole_unit_archive_bootstraps_and_malformed_records_are_backed_up() {
    let root = TempDir::new().unwrap();
    tokio::fs::copy(
        source_fixture("v3-single-unit.json"),
        root.path().join("session_projcache.json"),
    )
    .await
    .unwrap();
    let h = Harness::at(root, config(100, 60_000)).await;
    h.projections()
        .register(&h.ctx, title_definition())
        .await
        .unwrap();
    let archive: Value = serde_json::from_slice(
        &tokio::fs::read(source_fixture("v3-single-unit.json"))
            .await
            .unwrap(),
    )
    .unwrap();
    let (id, archived) = archive["tables"]["sessions"]
        .as_object()
        .unwrap()
        .iter()
        .next()
        .unwrap();
    let mut meta = header(id, archived["identity"]["createdAt"].as_i64().unwrap());
    meta.cwd = archived["identity"]["cwd"].as_str().map(str::to_owned);
    assert!(h.cache().cached_snapshot(&meta, None).unwrap().is_none());
    let predecessor = h.cache().cached_predecessor_title(&meta).unwrap().unwrap();
    assert_eq!(
        predecessor.values["title"].as_ref(),
        &archived["rows"]["title"]["val"]
    );
    let materialized: Value = serde_json::from_slice(
        &tokio::fs::read(
            h.root
                .path()
                .join(format!("session_projcache/sessions/{id}.json")),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(materialized["version"], 7);
    let session = h
        .sessions()
        .create(&h.ctx, Some(id.as_str().into()), Default::default())
        .unwrap();
    session
        .append("fixtures-test/set-title", json!({"title":"重写标题"}), None)
        .unwrap();
    h.cache().write(&session).await.unwrap();
    let rewritten = h.table().get(&id.as_str().into()).unwrap().unwrap();
    assert_eq!(
        rewritten.identity.format_version,
        Some(SESSION_FORMAT_VERSION)
    );
    assert_eq!(rewritten.identity.is_seeded, Some(false));
    assert_eq!(
        rewritten.identity.inherited_event_count,
        Some(SessionLogOffset::new(0).unwrap())
    );
    assert_eq!(rewritten.rows["title"].val, Some(json!("重写标题")));
    h.stop().await;

    let root = TempDir::new().unwrap();
    let dir = root.path().join("session_projcache/sessions");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    tokio::fs::write(
        dir.join("broken.json"),
        br#"{"version":7,"record":{"identity":{"createdAt":"not-a-number"},"rows":"not-an-object"}}"#,
    )
    .await
    .unwrap();
    place_fixture(root.path(), "survivor", "v5-session-doc.json").await;
    let h = Harness::at(root, config(100, 60_000)).await;
    h.projections()
        .register(&h.ctx, title_definition())
        .await
        .unwrap();
    let names = std::fs::read_dir(h.root.path().join("session_projcache/sessions"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(!names.iter().any(|name| name == "broken.json"));
    assert!(
        names
            .iter()
            .any(|name| name.starts_with("broken.json.bak."))
    );
    assert!(h.ctx.logger().buffer().iter().any(|record| {
        record.message.contains("stored record 'broken'") && record.message.contains("moved to")
    }));
    let survivor: Value = serde_json::from_slice(
        &tokio::fs::read(source_fixture("v5-session-doc.json"))
            .await
            .unwrap(),
    )
    .unwrap();
    let mut survivor_header = header(
        "survivor",
        survivor["record"]["identity"]["createdAt"]
            .as_i64()
            .unwrap(),
    );
    survivor_header.cwd = survivor["record"]["identity"]["cwd"]
        .as_str()
        .map(str::to_owned);
    assert!(
        h.cache()
            .cached_snapshot(&survivor_header, None)
            .unwrap()
            .is_none()
    );
    let session = h
        .sessions()
        .create(&h.ctx, Some("survivor".into()), Default::default())
        .unwrap();
    session
        .append("fixtures-test/set-title", json!({"title":"重写标题"}), None)
        .unwrap();
    h.cache().write(&session).await.unwrap();
    let rewritten = h.table().get(&"survivor".into()).unwrap().unwrap();
    assert_eq!(
        rewritten.identity.format_version,
        Some(SESSION_FORMAT_VERSION)
    );
    assert_eq!(rewritten.rows["title"].val, Some(json!("重写标题")));
    h.stop().await;
}
