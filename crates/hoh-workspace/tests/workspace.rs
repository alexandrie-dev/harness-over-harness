use hoh_error::BoxError;
use hoh_kernel::{Context, Kernel};
use hoh_session::{SessionHeader, SessionId};
use hoh_session_persistence::{
    SessionAccess, SessionHandle, SessionPersistence, SessionPersistenceCreateOptions,
    SessionPersistenceIdentity, SessionPersistenceListOptions, SessionPersistenceOpenOptions,
    SessionPersistenceRevision, SessionPersistenceService, SessionPersistenceSnapshot,
    SessionPersistenceStatOptions,
};
use hoh_storage::{
    BackendFuture, KvFacet, KvSnapshot, KvUnit, KvUnitDescriptor, Storage, StorageBackend,
    StorageService,
};
use hoh_storage_domain::{Config as DomainConfig, DomainFacility, StorageDomain};
use hoh_workspace::*;
use indexmap::IndexMap;
use std::collections::HashSet;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use tempfile::TempDir;

#[derive(Default)]
struct Failures {
    puts: AtomicU64,
    deletes: AtomicU64,
    globals: AtomicU64,
    put_at: Mutex<HashSet<u64>>,
    delete_at: Mutex<HashSet<u64>>,
    global_at: Mutex<HashSet<u64>>,
}
struct Medium {
    snapshot: Mutex<KvSnapshot>,
    failures: Failures,
    next_put_gate: Mutex<Option<Arc<WriteGate>>>,
}
#[derive(Default)]
struct WriteGate {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl Default for Medium {
    fn default() -> Self {
        Self {
            snapshot: Mutex::new(KvSnapshot {
                tables: IndexMap::new(),
                global: serde_json::Value::Null,
            }),
            failures: Failures::default(),
            next_put_gate: Mutex::new(None),
        }
    }
}
impl Medium {
    fn fail(set: &Mutex<HashSet<u64>>, counter: &AtomicU64, message: &str) -> Result<(), BoxError> {
        let at = counter.fetch_add(1, Ordering::SeqCst) + 1;
        if set.lock().unwrap().remove(&at) {
            Err(message.to_owned().into())
        } else {
            Ok(())
        }
    }
    fn fail_next_put(&self) {
        let at = self.failures.puts.load(Ordering::SeqCst) + 1;
        self.failures.put_at.lock().unwrap().insert(at);
    }
    fn fail_put_offset(&self, offset: u64) {
        let at = self.failures.puts.load(Ordering::SeqCst) + offset;
        self.failures.put_at.lock().unwrap().insert(at);
    }
    fn fail_next_delete(&self) {
        let at = self.failures.deletes.load(Ordering::SeqCst) + 1;
        self.failures.delete_at.lock().unwrap().insert(at);
    }
    fn fail_next_global(&self) {
        let at = self.failures.globals.load(Ordering::SeqCst) + 1;
        self.failures.global_at.lock().unwrap().insert(at);
    }
    fn fail_global_offset(&self, offset: u64) {
        let at = self.failures.globals.load(Ordering::SeqCst) + offset;
        self.failures.global_at.lock().unwrap().insert(at);
    }
    fn gate_next_put(&self) -> Arc<WriteGate> {
        let gate = Arc::new(WriteGate::default());
        *self.next_put_gate.lock().unwrap() = Some(gate.clone());
        gate
    }
}
struct Backend(Arc<Medium>);
impl StorageBackend for Backend {
    fn kv(&self) -> Option<&dyn KvFacet> {
        Some(self)
    }
    fn close(&self) -> BackendFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}
impl KvFacet for Backend {
    fn open(&self, descriptor: KvUnitDescriptor) -> BackendFuture<'_, Arc<dyn KvUnit>> {
        let medium = self.0.clone();
        Box::pin(async move {
            let mut snapshot = medium.snapshot.lock().unwrap();
            for table in descriptor.tables {
                snapshot.tables.entry(table).or_default();
            }
            drop(snapshot);
            Ok(Arc::new(Unit(medium)) as Arc<dyn KvUnit>)
        })
    }
}
struct Unit(Arc<Medium>);
impl KvUnit for Unit {
    fn load_all(&self) -> BackendFuture<'_, KvSnapshot> {
        let value = self.0.snapshot.lock().unwrap().clone();
        Box::pin(async move { Ok(value) })
    }
    fn put_record<'a>(
        &'a self,
        table: &'a str,
        key: &'a str,
        value: serde_json::Value,
    ) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            let gate = self.0.next_put_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                gate.entered.notify_one();
                gate.release.notified().await;
            }
            Medium::fail(
                &self.0.failures.put_at,
                &self.0.failures.puts,
                "selected bootstrap put failure",
            )?;
            self.0
                .snapshot
                .lock()
                .unwrap()
                .tables
                .get_mut(table)
                .unwrap()
                .insert(key.into(), value);
            Ok(())
        })
    }
    fn delete_record<'a>(&'a self, table: &'a str, key: &'a str) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            Medium::fail(
                &self.0.failures.delete_at,
                &self.0.failures.deletes,
                "selected rollback delete failure",
            )?;
            self.0
                .snapshot
                .lock()
                .unwrap()
                .tables
                .get_mut(table)
                .unwrap()
                .shift_remove(key);
            Ok(())
        })
    }
    fn delete_records<'a>(&'a self, table: &'a str, keys: &'a [String]) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            let mut s = self.0.snapshot.lock().unwrap();
            for key in keys {
                s.tables.get_mut(table).unwrap().shift_remove(key);
            }
            Ok(())
        })
    }
    fn set_global(&self, value: serde_json::Value) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            Medium::fail(
                &self.0.failures.global_at,
                &self.0.failures.globals,
                "selected bootstrap marker failure",
            )?;
            self.0.snapshot.lock().unwrap().global = value;
            Ok(())
        })
    }
    fn close(&self) -> BackendFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

struct Persistence {
    identity: SessionPersistenceIdentity,
    headers: Mutex<Vec<SessionHeader>>,
    fail: Mutex<Option<String>>,
    lists: AtomicU64,
}
impl Persistence {
    fn new(headers: Vec<SessionHeader>) -> Self {
        Self {
            identity: SessionPersistenceIdentity::new(),
            headers: Mutex::new(headers),
            fail: Mutex::new(None),
            lists: AtomicU64::new(0),
        }
    }
}
impl SessionPersistence for Persistence {
    fn identity(&self) -> SessionPersistenceIdentity {
        self.identity.clone()
    }
    fn create<'a>(
        &'a self,
        _: &'a SessionHeader,
        _: Option<&'a SessionPersistenceCreateOptions>,
    ) -> futures_util::future::BoxFuture<'a, Result<Arc<dyn SessionHandle>, BoxError>> {
        Box::pin(async { Err("unused create".into()) })
    }
    fn open<'a>(
        &'a self,
        _: &'a SessionId,
        _: SessionAccess,
        _: Option<&'a SessionPersistenceOpenOptions>,
    ) -> futures_util::future::BoxFuture<'a, Result<Arc<dyn SessionHandle>, BoxError>> {
        Box::pin(async { Err("unused open".into()) })
    }
    fn flush(&self) -> futures_util::future::BoxFuture<'_, Result<(), BoxError>> {
        Box::pin(async { Ok(()) })
    }
    fn stat<'a>(
        &'a self,
        _: &'a SessionId,
        _: Option<&'a SessionPersistenceStatOptions>,
    ) -> futures_util::future::BoxFuture<'a, Result<Option<SessionPersistenceSnapshot>, BoxError>>
    {
        Box::pin(async { Err("unused stat".into()) })
    }
    fn list<'a>(
        &'a self,
        _: Option<&'a SessionPersistenceListOptions>,
    ) -> futures_util::future::BoxFuture<'a, Result<Vec<SessionPersistenceSnapshot>, BoxError>>
    {
        Box::pin(async move {
            self.lists.fetch_add(1, Ordering::SeqCst);
            if let Some(e) = self.fail.lock().unwrap().take() {
                return Err(e.into());
            }
            Ok(self
                .headers
                .lock()
                .unwrap()
                .iter()
                .cloned()
                .map(|header| SessionPersistenceSnapshot {
                    revision: SessionPersistenceRevision::from(format!("rev-{}", header.id)),
                    header,
                    format_status: None,
                    event_count: None,
                    size_bytes: None,
                    location: None,
                })
                .collect())
        })
    }
}

struct Harness {
    kernel: Kernel,
    ctx: Context,
    registry: WorkspaceRegistry,
    persistence: Arc<Persistence>,
    medium: Arc<Medium>,
    sessions: Option<Arc<hoh_session::SessionStore>>,
}
impl Harness {
    async fn new(headers: Vec<SessionHeader>) -> Self {
        Self::with_medium(headers, Arc::new(Medium::default())).await
    }
    async fn with_medium(headers: Vec<SessionHeader>, medium: Arc<Medium>) -> Self {
        Self::try_with_medium(headers, medium).await.unwrap()
    }
    async fn try_with_medium(
        headers: Vec<SessionHeader>,
        medium: Arc<Medium>,
    ) -> Result<Self, BoxError> {
        Self::try_with_options(headers, medium, false).await
    }
    async fn with_session_store(headers: Vec<SessionHeader>, medium: Arc<Medium>) -> Self {
        Self::try_with_options(headers, medium, true).await.unwrap()
    }
    async fn try_with_options(
        headers: Vec<SessionHeader>,
        medium: Arc<Medium>,
        session_store: bool,
    ) -> Result<Self, BoxError> {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let storage = Arc::new(StorageService::new());
        let _registration = storage
            .backend
            .register("memory", Arc::new(Backend(medium.clone())))
            .unwrap();
        ctx.provide::<Storage>(storage).await.unwrap();
        let facility = Arc::new(DomainFacility::new(
            ctx.clone(),
            DomainConfig {
                backend: "memory".into(),
                routes: IndexMap::new(),
            },
        ));
        ctx.provide::<StorageDomain>(facility.clone())
            .await
            .unwrap();
        let persistence = Arc::new(Persistence::new(headers));
        ctx.provide::<SessionPersistenceService>(persistence.clone())
            .await
            .unwrap();
        let sessions = session_store.then(|| Arc::new(hoh_session::SessionStore::new()));
        if let Some(sessions) = &sessions {
            ctx.provide::<hoh_session::Sessions>(sessions.clone())
                .await
                .unwrap();
        }
        let facility_service = ctx.get::<StorageDomain>().unwrap();
        let persistence_service = ctx.get::<SessionPersistenceService>().unwrap();
        let registry =
            WorkspaceRegistry::start(ctx.clone(), facility_service, persistence_service).await?;
        Ok(Self {
            kernel,
            ctx,
            registry,
            persistence,
            medium,
            sessions,
        })
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        let _ = &self.kernel;
    }
}

