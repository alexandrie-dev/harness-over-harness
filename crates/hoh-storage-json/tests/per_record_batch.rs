//! Included as crate unit tests so fault injection stays private and per-instance.
use crate::{
    atomic,
    per_record_unit::{BatchDeleteError, DirectoryIo, PerRecordJsonUnit},
    runtime::lock,
};
use hoh_error::{BoxError, SharedError};
use hoh_storage::{BackendFuture, KvLayout, KvUnit, KvUnitDescriptor, StorageError};
use hoh_util_values::{js_json_parse, js_json_stringify};
use serde_json::{Value, json};
use std::{
    error::Error,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tempfile::TempDir;
use tokio::fs;

type RenameHook = Arc<dyn Fn(String, String) -> BackendFuture<'static, bool> + Send + Sync>;
type SyncHook = Arc<dyn Fn(String) -> BackendFuture<'static, bool> + Send + Sync>;
#[derive(Default)]
struct Gates {
    rename: Mutex<Option<RenameHook>>,
    sync: Mutex<Option<SyncHook>>,
}
impl DirectoryIo for Gates {
    fn rename<'a>(&'a self, from: &'a str, to: &'a str) -> BackendFuture<'a, ()> {
        let hook = lock(&self.rename).clone();
        Box::pin(async move {
            if let Some(hook) = hook
                && hook(from.into(), to.into()).await?
            {
                return Ok(());
            }
            atomic::rename(from, to).await
        })
    }
    fn sync<'a>(&'a self, dir: &'a str) -> BackendFuture<'a, ()> {
        let hook = lock(&self.sync).clone();
        Box::pin(async move {
            if let Some(hook) = hook
                && hook(dir.into()).await?
            {
                return Ok(());
            }
            atomic::fsync_directory(dir).await
        })
    }
}
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct Sentinel(&'static str);
fn descriptor() -> KvUnitDescriptor {
    KvUnitDescriptor {
        name: "batch".into(),
        version: 1,
        tables: vec!["t".into()],
        has_global: true,
        layout: Some(KvLayout::PerRecord),
        compatible_versions: None,
    }
}
fn setup() -> (TempDir, Arc<Gates>, Arc<PerRecordJsonUnit>) {
    let root = tempfile::tempdir().unwrap();
    let gates = Arc::new(Gates::default());
    let unit = Arc::new(PerRecordJsonUnit::with_io(
        descriptor(),
        root.path().join("batch").to_str().unwrap().into(),
        Arc::new(|| Ok(())),
        gates.clone(),
    ));
    (root, gates, unit)
}
async fn state(unit: &PerRecordJsonUnit) -> Value {
    serde_json::to_value(unit.load_all().await.unwrap()).unwrap()
}
fn storage_error(error: &BoxError) -> &StorageError {
    error
        .downcast_ref::<StorageError>()
        .expect("one downcast must recognize every StorageError")
}
fn code(error: &BoxError, expected: &str) {
    assert_eq!(storage_error(error).code(), expected);
}
async fn seed(unit: &PerRecordJsonUnit) {
    unit.put_record("t", "a", json!({"v":1})).await.unwrap();
    unit.put_record("t", "b", json!({"v":2})).await.unwrap();
}
fn same(error: &BoxError, expected: &SharedError) {
    assert!(
        error
            .downcast_ref::<SharedError>()
            .unwrap()
            .ptr_eq(expected)
    );
}

async fn restores_after_sync(single: bool) {
    let (root, gates, unit) = setup();
    seed(&unit).await;
    let path = root.path().join("batch/t/a.json");
    let before = fs::read(&path).await.unwrap();
    let failure = SharedError::new(Sentinel("directory sync failed"));
    let captured = failure.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    *lock(&gates.sync) = Some(Arc::new(move |_| {
        let failure = captured.clone();
        let first = count.fetch_add(1, Ordering::SeqCst) == 0;
        Box::pin(async move {
            if first {
                Err(failure.into())
            } else {
                Ok(false)
            }
        })
    }));
    let keys = vec!["a".into(), "b".into()];
    let remove = || {
        if single {
            unit.delete_record("t", "a")
        } else {
            unit.delete_records("t", &keys)
        }
    };
    same(&remove().await.unwrap_err(), &failure);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(fs::read(&path).await.unwrap(), before);
    assert_eq!(
        state(&unit).await,
        json!({"tables":{"t":{"a":{"v":1},"b":{"v":2}}},"global":null})
    );
    remove().await.unwrap();
    assert_eq!(
        state(&unit).await,
        if single {
            json!({"tables":{"t":{"b":{"v":2}}},"global":null})
        } else {
            json!({"tables":{"t":{}},"global":null})
        }
    );
    unit.close().await.unwrap();
}

#[tokio::test]
async fn restores_documents_after_a_directory_sync_failure_in_delete_record_then_retries() {
    restores_after_sync(true).await;
}
#[tokio::test]
async fn restores_documents_after_a_directory_sync_failure_in_delete_records_then_retries() {
    restores_after_sync(false).await;
}

#[tokio::test]
async fn quarantines_a_deletion_whose_restored_directory_cannot_be_synced() {
    let (_root, gates, unit) = setup();
    unit.put_record("t", "a", json!({"v":1})).await.unwrap();
    let failure = SharedError::new(Sentinel("directory sync denied"));
    let captured = failure.clone();
    *lock(&gates.sync) = Some(Arc::new(move |_| {
        let failure = captured.clone();
        Box::pin(async move { Err(failure.into()) })
    }));
    let error = unit.delete_records("t", &["a".into()]).await.unwrap_err();
    code(&error, "unknown-state");
    let cause = storage_error(&error).source().unwrap();
    let cause = cause
        .downcast_ref::<Arc<dyn Error + Send + Sync>>()
        .map_or(cause, |cause| cause.as_ref());
    let aggregate = cause.downcast_ref::<BatchDeleteError>().unwrap();
    assert_eq!(aggregate.errors.len(), 2);
    for error in &aggregate.errors {
        same(error, &failure);
    }
    let later = unit.load_all().await.unwrap_err();
    code(&later, "unknown-state");
    let first_cause = storage_error(&error)
        .source()
        .unwrap()
        .downcast_ref::<Arc<dyn Error + Send + Sync>>()
        .unwrap();
    let later_cause = storage_error(&later)
        .source()
        .unwrap()
        .downcast_ref::<Arc<dyn Error + Send + Sync>>()
        .unwrap();
    assert!(
        Arc::ptr_eq(first_cause, later_cause),
        "the original aggregate cause is retained across clones"
    );
    code(
        &unit.delete_records("t", &[]).await.unwrap_err(),
        "unknown-state",
    );
    unit.close().await.unwrap();
    code(&unit.load_all().await.unwrap_err(), "closed");
}
#[tokio::test]
async fn does_not_sync_or_create_directories_for_empty_and_missing_key_deletions() {
    let (root, gates, unit) = setup();
    *lock(&gates.sync) = Some(Arc::new(|_| {
        Box::pin(async { Err(Box::new(Sentinel("unexpected directory sync")) as BoxError) })
    }));
    unit.delete_records("t", &[]).await.unwrap();
    unit.delete_records("t", &["missing".into(), "missing".into()])
        .await
        .unwrap();
    unit.delete_record("t", "missing").await.unwrap();
    assert!(
        fs::read_dir(root.path())
            .await
            .unwrap()
            .next_entry()
            .await
            .unwrap()
            .is_none()
    );
    unit.close().await.unwrap();
}
#[tokio::test]
async fn leaves_a_failed_first_staging_operation_unapplied_without_syncing() {
    let (_root, gates, unit) = setup();
    unit.put_record("t", "a", json!({"v":1})).await.unwrap();
    let failure = SharedError::new(Sentinel("staging denied"));
    let captured = failure.clone();
    *lock(&gates.rename) = Some(Arc::new(move |_, _| {
        let failure = captured.clone();
        Box::pin(async move { Err(failure.into()) })
    }));
    *lock(&gates.sync) = Some(Arc::new(|_| {
        Box::pin(async { Err(Box::new(Sentinel("unexpected directory sync")) as BoxError) })
    }));
    same(
        &unit.delete_records("t", &["a".into()]).await.unwrap_err(),
        &failure,
    );
    assert_eq!(
        state(&unit).await,
        json!({"tables":{"t":{"a":{"v":1}}},"global":null})
    );
    unit.close().await.unwrap();
}
#[tokio::test]
async fn leaves_an_unmaterialized_tree_eligible_for_bootstrap_after_a_rejected_write() {
    let (root, _gates, unit) = setup();
    // Source throwing toJSON is not representable by Value. An undeclared
    // table likewise rejects before creating a directory; this is an adaptation.
    assert!(
        unit.put_record("undeclared", "broken", json!({}))
            .await
            .is_err()
    );
    assert!(
        fs::read_dir(root.path())
            .await
            .unwrap()
            .next_entry()
            .await
            .unwrap()
            .is_none()
    );
    fs::write(root.path().join("batch.json"),js_json_stringify(&json!({"unit":{"name":"batch","version":1},"global":null,"tables":{"t":{"saved":{"v":1}}}}))).await.unwrap();
    assert_eq!(state(&unit).await["tables"]["t"], json!({"saved":{"v":1}}));
    unit.close().await.unwrap();
}
async fn emptied_tree(single: bool) {
    let (root, _gates, original) = setup();
    original.close().await.unwrap();
    let legacy = js_json_stringify(
        &json!({"unit":{"name":"batch","version":0},"global":null,"tables":{"t":{"a":{"v":1},"b":{"v":2}}}}),
    );
    let path = root.path().join("batch.json");
    fs::write(&path, &legacy).await.unwrap();
    let mut desc = descriptor();
    desc.compatible_versions = Some(vec![0]);
    let backend = crate::JsonStorageBackend::new(root.path().to_str().unwrap());
    let unit = hoh_storage::KvFacet::open(&backend, desc.clone())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(unit.load_all().await.unwrap()).unwrap()["tables"]["t"],
        json!({"a":{"v":1},"b":{"v":2}})
    );
    if single {
        unit.delete_record("t", "a").await.unwrap();
        unit.delete_record("t", "b").await.unwrap();
    } else {
        unit.delete_records("t", &["a".into(), "b".into()])
            .await
            .unwrap();
    }
    assert!(unit.load_all().await.unwrap().tables["t"].is_empty());
    hoh_storage::StorageBackend::close(&backend).await.unwrap();
    let reopened = crate::JsonStorageBackend::new(root.path().to_str().unwrap());
    assert!(
        hoh_storage::KvFacet::open(&reopened, desc)
            .await
            .unwrap()
            .load_all()
            .await
            .unwrap()
            .tables["t"]
            .is_empty()
    );
    assert_eq!(fs::read_to_string(path).await.unwrap(), legacy);
    hoh_storage::StorageBackend::close(&reopened).await.unwrap();
}
#[tokio::test]
async fn keeps_an_emptied_tree_authoritative_after_delete_record_including_reopen() {
    emptied_tree(true).await;
}
#[tokio::test]
async fn keeps_an_emptied_tree_authoritative_after_delete_records_including_reopen() {
    emptied_tree(false).await;
}