fn header(id: &str, cwd: Option<&str>, created: i64) -> SessionHeader {
    let mut h = SessionHeader::new(id.into(), created);
    h.cwd = cwd.map(str::to_owned);
    h
}
fn medium_state(h: &Harness) -> serde_json::Value {
    h.medium.snapshot.lock().unwrap().global.clone()
}
fn stored_workspace(path: &str, sessions: &[&str], created_at: &str) -> serde_json::Value {
    serde_json::json!({
        "path": path,
        "title": "stored",
        "sessionIds": sessions,
        "createdAt": created_at,
        "updatedAt": created_at,
    })
}
fn seed_medium(
    medium: &Medium,
    records: impl IntoIterator<Item = (&'static str, serde_json::Value)>,
    state: serde_json::Value,
) {
    let mut snapshot = medium.snapshot.lock().unwrap();
    snapshot.tables.insert(
        "workspaces".into(),
        records
            .into_iter()
            .map(|(id, record)| (id.to_owned(), record))
            .collect(),
    );
    snapshot.global = state;
}

#[test]
fn accepts_fully_qualified_roots_and_directories_without_accepting_drive_relative_paths() {
    assert!(fully_qualified_workspace_path_for(
        "C:\\",
        PathPlatform::Windows
    ));
    assert!(fully_qualified_workspace_path_for(
        "C:\\work",
        PathPlatform::Windows
    ));
    assert!(fully_qualified_workspace_path_for(
        "\\\\server\\share",
        PathPlatform::Windows
    ));
    assert_eq!(
        default_workspace_title_for("C:\\", PathPlatform::Windows),
        "C:\\"
    );
    assert_eq!(
        default_workspace_title_for("C:\\work", PathPlatform::Windows),
        "work"
    );
    assert_eq!(
        default_workspace_title_for("\\\\server\\share", PathPlatform::Windows),
        "share"
    );
    assert!(!fully_qualified_workspace_path_for(
        "C:",
        PathPlatform::Windows
    ));
    assert!(!fully_qualified_workspace_path_for(
        "C:work",
        PathPlatform::Windows
    ));
    assert!(!fully_qualified_workspace_path_for(
        "\\work",
        PathPlatform::Windows
    ));
    assert_eq!(default_workspace_title_for("/", PathPlatform::Posix), "/");
}

#[tokio::test]
async fn bootstraps_once_from_list_headers_only_in_workspace_session_created_at_order() {
    let temp = TempDir::new().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let h = Harness::new(vec![
        header("old", Some(a.to_str().unwrap()), 1),
        header("new", Some(b.to_str().unwrap()), 2),
    ])
    .await;
    let list = h.registry.list().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(
        list[0].path(),
        std::fs::canonicalize(b).unwrap().to_string_lossy()
    );
    assert_eq!(list[0].session_ids(), vec![SessionId::from("new")]);
}

#[tokio::test]
async fn creates_newest_first_and_idempotently_reuses_a_canonical_path_without_retitling() {
    let temp = TempDir::new().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let h = Harness::new(vec![]).await;
    let first = h
        .registry
        .create(a.to_str().unwrap(), Some("Original".into()))
        .await
        .unwrap();
    let second = h.registry.create(b.to_str().unwrap(), None).await.unwrap();
    let reused = h
        .registry
        .create(a.to_str().unwrap(), Some("Ignored".into()))
        .await
        .unwrap();
    assert_eq!(first.id(), reused.id());
    assert_eq!(first.title(), "Original");
    assert_eq!(
        h.registry
            .list()
            .unwrap()
            .iter()
            .map(|w| w.id())
            .collect::<Vec<_>>(),
        vec![second.id(), first.id()]
    );
}

#[tokio::test]
async fn serializes_concurrent_same_path_creates_into_one_entity() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().to_string_lossy().into_owned();
    let h = Harness::new(vec![]).await;
    let (a, b) = tokio::join!(
        h.registry.create(&path, Some("A".into())),
        h.registry.create(&path, Some("B".into()))
    );
    assert_eq!(a.unwrap().id(), b.unwrap().id());
    assert_eq!(h.registry.list().unwrap().len(), 1);
}

#[tokio::test]
async fn rejects_nonexistent_non_directory_and_relative_paths_without_changing_order() {
    let temp = TempDir::new().unwrap();
    let h = Harness::new(vec![]).await;
    assert!(
        h.registry
            .create("relative", None)
            .await
            .unwrap_err()
            .to_string()
            .contains("not fully qualified")
    );
    let missing = temp.path().join("missing");
    let missing_error = h
        .registry
        .create(missing.to_str().unwrap(), None)
        .await
        .unwrap_err();
    let missing_error = missing_error
        .downcast_ref::<WorkspacePathError>()
        .expect("missing create preserves WorkspacePathError");
    assert!(matches!(
        missing_error,
        WorkspacePathError::Io(error) if error.kind() == std::io::ErrorKind::NotFound
    ));
    let file = temp.path().join("file");
    std::fs::write(&file, "x").unwrap();
    assert!(
        h.registry
            .create(file.to_str().unwrap(), None)
            .await
            .unwrap_err()
            .to_string()
            .contains("not a directory")
    );
    assert!(h.registry.list().unwrap().is_empty());
}

#[tokio::test]
async fn deletes_only_the_registration_and_leaves_its_directory_and_session_headers_untouched() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().to_string_lossy().into_owned();
    let h = Harness::new(vec![]).await;
    let w = h.registry.create(&path, None).await.unwrap();
    assert!(h.registry.delete(w.id()).await.unwrap());
    assert!(!h.registry.delete(w.id()).await.unwrap());
    assert!(temp.path().exists());
    assert!(h.registry.list().unwrap().is_empty());
}

#[tokio::test]
async fn moves_a_workspace_before_an_anchor_or_to_the_end() {
    let temp = TempDir::new().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let h = Harness::new(vec![]).await;
    let wa = h.registry.create(a.to_str().unwrap(), None).await.unwrap();
    let wb = h.registry.create(b.to_str().unwrap(), None).await.unwrap();
    assert_eq!(
        h.registry
            .insert_before(wa.id(), Some(wb.id()))
            .await
            .unwrap(),
        vec![wa.id().clone(), wb.id().clone()]
    );
    assert_eq!(
        h.registry.insert_before(wa.id(), None).await.unwrap(),
        vec![wb.id().clone(), wa.id().clone()]
    );
    assert!(
        h.registry
            .insert_before(&"ghost".into(), None)
            .await
            .unwrap_err()
            .is::<WorkspaceOrderInvalidError>()
    );
}

#[tokio::test]
async fn prepends_new_attaches_moves_detaches_and_keeps_repeat_attach_idempotent() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().to_string_lossy().into_owned();
    let h = Harness::new(vec![]).await;
    let w = h.registry.create(&path, None).await.unwrap();
    *h.persistence.headers.lock().unwrap() =
        vec![header("one", Some(&path), 1), header("two", Some(&path), 2)];
    w.attach_session("one".into()).await.unwrap();
    w.attach_session("two".into()).await.unwrap();
    w.attach_session("one".into()).await.unwrap();
    assert_eq!(
        w.session_ids(),
        vec![SessionId::from("two"), SessionId::from("one")]
    );
    w.insert_session_before("one".into(), Some("two".into()))
        .await
        .unwrap();
    assert_eq!(
        w.session_ids(),
        vec![SessionId::from("one"), SessionId::from("two")]
    );
    w.detach_session("two".into()).await.unwrap();
    assert_eq!(w.session_ids(), vec![SessionId::from("one")]);
}

#[tokio::test]
async fn rejects_mismatched_missing_unresolved_non_directory_and_unknown_cwd_facts() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("home");
    let other = temp.path().join("other");
    let file = temp.path().join("file");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&other).unwrap();
    std::fs::write(&file, "x").unwrap();
    let missing = temp.path().join("missing");
    let h = Harness::new(vec![
        header("mismatch", Some(other.to_str().unwrap()), 1),
        header("none", None, 2),
        header("missing", Some(missing.to_str().unwrap()), 3),
        header("file", Some(file.to_str().unwrap()), 4),
    ])
    .await;
    let w = h
        .registry
        .create(home.to_str().unwrap(), None)
        .await
        .unwrap();
    let cases = [
        ("mismatch", "resolves to"),
        ("none", "no cwd"),
        ("missing", "does not resolve"),
        ("file", "not a directory"),
        ("ghost", "no such session"),
    ];
    for (id, expected) in cases {
        let error = w.attach_session(id.into()).await.unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "{id}: expected {expected:?}, got {error}"
        );
    }
}

#[tokio::test]
async fn keeps_created_at_stable_advances_updated_at_and_reports_directory_disappearance() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("gone");
    std::fs::create_dir(&path).unwrap();
    let h = Harness::new(vec![]).await;
    let w = h
        .registry
        .create(path.to_str().unwrap(), None)
        .await
        .unwrap();
    let created = w.created_at();
    w.set_title("kept").await.unwrap();
    assert_eq!(w.created_at(), created);
    assert!(w.updated_at() >= created);
    assert_eq!(w.status().await, WorkspaceStatus::Ok);
    std::fs::remove_dir(&path).unwrap();
    assert_eq!(w.status().await, WorkspaceStatus::MissingDir);
}

#[tokio::test]
async fn archives_unarchives_pins_and_unpins_durably_and_idempotently() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().to_string_lossy().into_owned();
    let h = Harness::new(vec![
        header("one", Some(&path), 1),
        header("two", Some(&path), 2),
    ])
    .await;
    h.registry.pin_session("one".into()).await.unwrap();
    h.registry.pin_session("two".into()).await.unwrap();
    assert_eq!(
        h.registry.pinned_session_ids(),
        vec![SessionId::from("two"), SessionId::from("one")]
    );
    h.registry
        .archive_session("one".into(), ArchiveSessionOptions::default())
        .await
        .unwrap();
    assert_eq!(
        h.registry.pinned_session_ids(),
        vec![SessionId::from("two")]
    );
    assert_eq!(
        h.registry.archived_session_ids(),
        vec![SessionId::from("one")]
    );
    assert!(
        h.registry
            .pin_session("one".into())
            .await
            .unwrap_err()
            .is::<WorkspaceArchivedSessionPinError>()
    );
    h.registry.unarchive_session(&"one".into()).await.unwrap();
    h.registry.unpin_session(&"two".into()).await.unwrap();
    assert!(h.registry.archived_session_ids().is_empty());
    assert!(h.registry.pinned_session_ids().is_empty());
}

#[tokio::test]
async fn propagates_a_persistence_listing_failure_instead_of_reporting_unknown() {
    let h = Harness::new(vec![]).await;
    *h.persistence.fail.lock().unwrap() = Some("persistence backend down".into());
    for verb in ["archive", "pin"] {
        let error = if verb == "archive" {
            h.registry
                .archive_session("ghost".into(), ArchiveSessionOptions::default())
                .await
                .unwrap_err()
        } else {
            h.registry.pin_session("ghost".into()).await.unwrap_err()
        };
        assert_eq!(error.to_string(), "persistence backend down");
        *h.persistence.fail.lock().unwrap() = Some("persistence backend down".into());
    }
    assert!(h.registry.archived_session_ids().is_empty());
    assert!(h.registry.pinned_session_ids().is_empty());
}

#[tokio::test]
async fn rejects_unknown_archive_and_pin_without_writing() {
    let h = Harness::new(vec![]).await;
    assert!(
        h.registry
            .archive_session("ghost".into(), ArchiveSessionOptions::default())
            .await
            .unwrap_err()
            .is::<WorkspaceUnknownSessionError>()
    );
    assert!(
        h.registry
            .pin_session("ghost".into())
            .await
            .unwrap_err()
            .is::<WorkspaceUnknownSessionError>()
    );
    assert!(h.registry.archived_session_ids().is_empty());
    assert!(h.registry.pinned_session_ids().is_empty());
}

#[tokio::test]
async fn creates_parent_directories_only_for_eligible_default_initialization() {
    let temp = TempDir::new().unwrap();
    let nested = temp.path().join("nested/workspace");
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let storage = Arc::new(StorageService::new());
    let medium = Arc::new(Medium::default());
    let _r = storage
        .backend
        .register("memory", Arc::new(Backend(medium)))
        .unwrap();
    ctx.provide::<Storage>(storage).await.unwrap();
    let facility = Arc::new(DomainFacility::new(
        ctx.clone(),
        DomainConfig {
            backend: "memory".into(),
            routes: IndexMap::new(),
        },
    ));
    let persistence = Arc::new(Persistence::new(vec![]));
    ctx.provide::<StorageDomain>(facility).await.unwrap();
    ctx.provide::<SessionPersistenceService>(persistence)
        .await
        .unwrap();
    ctx.provide::<hoh_session::Sessions>(Arc::new(hoh_session::SessionStore::new()))
        .await
        .unwrap();
    let facility_service = ctx.get::<StorageDomain>().unwrap();
    let persistence_service = ctx.get::<SessionPersistenceService>().unwrap();
    let registry = WorkspaceRegistry::start(ctx, facility_service, persistence_service)
        .await
        .unwrap();
    let requested = nested.to_string_lossy().into_owned();
    let workspace = registry
        .initialize_default(move || async move { Ok(requested) })
        .await
        .unwrap()
        .unwrap();
    assert!(nested.is_dir());
    assert_eq!(workspace.title(), "workspace");
}

#[test]
fn stored_schemas_strip_unknown_fields_and_default_old_session_sets() {
    let record=workspace_record_schema().parse(&serde_json::json!({"path":"/x","title":"x","sessionIds":["s"],"createdAt":"a","updatedAt":"b","extra":1})).unwrap();
    assert_eq!(record.session_ids, vec![SessionId::from("s")]);
    let state = workspace_domain_state_schema()
        .parse(&serde_json::json!({"initialized":true,"workspaceIds":[],"extra":1}))
        .unwrap();
    assert!(state.archived_session_ids.is_empty());
    assert!(state.pinned_session_ids.is_empty());
}

#[tokio::test]
async fn refuses_a_session_the_activity_waterfall_reports_active_and_writes_nothing() {
    let h = Harness::new(vec![header("busy", None, 1)]).await;
    let _listener = h
        .ctx
        .on_waterfall::<WorkspaceSessionActivity, Vec<SessionActivity>, _>(|_, next| async move {
            let rest = next.run().await?;
            Ok(std::iter::once(SessionActivity {
                kind: "probe".into(),
                items: None,
            })
            .chain(rest)
            .collect())
        })
        .unwrap();
    let error = h
        .registry
        .archive_session("busy".into(), ArchiveSessionOptions::default())
        .await
        .unwrap_err();
    let error = error.downcast_ref::<WorkspaceActiveSessionError>().unwrap();
    assert_eq!(error.session_id, "busy");
    assert_eq!(error.activity[0].kind, "probe");
    assert!(h.registry.archived_session_ids().is_empty());
}

#[tokio::test]
async fn with_stop_activity_writes_the_archive_and_then_asks_providers_to_stop() {
    let h = Harness::new(vec![header("busy", None, 1)]).await;
    let order = Arc::new(Mutex::new(Vec::new()));
    let seen = order.clone();
    let _stop = h
        .ctx
        .on_parallel::<WorkspaceSessionStop, _>(move |event| {
            let seen = seen.clone();
            async move {
                seen.lock()
                    .unwrap()
                    .push(format!("stop:{}", event.session_id));
                Ok(())
            }
        })
        .unwrap();
    h.registry
        .archive_session(
            "busy".into(),
            ArchiveSessionOptions {
                stop_activity: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        h.registry.archived_session_ids(),
        vec![SessionId::from("busy")]
    );
    assert_eq!(*order.lock().unwrap(), vec!["stop:busy"]);
}

#[tokio::test]
async fn dispatches_only_after_durability_and_waits_before_unarchiving() {
    let h = Harness::new(vec![header("cleanup", None, 1)]).await;
    let registry = h.registry.clone();
    let _archived = h
        .ctx
        .on_parallel::<WorkspaceSessionArchived, _>(move |event| {
            let registry = registry.clone();
            async move {
                assert!(registry.archived_session_ids().contains(&event.session_id));
                Ok(())
            }
        })
        .unwrap();
    h.registry
        .archive_session("cleanup".into(), ArchiveSessionOptions::default())
        .await
        .unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let entered_l = entered.clone();
    let release_l = release.clone();
    let _unarchiving = h
        .ctx
        .on_parallel::<WorkspaceSessionUnarchiving, _>(move |_| {
            let entered = entered_l.clone();
            let release = release_l.clone();
            async move {
                entered.notify_one();
                release.notified().await;
                Ok(())
            }
        })
        .unwrap();
    let r = h.registry.clone();
    let task = tokio::spawn(async move { r.unarchive_session(&"cleanup".into()).await });
    entered.notified().await;
    assert_eq!(
        h.registry.archived_session_ids(),
        vec![SessionId::from("cleanup")]
    );
    release.notify_one();
    task.await.unwrap().unwrap();
    assert!(h.registry.archived_session_ids().is_empty());
}

#[tokio::test]
async fn retains_successful_archival_when_follow_up_admission_fails() {
    let h = Harness::new(vec![header("cleanup-error", None, 1)]).await;
    let _listener = h
        .ctx
        .on_parallel::<WorkspaceSessionArchived, _>(|_| async {
            Err::<(), _>("inspection unavailable".into())
        })
        .unwrap();
    h.registry
        .archive_session("cleanup-error".into(), ArchiveSessionOptions::default())
        .await
        .unwrap();
    assert_eq!(
        h.registry.archived_session_ids(),
        vec![SessionId::from("cleanup-error")]
    );
}

#[tokio::test]
async fn rejects_moves_naming_an_unaccounted_session_or_anchor() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().to_string_lossy().into_owned();
    let h = Harness::new(vec![]).await;
    let w = h.registry.create(&path, None).await.unwrap();
    let error = w
        .insert_session_before("ghost".into(), None)
        .await
        .unwrap_err();
    assert!(error.is::<WorkspaceMoveInvalidError>());
    *h.persistence.headers.lock().unwrap() = vec![header("one", Some(&path), 1)];
    w.attach_session("one".into()).await.unwrap();
    let error = w
        .insert_session_before("one".into(), Some("ghost".into()))
        .await
        .unwrap_err();
    assert!(error.is::<WorkspaceMoveInvalidError>());
}

#[tokio::test]
async fn preserves_owned_sessions_and_manual_order_after_directory_removal() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("owned");
    std::fs::create_dir(&path).unwrap();
    let path = path.to_string_lossy().into_owned();
    let h = Harness::new(vec![
        header("one", Some(&path), 1),
        header("two", Some(&path), 2),
    ])
    .await;
    let w = h.registry.list().unwrap()[0].clone();
    std::fs::remove_dir(&path).unwrap();
    assert_eq!(
        w.session_ids(),
        vec![SessionId::from("two"), SessionId::from("one")]
    );
    w.insert_session_before("one".into(), Some("two".into()))
        .await
        .unwrap();
    assert_eq!(
        w.session_ids(),
        vec![SessionId::from("one"), SessionId::from("two")]
    );
}

#[tokio::test]
async fn unarchives_and_unpins_entries_whose_sessions_are_gone_without_listing() {
    let h = Harness::new(vec![header("gone", None, 1)]).await;
    h.registry.pin_session("gone".into()).await.unwrap();
    h.registry.unpin_session(&"gone".into()).await.unwrap();
    h.registry
        .archive_session("gone".into(), ArchiveSessionOptions::default())
        .await
        .unwrap();
    *h.persistence.fail.lock().unwrap() = Some("must not list".into());
    h.registry.unarchive_session(&"gone".into()).await.unwrap();
    assert!(h.registry.archived_session_ids().is_empty());
    assert_eq!(
        h.persistence.fail.lock().unwrap().as_deref(),
        Some("must not list")
    );
}

#[tokio::test]
async fn does_not_infer_an_empty_live_session_store_when_the_peer_is_unavailable() {
    let h = Harness::new(vec![]).await;
    let error = h
        .registry
        .initialize_default(|| async { Err::<String, BoxError>("resolver failed".into()) })
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "default Workspace initialization requires the Session store"
    );
}

#[test]
fn workspace_error_messages_match_the_source_vocabulary() {
    assert_eq!(
        WorkspaceUnknownSessionError {
            session_id: "s".into(),
            verb: "archive"
        }
        .to_string(),
        "cannot archive session 's': live sessions and session persistence hold no such session"
    );
    assert_eq!(
        WorkspaceArchivedSessionPinError {
            session_id: "s".into()
        }
        .to_string(),
        "cannot pin session 's': the session is archived"
    );
    assert_eq!(
        WorkspaceOrderInvalidError {
            workspace_id: "w".into()
        }
        .to_string(),
        "cannot reorder unknown workspace 'w'"
    );
}

#[tokio::test]
async fn rolls_back_the_provisional_cache_when_the_record_write_fails() {
    let medium = Arc::new(Medium::default());
    let h = Harness::with_medium(vec![], medium.clone()).await;
    medium.fail_next_put();
    let temp = TempDir::new().unwrap();
    let error = h
        .registry
        .create(temp.path().to_str().unwrap(), None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("put failure"));
    assert!(h.registry.list().unwrap().is_empty());
    assert!(medium.snapshot.lock().unwrap().tables["workspaces"].is_empty());
}