#[tokio::test]
async fn waits_for_the_whole_batch_before_close_releases_the_unit() {
    let (root, gates, unit) = setup();
    seed(&unit).await;
    unit.delete_records("t", &[]).await.unwrap();
    let first_entered = Arc::new(tokio::sync::Notify::new());
    let first_release = Arc::new(tokio::sync::Notify::new());
    let second_entered = Arc::new(tokio::sync::Notify::new());
    let second_release = Arc::new(tokio::sync::Notify::new());
    let (entered1, release1, entered2, release2) = (
        first_entered.clone(),
        first_release.clone(),
        second_entered.clone(),
        second_release.clone(),
    );
    *lock(&gates.rename) = Some(Arc::new(move |_, to| {
        let (entered1, release1, entered2, release2) = (
            entered1.clone(),
            release1.clone(),
            entered2.clone(),
            release2.clone(),
        );
        Box::pin(async move {
            if to.ends_with("a.json.deleting") {
                entered1.notify_one();
                release1.notified().await;
            }
            if to.ends_with("b.json.deleting") {
                entered2.notify_one();
                release2.notified().await;
            }
            Ok(false)
        })
    }));
    let keys = vec!["a".into(), "b".into()];
    let batch = unit.delete_records("t", &keys);
    first_entered.notified().await;
    let mut closing = unit.close();
    first_release.notify_one();
    second_entered.notified().await;
    // Poll deterministically while the second rename is held, without a timer.
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(
            Pin::new(&mut closing).poll(cx).is_pending()
        ))
        .await
    );
    second_release.notify_one();
    batch.await.unwrap();
    closing.await.unwrap();
    let backend = crate::JsonStorageBackend::new(root.path().to_str().unwrap());
    let reopened = hoh_storage::KvFacet::open(&backend, descriptor())
        .await
        .unwrap();
    assert!(reopened.load_all().await.unwrap().tables["t"].is_empty());
    hoh_storage::StorageBackend::close(&backend).await.unwrap();
}
#[tokio::test]
async fn rejects_with_unknown_state_when_a_failed_batch_cannot_restore_its_staged_documents() {
    let (root, gates, unit) = setup();
    seed(&unit).await;
    *lock(&gates.rename) = Some(Arc::new(|from, to| {
        Box::pin(async move {
            if to.ends_with("b.json.deleting") || from.ends_with("a.json.deleting") {
                return Err(Box::new(Sentinel("denied")) as BoxError);
            }
            Ok(false)
        })
    }));
    code(
        &unit
            .delete_records("t", &["a".into(), "b".into()])
            .await
            .unwrap_err(),
        "unknown-state",
    );
    code(&unit.load_all().await.unwrap_err(), "unknown-state");
    code(
        &unit.put_record("t", "c", json!({"v":3})).await.unwrap_err(),
        "unknown-state",
    );
    code(
        &unit.delete_record("t", "a").await.unwrap_err(),
        "unknown-state",
    );
    code(
        &unit.set_global(json!({"g":1})).await.unwrap_err(),
        "unknown-state",
    );
    code(
        &unit.backup_record("t", "b").unwrap().await.unwrap_err(),
        "unknown-state",
    );
    unit.close().await.unwrap();
    let backend = crate::JsonStorageBackend::new(root.path().to_str().unwrap());
    let reopened = hoh_storage::KvFacet::open(&backend, descriptor())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(reopened.load_all().await.unwrap()).unwrap(),
        json!({"tables":{"t":{"b":{"v":2}}},"global":null})
    );
    let residue = fs::read_to_string(root.path().join("batch/t/a.json.deleting"))
        .await
        .unwrap();
    assert_eq!(js_json_parse(&residue).unwrap()["record"], json!({"v":1}));
    hoh_storage::StorageBackend::close(&backend).await.unwrap();
}