#[tokio::test]
async fn does_not_publish_a_workspace_when_its_pending_marker_cannot_be_written() {
    let medium = Arc::new(Medium::default());
    let h = Harness::with_medium(vec![], medium.clone()).await;
    medium.fail_next_global();
    let temp = TempDir::new().unwrap();
    assert!(
        h.registry
            .create(temp.path().to_str().unwrap(), None)
            .await
            .unwrap_err()
            .to_string()
            .contains("marker failure")
    );
    assert!(h.registry.list().unwrap().is_empty());
    assert!(medium.snapshot.lock().unwrap().tables["workspaces"].is_empty());
}

#[tokio::test]
async fn rolls_back_a_record_when_registry_order_persistence_fails() {
    let medium = Arc::new(Medium::default());
    let h = Harness::with_medium(vec![], medium.clone()).await;
    medium.fail_global_offset(2);
    let temp = TempDir::new().unwrap();
    assert!(
        h.registry
            .create(temp.path().to_str().unwrap(), None)
            .await
            .unwrap_err()
            .to_string()
            .contains("marker failure")
    );
    assert!(h.registry.list().unwrap().is_empty());
    assert!(medium.snapshot.lock().unwrap().tables["workspaces"].is_empty());
}

#[tokio::test]
async fn reports_a_record_write_and_pending_marker_rollback_failure_together() {
    let medium = Arc::new(Medium::default());
    let h = Harness::with_medium(vec![], medium.clone()).await;
    medium.fail_next_put();
    medium.fail_global_offset(2);
    let temp = TempDir::new().unwrap();
    let error = h
        .registry
        .create(temp.path().to_str().unwrap(), None)
        .await
        .unwrap_err();
    let aggregate = error.downcast_ref::<WorkspaceAggregateError>().unwrap();
    assert!(
        aggregate
            .message
            .contains("record write and pending-marker rollback both failed")
    );
    assert_eq!(aggregate.errors.len(), 2);
    assert!(aggregate.errors[0].to_string().contains("put failure"));
    assert!(aggregate.errors[1].to_string().contains("marker failure"));
}

#[tokio::test]
async fn reports_both_order_and_rollback_failures_while_retaining_the_recoverable_record() {
    let medium = Arc::new(Medium::default());
    let h = Harness::with_medium(vec![], medium.clone()).await;
    medium.fail_global_offset(2);
    medium.fail_next_delete();
    let temp = TempDir::new().unwrap();
    let error = h
        .registry
        .create(temp.path().to_str().unwrap(), None)
        .await
        .unwrap_err();
    let aggregate = error.downcast_ref::<WorkspaceAggregateError>().unwrap();
    assert!(
        aggregate
            .message
            .contains("order write and record rollback both failed")
    );
    assert_eq!(aggregate.errors.len(), 2);
    assert_eq!(
        medium.snapshot.lock().unwrap().tables["workspaces"].len(),
        1
    );
    assert!(h.registry.list().unwrap().is_empty());
}

#[tokio::test]
async fn keeps_created_at_stable_advances_updated_at_and_preserves_snapshot_on_write_failure() {
    let medium = Arc::new(Medium::default());
    let h = Harness::with_medium(vec![], medium.clone()).await;
    let temp = TempDir::new().unwrap();
    let w = h
        .registry
        .create(temp.path().to_str().unwrap(), None)
        .await
        .unwrap();
    w.set_title("kept").await.unwrap();
    let created = w.created_at();
    medium.fail_next_put();
    assert!(w.set_title("lost").await.is_err());
    assert_eq!(w.title(), "kept");
    assert_eq!(w.created_at(), created);
}

#[tokio::test]
async fn rolls_registry_order_and_cache_back_when_record_deletion_fails() {
    let medium = Arc::new(Medium::default());
    let h = Harness::with_medium(vec![], medium.clone()).await;
    let temp = TempDir::new().unwrap();
    let w = h
        .registry
        .create(temp.path().to_str().unwrap(), None)
        .await
        .unwrap();
    medium.fail_next_delete();
    assert!(h.registry.delete(w.id()).await.is_err());
    assert!(h.registry.get(w.id()).is_some());
    assert_eq!(h.registry.list().unwrap().len(), 1);
}

#[tokio::test]
async fn resolve_by_path_uses_the_same_canonical_identity_without_mutation() {
    let h = Harness::new(vec![]).await;
    let temp = TempDir::new().unwrap();
    let w = h
        .registry
        .create(temp.path().to_str().unwrap(), None)
        .await
        .unwrap();
    assert_eq!(
        h.registry
            .resolve_by_path(temp.path().to_str().unwrap())
            .await
            .unwrap()
            .unwrap()
            .id(),
        w.id()
    );
    let other = TempDir::new().unwrap();
    assert!(
        h.registry
            .resolve_by_path(other.path().to_str().unwrap())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn allows_a_duplicate_display_name_on_a_different_canonical_path() {
    let root = TempDir::new().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let h = Harness::new(vec![]).await;
    let a = h
        .registry
        .create(a.to_str().unwrap(), Some("same".into()))
        .await
        .unwrap();
    let b = h
        .registry
        .create(b.to_str().unwrap(), Some("same".into()))
        .await
        .unwrap();
    assert_ne!(a.id(), b.id());
    assert_eq!(a.title(), b.title());
}

#[tokio::test]
async fn rejects_a_resolvable_relative_path_instead_of_adopting_it_from_the_host_cwd() {
    let h = Harness::new(vec![]).await;
    let error = h.registry.create(".", None).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Workspace path is not fully qualified: '.'"
    );
    assert!(h.registry.list().unwrap().is_empty());
}

#[tokio::test]
async fn keeps_self_anchored_and_already_positioned_moves_write_free_and_rejects_unknown_ids() {
    let root = TempDir::new().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let h = Harness::new(vec![]).await;
    let a = h.registry.create(a.to_str().unwrap(), None).await.unwrap();
    let b = h.registry.create(b.to_str().unwrap(), None).await.unwrap();
    let before = h
        .registry
        .list()
        .unwrap()
        .into_iter()
        .map(|w| w.id().clone())
        .collect::<Vec<_>>();
    assert_eq!(
        h.registry
            .insert_before(b.id(), Some(b.id()))
            .await
            .unwrap(),
        before
    );
    assert!(
        h.registry
            .insert_before(&"ghost".into(), Some(a.id()))
            .await
            .unwrap_err()
            .is::<WorkspaceOrderInvalidError>()
    );
}

#[tokio::test]
async fn treats_self_anchored_and_already_in_place_session_moves_as_no_ops_without_writing() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().to_string_lossy().into_owned();
    let h = Harness::new(vec![]).await;
    let w = h.registry.create(&path, None).await.unwrap();
    *h.persistence.headers.lock().unwrap() =
        vec![header("a", Some(&path), 1), header("b", Some(&path), 2)];
    w.attach_session("a".into()).await.unwrap();
    w.attach_session("b".into()).await.unwrap();
    let updated = w.updated_at();
    w.insert_session_before("b".into(), Some("b".into()))
        .await
        .unwrap();
    assert_eq!(w.updated_at(), updated);
    w.insert_session_before("b".into(), Some("a".into()))
        .await
        .unwrap();
    assert_eq!(w.updated_at(), updated);
}

#[tokio::test]
async fn reports_directory_disappearance_without_mutating_the_workspace() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("gone");
    std::fs::create_dir(&path).unwrap();
    let h = Harness::new(vec![]).await;
    let w = h
        .registry
        .create(path.to_str().unwrap(), None)
        .await
        .unwrap();
    let updated = w.updated_at();
    std::fs::remove_dir(&path).unwrap();
    assert_eq!(w.status().await, WorkspaceStatus::MissingDir);
    assert_eq!(w.updated_at(), updated);
    std::fs::write(&path, "file").unwrap();
    assert_eq!(w.status().await, WorkspaceStatus::MissingDir);
}

#[tokio::test]
async fn archives_durably_in_order_idempotently_skips_repeats_and_leaves_accounting_untouched() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().to_string_lossy().into_owned();
    let h = Harness::new(vec![
        header("one", Some(&path), 1),
        header("two", Some(&path), 2),
    ])
    .await;
    let w = h.registry.list().unwrap()[0].clone();
    h.registry
        .archive_session("one".into(), ArchiveSessionOptions::default())
        .await
        .unwrap();
    h.registry
        .archive_session("one".into(), ArchiveSessionOptions::default())
        .await
        .unwrap();
    h.registry
        .archive_session("two".into(), ArchiveSessionOptions::default())
        .await
        .unwrap();
    assert_eq!(
        h.registry.archived_session_ids(),
        vec![SessionId::from("one"), SessionId::from("two")]
    );
    assert_eq!(
        w.session_ids(),
        vec![SessionId::from("two"), SessionId::from("one")]
    );
}

#[tokio::test]
async fn rejects_pinning_archived_and_unknown_sessions_without_writing() {
    let h = Harness::new(vec![header("archived", None, 1)]).await;
    h.registry
        .archive_session("archived".into(), ArchiveSessionOptions::default())
        .await
        .unwrap();
    assert!(
        h.registry
            .pin_session("archived".into())
            .await
            .unwrap_err()
            .is::<WorkspaceArchivedSessionPinError>()
    );
    assert!(
        h.registry
            .pin_session("ghost".into())
            .await
            .unwrap_err()
            .is::<WorkspaceUnknownSessionError>()
    );
    assert!(h.registry.pinned_session_ids().is_empty());
}

#[tokio::test]
async fn archiving_drops_the_pin_in_the_same_durable_write() {
    let h = Harness::new(vec![header("session", None, 1)]).await;
    h.registry.pin_session("session".into()).await.unwrap();
    h.registry
        .archive_session("session".into(), ArchiveSessionOptions::default())
        .await
        .unwrap();
    assert_eq!(
        h.registry.archived_session_ids(),
        vec![SessionId::from("session")]
    );
    assert!(h.registry.pinned_session_ids().is_empty());
    let saved = medium_state(&h);
    assert_eq!(saved["archivedSessionIds"], serde_json::json!(["session"]));
    assert_eq!(saved["pinnedSessionIds"], serde_json::json!([]));
}

#[tokio::test]
async fn closes_its_domain_on_disposal_and_reloads_the_persisted_stable_order() {
    let medium = Arc::new(Medium::default());
    let root = TempDir::new().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let first = Harness::with_medium(vec![], medium.clone()).await;
    let wa = first
        .registry
        .create(a.to_str().unwrap(), None)
        .await
        .unwrap();
    let wb = first
        .registry
        .create(b.to_str().unwrap(), None)
        .await
        .unwrap();
    first
        .registry
        .insert_before(wa.id(), Some(wb.id()))
        .await
        .unwrap();
    first.registry.domain().close().await.unwrap();
    drop(first);
    let second = Harness::with_medium(vec![], medium).await;
    assert_eq!(
        second
            .registry
            .list()
            .unwrap()
            .iter()
            .map(|w| w.id())
            .collect::<Vec<_>>(),
        vec![wa.id(), wb.id()]
    );
}

#[tokio::test]
async fn does_not_rerun_bootstrap_for_a_genuinely_initialized_empty_registry() {
    let medium = Arc::new(Medium::default());
    let first = Harness::with_medium(vec![], medium.clone()).await;
    first.registry.domain().close().await.unwrap();
    drop(first);
    let temp = TempDir::new().unwrap();
    let second = Harness::with_medium(
        vec![header("late", Some(temp.path().to_str().unwrap()), 1)],
        medium,
    )
    .await;
    assert!(second.registry.list().unwrap().is_empty());
}

#[tokio::test]
async fn restores_archive_and_pin_sets_across_restarts() {
    let medium = Arc::new(Medium::default());
    let headers = vec![header("archived", None, 1), header("pinned", None, 2)];
    let first = Harness::with_medium(headers.clone(), medium.clone()).await;
    first.registry.pin_session("pinned".into()).await.unwrap();
    first
        .registry
        .archive_session("archived".into(), ArchiveSessionOptions::default())
        .await
        .unwrap();
    first.registry.domain().close().await.unwrap();
    drop(first);
    let second = Harness::with_medium(headers, medium).await;
    assert_eq!(
        second.registry.archived_session_ids(),
        vec![SessionId::from("archived")]
    );
    assert_eq!(
        second.registry.pinned_session_ids(),
        vec![SessionId::from("pinned")]
    );
}

#[tokio::test]
async fn reuses_partial_records_after_a_bootstrap_record_write_fails() {
    let medium = Arc::new(Medium::default());
    medium.fail_put_offset(2);
    let first = TempDir::new().unwrap();
    let second = TempDir::new().unwrap();
    let headers = vec![
        header("newer", Some(first.path().to_str().unwrap()), 2),
        header("older", Some(second.path().to_str().unwrap()), 1),
    ];
    assert!(
        Harness::try_with_medium(headers.clone(), medium.clone())
            .await
            .is_err()
    );
    let partial_id = medium.snapshot.lock().unwrap().tables["workspaces"]
        .keys()
        .next()
        .unwrap()
        .clone();
    let retried = Harness::with_medium(headers, medium.clone()).await;
    assert_eq!(retried.registry.list().unwrap().len(), 2);
    assert!(retried.registry.get(&partial_id.into()).is_some());
    assert_eq!(
        medium.snapshot.lock().unwrap().tables["workspaces"].len(),
        2
    );
}

#[tokio::test]
async fn rejects_duplicate_candidate_ownership_duplicate_paths_and_initialized_order_drift() {
    async fn rejected(entries: serde_json::Value, state: serde_json::Value) -> String {
        let medium = Arc::new(Medium::default());
        {
            let mut snapshot = medium.snapshot.lock().unwrap();
            snapshot.tables.insert(
                "workspaces".into(),
                entries
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            );
            snapshot.global = state;
        }
        Harness::try_with_medium(vec![], medium)
            .await
            .err()
            .unwrap()
            .to_string()
    }
    let r = |path: &str, sessions: Vec<&str>| serde_json::json!({"path":path,"title":"x","sessionIds":sessions,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:00.000Z"});
    let error = rejected(
        serde_json::json!({"a":r("/same",vec!["s"]),"b":r("/same",vec![])}),
        serde_json::json!({"initialized":true,"workspaceIds":["a","b"]}),
    )
    .await;
    assert!(error.contains("path '/same' is claimed"));
    let error = rejected(
        serde_json::json!({"a":r("/a",vec!["s"]),"b":r("/b",vec!["s"])}),
        serde_json::json!({"initialized":true,"workspaceIds":["a","b"]}),
    )
    .await;
    assert!(error.contains("session 's' is accounted"));
    let error = rejected(
        serde_json::json!({"a":r("/a",vec![])}),
        serde_json::json!({"initialized":true,"workspaceIds":[]}),
    )
    .await;
    assert!(error.contains("absent from registry order"));
}

#[tokio::test]
async fn creates_parent_directories_and_resolves_only_one_directory_for_concurrent_initialization()
{
    let medium = Arc::new(Medium::default());
    let h = Harness::with_session_store(vec![], medium).await;
    let root = TempDir::new().unwrap();
    let requested = root.path().join("nested/Workspace");
    let calls = Arc::new(AtomicU64::new(0));
    let first = {
        let requested = requested.clone();
        let calls = calls.clone();
        h.registry.initialize_default(move || async move {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(requested.to_string_lossy().into_owned())
        })
    };
    let second = {
        let calls = calls.clone();
        h.registry.initialize_default(move || async move {
            calls.fetch_add(100, Ordering::SeqCst);
            Err("unexpected directory lookup".into())
        })
    };
    let (first, repeated) = tokio::join!(first, second);
    let first = first.unwrap().unwrap();
    let repeated = repeated.unwrap().unwrap();
    assert_eq!(first.id(), repeated.id());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(first.title(), "Workspace");
    assert!(requested.is_dir());
    assert_eq!(medium_state(&h)["defaultWorkspaceId"], first.id().as_str());
}

#[tokio::test]
async fn reuses_an_existing_directory_without_changing_its_contents() {
    let h = Harness::with_session_store(vec![], Arc::new(Medium::default())).await;
    let root = TempDir::new().unwrap();
    let requested = root.path().join("nested/Workspace");
    std::fs::create_dir_all(&requested).unwrap();
    let keep = requested.join("keep.txt");
    std::fs::write(&keep, "keep").unwrap();
    let path = requested.to_string_lossy().into_owned();
    assert_eq!(
        h.registry
            .initialize_default(|| async { Ok(path) })
            .await
            .unwrap()
            .unwrap()
            .path(),
        requested.to_string_lossy()
    );
    assert_eq!(std::fs::read_to_string(keep).unwrap(), "keep");
}

#[tokio::test]
async fn leaves_initialization_retryable_when_directory_resolution_fails() {
    let h = Harness::with_session_store(vec![], Arc::new(Medium::default())).await;
    assert_eq!(
        h.registry
            .initialize_default(|| async { Err::<String, BoxError>("lookup unavailable".into()) })
            .await
            .unwrap_err()
            .to_string(),
        "lookup unavailable"
    );
    let root = TempDir::new().unwrap();
    let path = root.path().join("Workspace").to_string_lossy().into_owned();
    assert!(
        h.registry
            .initialize_default(|| async { Ok(path) })
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn rejects_a_relative_candidate_before_creating_its_directory() {
    let h = Harness::with_session_store(vec![], Arc::new(Medium::default())).await;
    let error = h
        .registry
        .initialize_default(|| async { Ok("relative/Workspace".into()) })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("fully qualified"));
    assert!(h.registry.list().unwrap().is_empty());
    assert!(medium_state(&h).get("defaultWorkspaceId").is_none());
}

#[tokio::test]
async fn keeps_the_initialization_marker_across_deletion_and_restart() {
    let medium = Arc::new(Medium::default());
    let root = TempDir::new().unwrap();
    let requested = root.path().join("Workspace");
    let first = Harness::with_session_store(vec![], medium.clone()).await;
    let path = requested.to_string_lossy().into_owned();
    let workspace = first
        .registry
        .initialize_default(|| async { Ok(path) })
        .await
        .unwrap()
        .unwrap();
    workspace.set_title("Renamed").await.unwrap();
    assert_eq!(
        first
            .registry
            .initialize_default(|| async { Err("unused".into()) })
            .await
            .unwrap()
            .unwrap()
            .title(),
        "Renamed"
    );
    first.registry.delete(workspace.id()).await.unwrap();
    first.registry.domain().close().await.unwrap();
    drop(first);
    let second = Harness::with_session_store(vec![], medium).await;
    let called = Arc::new(AtomicU64::new(0));
    let seen = called.clone();
    assert!(
        second
            .registry
            .initialize_default(move || async move {
                seen.fetch_add(1, Ordering::SeqCst);
                Ok("/unused".into())
            })
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(called.load(Ordering::SeqCst), 0);
    assert_eq!(
        medium_state(&second)["defaultWorkspaceId"],
        workspace.id().as_str()
    );
}

#[tokio::test]
async fn refuses_a_non_empty_workspace_list_and_newly_persisted_history() {
    let h = Harness::with_session_store(vec![], Arc::new(Medium::default())).await;
    let root = TempDir::new().unwrap();
    let explicit = h
        .registry
        .create(root.path().to_str().unwrap(), None)
        .await
        .unwrap();
    assert!(
        h.registry
            .initialize_default(|| async { Err("must not resolve".into()) })
            .await
            .unwrap()
            .is_none()
    );
    h.registry.delete(explicit.id()).await.unwrap();
    *h.persistence.headers.lock().unwrap() = vec![header("arrived", None, 1)];
    assert!(
        h.registry
            .initialize_default(|| async { Err("must not resolve".into()) })
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn fails_on_a_same_path_file_and_remains_eligible_after_it_is_removed() {
    let h = Harness::with_session_store(vec![], Arc::new(Medium::default())).await;
    let root = TempDir::new().unwrap();
    let requested = root.path().join("nested/Workspace");
    std::fs::create_dir(requested.parent().unwrap()).unwrap();
    std::fs::write(&requested, "occupied").unwrap();
    let path = requested.to_string_lossy().into_owned();
    let first_path = path.clone();
    assert!(
        h.registry
            .initialize_default(move || async move { Ok(first_path) })
            .await
            .is_err()
    );
    assert!(h.registry.list().unwrap().is_empty());
    std::fs::remove_file(&requested).unwrap();
    assert!(
        h.registry
            .initialize_default(move || async move { Ok(path) })
            .await
            .unwrap()
            .is_some()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn titles_the_workspace_after_the_requested_directory_when_that_path_is_a_symlink() {
    use std::os::unix::fs::symlink;
    let h = Harness::with_session_store(vec![], Arc::new(Medium::default())).await;
    let root = TempDir::new().unwrap();
    let target = root.path().join("elsewhere");
    let parent = root.path().join("nested");
    std::fs::create_dir(&target).unwrap();
    std::fs::create_dir(&parent).unwrap();
    let requested = parent.join("Workspace");
    symlink(&target, &requested).unwrap();
    let path = requested.to_string_lossy().into_owned();
    let workspace = h
        .registry
        .initialize_default(|| async { Ok(path) })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        workspace.path(),
        std::fs::canonicalize(target).unwrap().to_string_lossy()
    );
    assert_eq!(workspace.title(), "Workspace");
}

#[tokio::test]
async fn refuses_registration_when_a_persisted_session_appears_during_directory_preparation() {
    let h = Harness::with_session_store(vec![], Arc::new(Medium::default())).await;
    let root = TempDir::new().unwrap();
    let requested = root.path().join("Workspace");
    let persistence = h.persistence.clone();
    let path = requested.to_string_lossy().into_owned();
    let result = h
        .registry
        .initialize_default(move || async move {
            *persistence.headers.lock().unwrap() = vec![header("arrived", None, 1)];
            Ok(path)
        })
        .await
        .unwrap();
    assert!(result.is_none());
    assert!(h.registry.list().unwrap().is_empty());
}

#[tokio::test]
async fn rolls_back_a_failed_final_default_marker_write_and_allows_a_retry() {
    let medium = Arc::new(Medium::default());
    let h = Harness::with_session_store(vec![], medium.clone()).await;
    let root = TempDir::new().unwrap();
    let requested = root.path().join("Workspace");
    let path = requested.to_string_lossy().into_owned();
    let first_path = path.clone();
    medium.fail_global_offset(2);
    assert!(
        h.registry
            .initialize_default(move || async move { Ok(first_path) })
            .await
            .is_err()
    );
    assert!(h.registry.list().unwrap().is_empty());
    assert!(requested.is_dir());
    assert!(
        h.registry
            .initialize_default(move || async move { Ok(path) })
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn reports_an_order_write_and_pending_marker_rollback_failure_as_an_aggregate() {
    let medium = Arc::new(Medium::default());
    let h = Harness::with_medium(vec![], medium.clone()).await;
    medium.fail_global_offset(2);
    medium.fail_global_offset(3);
    let temp = TempDir::new().unwrap();
    let error = h
        .registry
        .create(temp.path().to_str().unwrap(), None)
        .await
        .unwrap_err();
    let aggregate = error.downcast_ref::<WorkspaceAggregateError>().unwrap();
    assert_eq!(aggregate.errors.len(), 2);
    assert!(
        aggregate
            .message
            .contains("order write and pending-marker rollback both failed")
    );
    assert!(medium_state(&h).get("pendingMutation").is_some());
}

#[tokio::test]
async fn keeps_a_double_failed_deletion_unpublished_and_preserves_both_errors() {
    let medium = Arc::new(Medium::default());
    let h = Harness::with_medium(vec![], medium.clone()).await;
    let temp = TempDir::new().unwrap();
    let workspace = h
        .registry
        .create(temp.path().to_str().unwrap(), None)
        .await
        .unwrap();
    medium.fail_next_delete();
    medium.fail_global_offset(2);
    let error = h.registry.delete(workspace.id()).await.unwrap_err();
    let aggregate = error.downcast_ref::<WorkspaceAggregateError>().unwrap();
    assert_eq!(aggregate.errors.len(), 2);
    assert!(
        aggregate
            .message
            .contains("record deletion and registry-order rollback both failed")
    );
    assert!(h.registry.get(workspace.id()).is_none());
    assert!(h.registry.list().unwrap().is_empty());
    assert_eq!(medium_state(&h)["pendingMutation"]["operation"], "delete");
}

#[tokio::test]
async fn finishes_a_registry_create_after_its_calling_future_is_cancelled() {
    let medium = Arc::new(Medium::default());
    let h = Harness::with_medium(vec![], medium.clone()).await;
    let gate = medium.gate_next_put();
    let temp = TempDir::new().unwrap();
    let path = temp.path().to_string_lossy().into_owned();
    let registry = h.registry.clone();
    let task = tokio::spawn(async move { registry.create(&path, None).await });
    gate.entered.notified().await;
    task.abort();
    gate.release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while h.registry.list().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(h.registry.list().unwrap().len(), 1);
    assert_eq!(
        medium.snapshot.lock().unwrap().tables["workspaces"].len(),
        1
    );
}

#[tokio::test]
async fn publishes_an_entity_mutation_after_its_calling_future_is_cancelled() {
    let medium = Arc::new(Medium::default());
    let h = Harness::with_medium(vec![], medium.clone()).await;
    let temp = TempDir::new().unwrap();
    let workspace = h
        .registry
        .create(temp.path().to_str().unwrap(), None)
        .await
        .unwrap();
    let gate = medium.gate_next_put();
    let mutating = workspace.clone();
    let task = tokio::spawn(async move { mutating.set_title("finished").await });
    gate.entered.notified().await;
    task.abort();
    gate.release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while workspace.title() != "finished" {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(workspace.title(), "finished");
}

#[tokio::test]
async fn accepts_an_integral_float_created_at_from_a_valid_live_session_header() {
    let h = Harness::with_session_store(vec![], Arc::new(Medium::default())).await;
    let temp = TempDir::new().unwrap();
    let path = temp.path().to_string_lossy().into_owned();
    let workspace = h.registry.create(&path, None).await.unwrap();
    let sessions = h.sessions.as_ref().unwrap();
    sessions
        .create(
            &h.ctx,
            Some("live-float".into()),
            hoh_session::CreateSessionOptions {
                meta: Some(serde_json::json!({"createdAt": 1000.0, "cwd": path})),
                ..Default::default()
            },
        )
        .unwrap();
    workspace.attach_session("live-float".into()).await.unwrap();
    assert_eq!(workspace.session_ids(), vec![SessionId::from("live-float")]);
}

#[tokio::test]
async fn logs_each_stop_failure_after_the_archive_has_committed() {
    let h = Harness::new(vec![header("busy", None, 1)]).await;
    let _first = h
        .ctx
        .on_parallel::<WorkspaceSessionStop, _>(|_| async {
            Err::<(), _>("job kill exploded".into())
        })
        .unwrap();
    let _second = h
        .ctx
        .on_parallel::<WorkspaceSessionStop, _>(|_| async {
            Err::<(), _>("tool shutdown exploded".into())
        })
        .unwrap();
    h.registry
        .archive_session(
            "busy".into(),
            ArchiveSessionOptions {
                stop_activity: true,
            },
        )
        .await
        .unwrap();
    let messages = h
        .ctx
        .logger()
        .buffer()
        .into_iter()
        .map(|record| record.message)
        .collect::<Vec<_>>();
    assert!(messages.iter().any(|message| message.contains(
        "workspace: stopping session 'busy' for archive failed: Error: job kill exploded"
    )));
    assert!(messages.iter().any(|message| message.contains(
        "workspace: stopping session 'busy' for archive failed: Error: tool shutdown exploded"
    )));
}

#[test]
fn accepts_unc_roots_with_mixed_separators_and_normalizes_drive_root_titles() {
    assert!(fully_qualified_workspace_path_for(
        "//server/share/folder",
        PathPlatform::Windows
    ));
    assert!(fully_qualified_workspace_path_for(
        r"\/server/share",
        PathPlatform::Windows
    ));
    assert!(fully_qualified_workspace_path_for(
        r"/\server/share",
        PathPlatform::Windows
    ));
    assert!(!fully_qualified_workspace_path_for(
        "///server/share",
        PathPlatform::Windows
    ));
    assert!(!fully_qualified_workspace_path_for(
        r"\\\server\share",
        PathPlatform::Windows
    ));
    assert!(!fully_qualified_workspace_path_for(
        r"\\server",
        PathPlatform::Windows
    ));
    assert_eq!(
        default_workspace_title_for(r"C:\\\\", PathPlatform::Windows),
        "C:\\"
    );
}

#[tokio::test]
async fn breaks_equal_bootstrap_timestamps_by_session_id_and_canonical_path() {
    let root = TempDir::new().unwrap();
    let first = root.path().join("z-path");
    let second = root.path().join("a-path");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    let h = Harness::new(vec![
        header("z-session", Some(first.to_str().unwrap()), 100),
        header("a-session", Some(first.to_str().unwrap()), 100),
        header("second-session", Some(second.to_str().unwrap()), 100),
    ])
    .await;
    let workspaces = h.registry.list().unwrap();
    assert_eq!(
        workspaces
            .iter()
            .map(Workspace::path)
            .collect::<HashSet<_>>(),
        HashSet::from([
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ])
    );
    let first = workspaces
        .iter()
        .find(|workspace| workspace.path() == first.to_string_lossy())
        .unwrap();
    assert_eq!(
        first.session_ids(),
        vec![SessionId::from("a-session"), SessionId::from("z-session")]
    );
}

#[tokio::test]
async fn reuses_durable_bootstrap_order_when_the_final_initialized_marker_write_fails() {
    let medium = Arc::new(Medium::default());
    medium.fail_global_offset(2);
    let temp = TempDir::new().unwrap();
    let headers = vec![header("session", Some(temp.path().to_str().unwrap()), 100)];
    assert!(
        Harness::try_with_medium(headers.clone(), medium.clone())
            .await
            .is_err()
    );
    let before = medium.snapshot.lock().unwrap().global.clone();
    assert_eq!(before["initialized"], false);
    let id = before["workspaceIds"][0].as_str().unwrap().to_owned();
    let retried = Harness::with_medium(headers, medium).await;
    assert_eq!(retried.registry.list().unwrap()[0].id().as_str(), id);
    assert_eq!(medium_state(&retried)["initialized"], true);
}

#[tokio::test]
async fn merges_partial_records_and_leaves_an_accounted_cwd_drift_ungrouped() {
    let root = TempDir::new().unwrap();
    let owned = root.path().join("owned");
    let prior = root.path().join("prior");
    let drifted = root.path().join("drifted");
    for path in [&owned, &prior, &drifted] {
        std::fs::create_dir(path).unwrap();
    }
    let medium = Arc::new(Medium::default());
    seed_medium(
        &medium,
        [
            (
                "owned-id",
                stored_workspace(
                    owned.to_str().unwrap(),
                    &["old"],
                    "2026-07-24T00:00:00.000Z",
                ),
            ),
            (
                "prior-id",
                stored_workspace(
                    prior.to_str().unwrap(),
                    &["drift"],
                    "2026-07-23T00:00:00.000Z",
                ),
            ),
        ],
        serde_json::json!({"initialized":false,"workspaceIds":[]}),
    );
    let h = Harness::with_medium(
        vec![
            header("new", Some(owned.to_str().unwrap()), 200),
            header("old", Some(owned.to_str().unwrap()), 100),
            header("drift", Some(drifted.to_str().unwrap()), 300),
        ],
        medium,
    )
    .await;
    assert_eq!(
        h.registry.get(&"owned-id".into()).unwrap().session_ids(),
        vec![SessionId::from("new"), SessionId::from("old")]
    );
    assert!(
        h.registry
            .list()
            .unwrap()
            .iter()
            .all(|workspace| workspace.path() != drifted.to_string_lossy())
    );
}

#[tokio::test]
async fn orders_headerless_partial_records_by_prior_order_then_locale_id() {
    let root = TempDir::new().unwrap();
    let first = root.path().join("first");
    let second = root.path().join("second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    let records = || {
        [
            (
                "b-id",
                stored_workspace(second.to_str().unwrap(), &[], "2026-07-24T00:00:00.000Z"),
            ),
            (
                "a-id",
                stored_workspace(first.to_str().unwrap(), &[], "2026-07-24T00:00:00.000Z"),
            ),
        ]
    };
    let prior_medium = Arc::new(Medium::default());
    seed_medium(
        &prior_medium,
        records(),
        serde_json::json!({"initialized":false,"workspaceIds":["b-id","a-id"]}),
    );
    let prior = Harness::with_medium(vec![], prior_medium).await;
    assert_eq!(
        prior
            .registry
            .list()
            .unwrap()
            .iter()
            .map(|workspace| workspace.id().as_str())
            .collect::<Vec<_>>(),
        vec!["b-id", "a-id"]
    );
    let id_medium = Arc::new(Medium::default());
    seed_medium(
        &id_medium,
        records(),
        serde_json::json!({"initialized":false,"workspaceIds":[]}),
    );
    let by_id = Harness::with_medium(vec![], id_medium).await;
    assert_eq!(
        by_id
            .registry
            .list()
            .unwrap()
            .iter()
            .map(|workspace| workspace.id().as_str())
            .collect::<Vec<_>>(),
        vec!["a-id", "b-id"]
    );
}

#[tokio::test]
async fn commits_deletion_and_recovers_a_marker_cleanup_failure_before_the_next_create() {
    let medium = Arc::new(Medium::default());
    let h = Harness::with_medium(vec![], medium.clone()).await;
    let temp = TempDir::new().unwrap();
    let first = h
        .registry
        .create(temp.path().to_str().unwrap(), None)
        .await
        .unwrap();
    medium.fail_global_offset(2);
    assert!(h.registry.delete(first.id()).await.unwrap());
    assert_eq!(medium_state(&h)["pendingMutation"]["operation"], "delete");
    assert_eq!(h.persistence.lists.load(Ordering::SeqCst), 1);
    let messages = h
        .ctx
        .logger()
        .buffer()
        .into_iter()
        .map(|record| record.message)
        .collect::<Vec<_>>();
    assert!(messages.iter().any(|message| message
        == &format!(
            "workspace '{}' was deleted but its pending marker could not be cleared: Error: selected bootstrap marker failure",
            first.id()
        )));
    let second = h
        .registry
        .create(temp.path().to_str().unwrap(), None)
        .await
        .unwrap();
    assert_ne!(first.id(), second.id());
    assert!(medium_state(&h).get("pendingMutation").is_none());
    assert_eq!(h.persistence.lists.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn asks_about_activity_only_after_the_session_is_known() {
    let h = Harness::new(vec![header("known", None, 1)]).await;
    let calls = Arc::new(AtomicU64::new(0));
    let seen = calls.clone();
    let _listener = h
        .ctx
        .on_waterfall::<WorkspaceSessionActivity, Vec<SessionActivity>, _>(move |_, next| {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                next.run().await
            }
        })
        .unwrap();
    assert!(
        h.registry
            .archive_session("unknown".into(), ArchiveSessionOptions::default())
            .await
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    h.registry
        .archive_session("known".into(), ArchiveSessionOptions::default())
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn filters_invalid_candidates_and_prunes_them_on_the_next_entity_write() {
    let root = TempDir::new().unwrap();
    let owned = root.path().join("owned");
    let elsewhere = root.path().join("elsewhere");
    let missing = root.path().join("missing");
    let file = root.path().join("file");
    std::fs::create_dir(&owned).unwrap();
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::write(&file, "not a directory").unwrap();
    let medium = Arc::new(Medium::default());
    seed_medium(
        &medium,
        [(
            "workspace",
            stored_workspace(
                owned.to_str().unwrap(),
                &[
                    "missing",
                    "absent",
                    "no-cwd",
                    "relative",
                    "file",
                    "elsewhere",
                ],
                "2026-01-01T00:00:00.000Z",
            ),
        )],
        serde_json::json!({"initialized":true,"workspaceIds":["workspace"]}),
    );
    let h = Harness::with_medium(
        vec![
            header("missing", Some(missing.to_str().unwrap()), 1),
            header("no-cwd", None, 2),
            header("relative", Some("relative/path"), 3),
            header("file", Some(file.to_str().unwrap()), 4),
            header("elsewhere", Some(elsewhere.to_str().unwrap()), 5),
        ],
        medium.clone(),
    )
    .await;
    let workspace = h.registry.get(&"workspace".into()).unwrap();
    assert_eq!(workspace.session_ids(), vec![SessionId::from("missing")]);
    workspace.set_title("validated").await.unwrap();
    assert_eq!(
        medium.snapshot.lock().unwrap().tables["workspaces"]["workspace"]["sessionIds"],
        serde_json::json!(["missing"])
    );
}

#[tokio::test]
async fn detaches_and_reattaches_a_missing_directory_candidate_after_restore() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("restored");
    std::fs::create_dir(&path).unwrap();
    let canonical = std::fs::canonicalize(&path).unwrap();
    std::fs::remove_dir(&path).unwrap();
    let medium = Arc::new(Medium::default());
    seed_medium(
        &medium,
        [(
            "workspace",
            stored_workspace(
                canonical.to_str().unwrap(),
                &["session"],
                "2026-01-01T00:00:00.000Z",
            ),
        )],
        serde_json::json!({"initialized":true,"workspaceIds":["workspace"]}),
    );
    let h = Harness::with_medium(
        vec![header("session", Some(canonical.to_str().unwrap()), 1)],
        medium,
    )
    .await;
    let workspace = h.registry.get(&"workspace".into()).unwrap();
    assert_eq!(workspace.session_ids(), vec![SessionId::from("session")]);
    workspace.detach_session("session".into()).await.unwrap();
    std::fs::create_dir(&path).unwrap();
    workspace.attach_session("session".into()).await.unwrap();
    assert_eq!(workspace.session_ids(), vec![SessionId::from("session")]);
}

#[tokio::test]
async fn recovers_only_the_workspace_named_by_an_explicit_pending_marker() {
    let dir = TempDir::new().unwrap();
    let medium = Arc::new(Medium::default());
    seed_medium(
        &medium,
        [(
            "interrupted",
            stored_workspace(
                dir.path().to_str().unwrap(),
                &[],
                "2026-01-01T00:00:00.000Z",
            ),
        )],
        serde_json::json!({"initialized":true,"workspaceIds":[],"pendingMutation":{"operation":"create","workspaceId":"interrupted"}}),
    );
    let created = Harness::with_medium(vec![], medium.clone()).await;
    assert!(created.registry.list().unwrap().is_empty());
    assert!(medium.snapshot.lock().unwrap().tables["workspaces"].is_empty());
    assert!(medium_state(&created).get("pendingMutation").is_none());

    let deleted_medium = Arc::new(Medium::default());
    seed_medium(
        &deleted_medium,
        [],
        serde_json::json!({"initialized":true,"workspaceIds":[],"pendingMutation":{"operation":"delete","workspaceId":"gone"}}),
    );
    let deleted = Harness::with_medium(vec![], deleted_medium).await;
    assert!(medium_state(&deleted).get("pendingMutation").is_none());
}

#[tokio::test]
async fn orders_headerless_records_using_date_parse_offsets_and_falls_through_invalid_dates() {
    let root = TempDir::new().unwrap();
    let older = root.path().join("older");
    let newer = root.path().join("newer");
    let invalid = root.path().join("invalid");
    for path in [&older, &newer, &invalid] {
        std::fs::create_dir(path).unwrap();
    }
    let medium = Arc::new(Medium::default());
    seed_medium(
        &medium,
        [
            (
                "older",
                stored_workspace(older.to_str().unwrap(), &[], "2026-01-01T01:00:00+01:00"),
            ),
            (
                "newer",
                stored_workspace(newer.to_str().unwrap(), &[], "2026-01-01T00:30:00Z"),
            ),
            (
                "invalid",
                stored_workspace(invalid.to_str().unwrap(), &[], "not-a-date"),
            ),
        ],
        serde_json::json!({"initialized":false,"workspaceIds":["invalid"]}),
    );
    let h = Harness::with_medium(vec![], medium).await;
    let ids = h
        .registry
        .list()
        .unwrap()
        .iter()
        .map(|workspace| workspace.id().as_str().to_owned())
        .collect::<Vec<_>>();
    assert!(ids.iter().position(|id| id == "newer") < ids.iter().position(|id| id == "older"));
}

#[tokio::test]
async fn formats_bootstrap_dates_at_the_javascript_extended_year_boundary() {
    let temp = TempDir::new().unwrap();
    let h = Harness::new(vec![header(
        "far-future",
        Some(temp.path().to_str().unwrap()),
        8_640_000_000_000_000,
    )])
    .await;
    assert_eq!(
        h.registry.list().unwrap()[0].created_at(),
        "+275760-09-13T00:00:00.000Z"
    );
}

#[tokio::test]
async fn preserves_the_realpath_error_as_the_attach_validation_cause() {
    let root = TempDir::new().unwrap();
    let workspace_path = root.path().join("workspace");
    std::fs::create_dir(&workspace_path).unwrap();
    let missing = root.path().join("missing");
    let h = Harness::new(vec![header("missing", Some(missing.to_str().unwrap()), 1)]).await;
    let workspace = h
        .registry
        .create(workspace_path.to_str().unwrap(), None)
        .await
        .unwrap();
    let error = workspace
        .attach_session("missing".into())
        .await
        .unwrap_err();
    let validation = error
        .downcast_ref::<WorkspaceAttachValidationError>()
        .unwrap();
    assert!(std::error::Error::source(validation).is_some());
    assert!(validation.to_string().contains("does not resolve"));
}

#[tokio::test]
async fn pins_in_prepend_order_and_keeps_repeat_pins_write_free() {
    let h = Harness::new(vec![header("one", None, 1), header("two", None, 2)]).await;
    h.registry.pin_session("one".into()).await.unwrap();
    h.registry.pin_session("two".into()).await.unwrap();
    let before = medium_state(&h);
    h.registry.pin_session("two".into()).await.unwrap();
    assert_eq!(
        h.registry.pinned_session_ids(),
        vec![SessionId::from("two"), SessionId::from("one")]
    );
    assert_eq!(medium_state(&h), before);
}

#[tokio::test]
async fn unpins_in_place_and_idempotently_skips_absent_ids() {
    let h = Harness::new(vec![header("one", None, 1), header("two", None, 2)]).await;
    h.registry.pin_session("one".into()).await.unwrap();
    h.registry.pin_session("two".into()).await.unwrap();
    h.registry.unpin_session(&"one".into()).await.unwrap();
    let before = medium_state(&h);
    h.registry.unpin_session(&"one".into()).await.unwrap();
    assert_eq!(
        h.registry.pinned_session_ids(),
        vec![SessionId::from("two")]
    );
    assert_eq!(medium_state(&h), before);
}

#[tokio::test]
async fn plugin_waits_for_session_persistence_before_opening_the_domain() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let storage = Arc::new(StorageService::new());
    let medium = Arc::new(Medium::default());
    let _registration = storage
        .backend
        .register("memory", Arc::new(Backend(medium.clone())))
        .unwrap();
    ctx.provide::<Storage>(storage).await.unwrap();
    ctx.provide::<StorageDomain>(Arc::new(DomainFacility::new(
        ctx.clone(),
        DomainConfig {
            backend: "memory".into(),
            routes: IndexMap::new(),
        },
    )))
    .await
    .unwrap();

    let fiber = ctx
        .plugin::<WorkspaceRegistryPlugin>(serde_json::Value::Null)
        .unwrap();
    tokio::task::yield_now().await;
    assert!(ctx.get::<WorkspaceRegistryService>().is_none());
    assert!(medium.snapshot.lock().unwrap().tables.is_empty());
    assert!(medium.snapshot.lock().unwrap().global.is_null());

    let persistence = Arc::new(Persistence::new(vec![]));
    ctx.provide::<SessionPersistenceService>(persistence.clone())
        .await
        .unwrap();
    fiber.settled().await.unwrap();
    assert!(ctx.get::<WorkspaceRegistryService>().is_some());
    assert_eq!(persistence.lists.load(Ordering::SeqCst), 1);
    assert_eq!(
        medium.snapshot.lock().unwrap().global,
        serde_json::json!({
            "initialized":true,
            "workspaceIds":[],
            "archivedSessionIds":[],
            "pinnedSessionIds":[]
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn entity_mutations_keep_first_poll_order_on_a_multithread_runtime() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().to_string_lossy().into_owned();
    let h = Harness::new(vec![header("s1", Some(&path), 1)]).await;
    let workspace = h.registry.list().unwrap()[0].clone();
    assert_eq!(workspace.session_ids(), vec![SessionId::from("s1")]);

    for _ in 0..200 {
        let (detached, attached) = tokio::join!(
            workspace.detach_session("s1".into()),
            workspace.attach_session("s1".into())
        );
        detached.unwrap();
        attached.unwrap();
        assert_eq!(workspace.session_ids(), vec![SessionId::from("s1")]);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn registry_mutations_keep_archive_before_pin_on_a_multithread_runtime() {
    let headers = (0..200)
        .map(|index| header(&format!("session-{index}"), None, index))
        .collect();
    let h = Harness::new(headers).await;

    for index in 0..200 {
        let id = SessionId::from(format!("session-{index}"));
        let (archived, pinned) = tokio::join!(
            h.registry
                .archive_session(id.clone(), ArchiveSessionOptions::default()),
            h.registry.pin_session(id.clone())
        );
        archived.unwrap();
        assert!(pinned.unwrap_err().is::<WorkspaceArchivedSessionPinError>());
    }
    assert!(h.registry.pinned_session_ids().is_empty());
}

#[tokio::test]
async fn a_lazy_live_session_attach_does_not_relist_persistence() {
    let h = Harness::with_session_store(vec![], Arc::new(Medium::default())).await;
    let temp = TempDir::new().unwrap();
    let path = temp.path().to_string_lossy().into_owned();
    h.sessions
        .as_ref()
        .unwrap()
        .create(
            &h.ctx,
            Some("live".into()),
            hoh_session::CreateSessionOptions {
                meta: Some(serde_json::json!({"cwd":path})),
                ..Default::default()
            },
        )
        .unwrap();
    let workspace = h.registry.create(&path, None).await.unwrap();
    workspace.attach_session("live".into()).await.unwrap();
    assert_eq!(workspace.session_ids(), vec![SessionId::from("live")]);
    assert_eq!(h.persistence.lists.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn list_and_membership_projection_do_not_relist_persistence() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().to_string_lossy().into_owned();
    let h = Harness::new(vec![header("owned", Some(&path), 1)]).await;
    assert_eq!(h.persistence.lists.load(Ordering::SeqCst), 1);
    *h.persistence.fail.lock().unwrap() = Some("list must stay cached".into());
    let first = h.registry.list().unwrap();
    let second = h.registry.list().unwrap();
    assert_eq!(first[0].session_ids(), vec![SessionId::from("owned")]);
    assert_eq!(second[0].session_ids(), vec![SessionId::from("owned")]);
    assert_eq!(h.persistence.lists.load(Ordering::SeqCst), 1);
    assert_eq!(
        h.persistence.fail.lock().unwrap().as_deref(),
        Some("list must stay cached")
    );
}

#[tokio::test]
async fn live_and_archived_cwdless_sessions_block_default_initialization() {
    let live = Harness::with_session_store(vec![], Arc::new(Medium::default())).await;
    live.sessions
        .as_ref()
        .unwrap()
        .create(
            &live.ctx,
            Some("live-no-cwd".into()),
            hoh_session::CreateSessionOptions::default(),
        )
        .unwrap();
    let live_called = Arc::new(AtomicU64::new(0));
    let called = live_called.clone();
    assert!(
        live.registry
            .initialize_default(move || async move {
                called.fetch_add(1, Ordering::SeqCst);
                Err("must not resolve".into())
            })
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(live_called.load(Ordering::SeqCst), 0);

    let archived = Harness::with_session_store(
        vec![header("archived-no-cwd", None, 1)],
        Arc::new(Medium::default()),
    )
    .await;
    archived
        .registry
        .archive_session("archived-no-cwd".into(), ArchiveSessionOptions::default())
        .await
        .unwrap();
    archived.persistence.headers.lock().unwrap().clear();
    let archived_called = Arc::new(AtomicU64::new(0));
    let called = archived_called.clone();
    assert!(
        archived
            .registry
            .initialize_default(move || async move {
                called.fetch_add(1, Ordering::SeqCst);
                Err("must not resolve".into())
            })
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(archived_called.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_live_session_arriving_during_directory_preparation_blocks_default_creation() {
    let h = Harness::with_session_store(vec![], Arc::new(Medium::default())).await;
    let temp = TempDir::new().unwrap();
    let requested = temp.path().join("Workspace");
    let path = requested.to_string_lossy().into_owned();
    let sessions = h.sessions.as_ref().unwrap().clone();
    let ctx = h.ctx.clone();
    let result = h
        .registry
        .initialize_default(move || async move {
            sessions
                .create(
                    &ctx,
                    Some("arrived-live".into()),
                    hoh_session::CreateSessionOptions::default(),
                )
                .unwrap();
            Ok(path)
        })
        .await
        .unwrap();
    assert!(result.is_none());
    assert!(h.registry.list().unwrap().is_empty());
    assert!(medium_state(&h).get("defaultWorkspaceId").is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn an_owned_symlink_candidate_survives_when_its_target_disappears() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().unwrap();
    let target = root.path().join("target");
    let alias = root.path().join("alias");
    std::fs::create_dir(&target).unwrap();
    symlink(&target, &alias).unwrap();
    let canonical = std::fs::canonicalize(&target)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let medium = Arc::new(Medium::default());
    seed_medium(
        &medium,
        [(
            "workspace",
            stored_workspace(&canonical, &["linked"], "2026-01-01T00:00:00.000Z"),
        )],
        serde_json::json!({"initialized":true,"workspaceIds":["workspace"]}),
    );
    std::fs::remove_dir(&target).unwrap();
    let restored = Harness::with_medium(
        vec![header("linked", Some(alias.to_str().unwrap()), 1)],
        medium,
    )
    .await;
    assert_eq!(
        restored
            .registry
            .get(&"workspace".into())
            .unwrap()
            .session_ids(),
        vec![SessionId::from("linked")]
    );
    let fresh = Harness::new(vec![header("linked", Some(alias.to_str().unwrap()), 1)]).await;
    assert!(fresh.registry.list().unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn loop_and_not_directory_failures_do_not_preserve_candidates() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().unwrap();
    let owned = root.path().join("owned");
    std::fs::create_dir(&owned).unwrap();
    let loop_path = root.path().join("loop");
    symlink(&loop_path, &loop_path).unwrap();
    let file = root.path().join("file");
    std::fs::write(&file, "not a directory").unwrap();
    let not_directory = file.join("child");

    let medium = Arc::new(Medium::default());
    seed_medium(
        &medium,
        [(
            "workspace",
            stored_workspace(
                owned.to_str().unwrap(),
                &["loop", "not-directory"],
                "2026-01-01T00:00:00.000Z",
            ),
        )],
        serde_json::json!({"initialized":true,"workspaceIds":["workspace"]}),
    );
    let result = Harness::with_medium(
        vec![
            header("loop", Some(loop_path.to_str().unwrap()), 2),
            header("not-directory", Some(not_directory.to_str().unwrap()), 3),
        ],
        medium.clone(),
    )
    .await;
    let workspace = result.registry.get(&"workspace".into()).unwrap();
    assert!(workspace.session_ids().is_empty());
    workspace.set_title("pruned").await.unwrap();
    assert_eq!(
        medium.snapshot.lock().unwrap().tables["workspaces"]["workspace"]["sessionIds"],
        serde_json::json!([])
    );
}

#[tokio::test]
async fn reindexing_clears_and_reestablishes_the_missing_path_exemption() {
    let root = TempDir::new().unwrap();
    let missing = root.path().join("missing");
    std::fs::create_dir(&missing).unwrap();
    let canonical = std::fs::canonicalize(&missing)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    std::fs::remove_dir(&missing).unwrap();
    let elsewhere = root.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let medium = Arc::new(Medium::default());
    seed_medium(
        &medium,
        [(
            "workspace",
            stored_workspace(&canonical, &["kept"], "2026-01-01T00:00:00.000Z"),
        )],
        serde_json::json!({"initialized":true,"workspaceIds":["workspace"]}),
    );
    let h = Harness::with_medium(vec![header("kept", Some(&canonical), 1)], medium.clone()).await;
    let workspace = h.registry.get(&"workspace".into()).unwrap();
    assert_eq!(workspace.session_ids(), vec![SessionId::from("kept")]);

    *h.persistence.headers.lock().unwrap() = vec![
        header("kept", Some(elsewhere.to_str().unwrap()), 2),
        header("trigger-one", Some(elsewhere.to_str().unwrap()), 2),
    ];
    h.registry.pin_session("trigger-one".into()).await.unwrap();
    assert!(workspace.session_ids().is_empty());

    *h.persistence.headers.lock().unwrap() = vec![
        header("kept", Some(&canonical), 3),
        header("trigger-two", Some(elsewhere.to_str().unwrap()), 3),
    ];
    h.registry.pin_session("trigger-two".into()).await.unwrap();
    assert_eq!(workspace.session_ids(), vec![SessionId::from("kept")]);

    *h.persistence.headers.lock().unwrap() = vec![
        header("kept", None, 4),
        header("trigger-three", Some(elsewhere.to_str().unwrap()), 4),
    ];
    h.registry
        .pin_session("trigger-three".into())
        .await
        .unwrap();
    assert!(workspace.session_ids().is_empty());
    workspace.set_title("pruned").await.unwrap();
    assert_eq!(
        medium.snapshot.lock().unwrap().tables["workspaces"]["workspace"]["sessionIds"],
        serde_json::json!([])
    );
}
