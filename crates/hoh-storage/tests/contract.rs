//! Shared backend contract, imported by backend crates with #[path = ...].
//! No backend is supplied by the hub itself. Rust JSON values cannot contain
//! the throwing `toJSON` objects in the two JavaScript serialization cases.
use hoh_storage::{KvUnitDescriptor, StorageBackend};
use serde_json::{Value, json};
use std::{future::Future, sync::Arc};

pub struct KvBackendContractHarness {
    pub backend: Arc<dyn StorageBackend>,
    pub reopen:
        Box<dyn Fn() -> hoh_storage::BackendFuture<'static, Arc<dyn StorageBackend>> + Send + Sync>,
}

fn descriptor() -> KvUnitDescriptor {
    KvUnitDescriptor {
        name: "contract_unit".into(),
        version: 3,
        tables: vec!["alpha".into(), "beta".into()],
        has_global: true,
        layout: None,
        compatible_versions: None,
    }
}

fn tables(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap()
}

pub async fn opens_a_missing_unit_as_empty_and_serves_load_all_immediately(
    h: KvBackendContractHarness,
) {
    let unit = h.backend.kv().unwrap().open(descriptor()).await.unwrap();
    let snapshot = unit.load_all().await.unwrap();
    assert_eq!(tables(snapshot.tables), json!({"alpha":{},"beta":{}}));
    assert_eq!(snapshot.global, Value::Null);
    h.backend.close().await.unwrap();
}

pub async fn round_trips_records_and_global_durably_across_reopen(h: KvBackendContractHarness) {
    let unit = h.backend.kv().unwrap().open(descriptor()).await.unwrap();
    unit.put_record("alpha", "k1", json!({"n":1}))
        .await
        .unwrap();
    unit.put_record("alpha", "k2", json!({"n":2}))
        .await
        .unwrap();
    unit.put_record("beta", "weird key / with:stuff", json!({"ok":true}))
        .await
        .unwrap();
    unit.set_global(json!({"counter":7})).await.unwrap();
    h.backend.close().await.unwrap();
    let reopened = (h.reopen)().await.unwrap();
    let snapshot = reopened
        .kv()
        .unwrap()
        .open(descriptor())
        .await
        .unwrap()
        .load_all()
        .await
        .unwrap();
    assert_eq!(
        tables(&snapshot.tables["alpha"]),
        json!({"k1":{"n":1},"k2":{"n":2}})
    );
    assert_eq!(
        tables(&snapshot.tables["beta"]),
        json!({"weird key / with:stuff":{"ok":true}})
    );
    assert_eq!(snapshot.global, json!({"counter":7}));
    reopened.close().await.unwrap();
}

pub async fn put_record_overwrites_and_delete_record_is_idempotent(h: KvBackendContractHarness) {
    let unit = h.backend.kv().unwrap().open(descriptor()).await.unwrap();
    unit.put_record("alpha", "k", json!({"v":"old"}))
        .await
        .unwrap();
    unit.put_record("alpha", "k", json!({"v":"new"}))
        .await
        .unwrap();
    unit.delete_record("alpha", "k").await.unwrap();
    unit.delete_record("alpha", "k").await.unwrap();
    unit.delete_record("alpha", "never-existed").await.unwrap();
    assert_eq!(
        tables(&unit.load_all().await.unwrap().tables["alpha"]),
        json!({})
    );
    h.backend.close().await.unwrap();
}

pub async fn delete_records_removes_the_whole_batch_durably_and_is_idempotent(
    h: KvBackendContractHarness,
) {
    let unit = h.backend.kv().unwrap().open(descriptor()).await.unwrap();
    for (key, n) in [("k1", 1), ("k2", 2), ("k3", 3)] {
        unit.put_record("alpha", key, json!({"n":n})).await.unwrap();
    }
    unit.put_record("beta", "b1", json!({"n":4})).await.unwrap();
    unit.delete_records(
        "alpha",
        &["k1".into(), "k3".into(), "missing".into(), "k1".into()],
    )
    .await
    .unwrap();
    unit.delete_records("alpha", &[]).await.unwrap();
    let expected = json!({"alpha":{"k2":{"n":2}},"beta":{"b1":{"n":4}}});
    assert_eq!(tables(unit.load_all().await.unwrap().tables), expected);
    h.backend.close().await.unwrap();
    let reopened = (h.reopen)().await.unwrap();
    assert_eq!(
        tables(
            reopened
                .kv()
                .unwrap()
                .open(descriptor())
                .await
                .unwrap()
                .load_all()
                .await
                .unwrap()
                .tables
        ),
        expected
    );
    reopened.close().await.unwrap();
}

pub async fn rejects_version_on_reopen_without_touching_the_data(
    h: KvBackendContractHarness,
    version: u64,
) {
    let unit = h.backend.kv().unwrap().open(descriptor()).await.unwrap();
    unit.put_record("alpha", "k", json!({"v":1})).await.unwrap();
    h.backend.close().await.unwrap();
    let reopened = (h.reopen)().await.unwrap();
    let mut changed = descriptor();
    changed.version = version;
    let error = reopened.kv().unwrap().open(changed).await.err().unwrap();
    let error = error.downcast_ref::<hoh_storage::StorageError>().unwrap();
    assert_eq!(error.name(), "StorageError");
    assert_eq!(error.code(), "version-mismatch");
    assert_eq!(
        tables(
            &reopened
                .kv()
                .unwrap()
                .open(descriptor())
                .await
                .unwrap()
                .load_all()
                .await
                .unwrap()
                .tables["alpha"]
        ),
        json!({"k":{"v":1}})
    );
    reopened.close().await.unwrap();
}

pub async fn reads_an_accepted_older_version_without_changing_its_stamp_or_records(
    h: KvBackendContractHarness,
) {
    let unit = h.backend.kv().unwrap().open(descriptor()).await.unwrap();
    unit.put_record("alpha", "k", json!({"v":1})).await.unwrap();
    unit.set_global(json!({"counter":1})).await.unwrap();
    h.backend.close().await.unwrap();
    let reader = (h.reopen)().await.unwrap();
    let mut current = descriptor();
    current.version = 4;
    current.compatible_versions = Some(vec![3]);
    let snapshot = reader
        .kv()
        .unwrap()
        .open(current)
        .await
        .unwrap()
        .load_all()
        .await
        .unwrap();
    assert_eq!(
        tables(snapshot),
        json!({"tables":{"alpha":{"k":{"v":1}},"beta":{}},"global":{"counter":1}})
    );
    reader.close().await.unwrap();
    let unchanged = (h.reopen)().await.unwrap();
    assert_eq!(
        tables(
            &unchanged
                .kv()
                .unwrap()
                .open(descriptor())
                .await
                .unwrap()
                .load_all()
                .await
                .unwrap()
                .tables["alpha"]
        ),
        json!({"k":{"v":1}})
    );
    unchanged.close().await.unwrap();
}

pub async fn keeps_the_older_stamp_for_a_no_op(h: KvBackendContractHarness, operation: &str) {
    let unit = h.backend.kv().unwrap().open(descriptor()).await.unwrap();
    unit.put_record("alpha", "saved", json!({"v":1}))
        .await
        .unwrap();
    h.backend.close().await.unwrap();
    let writer = (h.reopen)().await.unwrap();
    let mut current = descriptor();
    current.version = 4;
    current.compatible_versions = Some(vec![3]);
    let unit = writer.kv().unwrap().open(current).await.unwrap();
    match operation {
        "deleteRecord" => unit.delete_record("alpha", "missing").await.unwrap(),
        "deleteRecords" => unit
            .delete_records("alpha", &["missing".into(), "missing".into()])
            .await
            .unwrap(),
        "emptyBatch" => unit.delete_records("alpha", &[]).await.unwrap(),
        _ => panic!("unknown contract operation"),
    }
    writer.close().await.unwrap();
    let unchanged = (h.reopen)().await.unwrap();
    assert_eq!(
        tables(
            &unchanged
                .kv()
                .unwrap()
                .open(descriptor())
                .await
                .unwrap()
                .load_all()
                .await
                .unwrap()
                .tables["alpha"]
        ),
        json!({"saved":{"v":1}})
    );
    unchanged.close().await.unwrap();
}

/// The mutation/stamp portion of the source case; throwing toJSON preamble is
/// excluded because Value is already serialized data (see PORT_NOTES).
pub async fn commits_the_current_stamp_with_its_data(h: KvBackendContractHarness, operation: &str) {
    let unit = h.backend.kv().unwrap().open(descriptor()).await.unwrap();
    unit.put_record("alpha", "k", json!({"v":1})).await.unwrap();
    unit.put_record("beta", "untouched", json!({"saved":true}))
        .await
        .unwrap();
    unit.set_global(json!({"counter":1})).await.unwrap();
    let mut other = descriptor();
    other.name = "other_unit".into();
    h.backend
        .kv()
        .unwrap()
        .open(other.clone())
        .await
        .unwrap()
        .put_record("alpha", "saved", json!({"v":1}))
        .await
        .unwrap();
    h.backend.close().await.unwrap();
    let retry = (h.reopen)().await.unwrap();
    let mut current = descriptor();
    current.version = 4;
    current.compatible_versions = Some(vec![3]);
    let writable = retry.kv().unwrap().open(current.clone()).await.unwrap();
    writable.delete_record("alpha", "missing").await.unwrap();
    writable.delete_records("alpha", &[]).await.unwrap();
    match operation {
        "putRecord" => writable
            .put_record("alpha", "k", json!({"v":2}))
            .await
            .unwrap(),
        "deleteRecord" => writable.delete_record("alpha", "k").await.unwrap(),
        "deleteRecords" => writable
            .delete_records("alpha", &["k".into(), "missing".into(), "k".into()])
            .await
            .unwrap(),
        "setGlobal" => writable.set_global(json!({"counter":2})).await.unwrap(),
        _ => panic!("unknown contract operation"),
    }
    retry.close().await.unwrap();
    let restarted = (h.reopen)().await.unwrap();
    let error = restarted
        .kv()
        .unwrap()
        .open(descriptor())
        .await
        .err()
        .unwrap();
    assert_eq!(
        error
            .downcast_ref::<hoh_storage::StorageError>()
            .unwrap()
            .code(),
        "version-mismatch"
    );
    let saved = restarted
        .kv()
        .unwrap()
        .open(current)
        .await
        .unwrap()
        .load_all()
        .await
        .unwrap();
    assert_eq!(
        tables(&saved.tables["beta"]),
        json!({"untouched":{"saved":true}})
    );
    let expected = if operation == "deleteRecord" || operation == "deleteRecords" {
        json!({})
    } else {
        json!({"k":{"v":if operation=="putRecord" {2} else {1}}})
    };
    assert_eq!(tables(&saved.tables["alpha"]), expected);
    assert_eq!(
        saved.global,
        json!({"counter":if operation=="setGlobal" {2} else {1}})
    );
    assert_eq!(
        tables(
            &restarted
                .kv()
                .unwrap()
                .open(other)
                .await
                .unwrap()
                .load_all()
                .await
                .unwrap()
                .tables["alpha"]
        ),
        json!({"saved":{"v":1}})
    );
    restarted.close().await.unwrap();
}

pub async fn reads_historical_values_through(h: KvBackendContractHarness, write_v2: bool) {
    let mut v1 = descriptor();
    v1.version = 1;
    let mut v2 = descriptor();
    v2.version = 2;
    v2.compatible_versions = Some(vec![1]);
    let mut v3 = descriptor();
    v3.compatible_versions = Some(vec![if write_v2 { 2 } else { 1 }]);
    let original = h.backend.kv().unwrap().open(v1).await.unwrap();
    original
        .put_record("alpha", "untouched", json!({"label":"Keep"}))
        .await
        .unwrap();
    original
        .put_record("alpha", "edited", json!({"label":"Edit"}))
        .await
        .unwrap();
    h.backend.close().await.unwrap();
    if write_v2 {
        let second = (h.reopen)().await.unwrap();
        second
            .kv()
            .unwrap()
            .open(v2.clone())
            .await
            .unwrap()
            .put_record("alpha", "edited", json!({"title":"Edit"}))
            .await
            .unwrap();
        second.close().await.unwrap();
    }
    let third = (h.reopen)().await.unwrap();
    let latest = third.kv().unwrap().open(v3.clone()).await.unwrap();
    assert_eq!(
        tables(&latest.load_all().await.unwrap().tables["alpha"]),
        json!({"untouched":{"label":"Keep"},"edited":if write_v2 {json!({"title":"Edit"})} else {json!({"label":"Edit"})}})
    );
    latest
        .put_record("alpha", "edited", json!({"title":"Edit","addedInV3":true}))
        .await
        .unwrap();
    third.close().await.unwrap();
    let restarted = (h.reopen)().await.unwrap();
    assert_eq!(
        restarted
            .kv()
            .unwrap()
            .open(v2)
            .await
            .err()
            .unwrap()
            .downcast_ref::<hoh_storage::StorageError>()
            .unwrap()
            .code(),
        "version-mismatch"
    );
    let snapshot = restarted
        .kv()
        .unwrap()
        .open(v3)
        .await
        .unwrap()
        .load_all()
        .await
        .unwrap();
    assert_eq!(
        snapshot.tables["alpha"]["untouched"],
        json!({"label":"Keep"})
    );
    assert_eq!(snapshot.tables["alpha"]["edited"]["addedInV3"], json!(true));
    restarted.close().await.unwrap();
}

pub async fn rejects_operations_after_unit_close_and_close_is_idempotent(
    h: KvBackendContractHarness,
) {
    let unit = h.backend.kv().unwrap().open(descriptor()).await.unwrap();
    unit.close().await.unwrap();
    unit.close().await.unwrap();
    let put = unit.put_record("alpha", "k", json!({})).await.unwrap_err();
    let load = unit.load_all().await.unwrap_err();
    let delete = unit
        .delete_records("alpha", &["k".into()])
        .await
        .unwrap_err();
    for error in [put, load, delete] {
        assert_eq!(
            error
                .downcast_ref::<hoh_storage::StorageError>()
                .unwrap()
                .code(),
            "closed"
        );
    }
    h.backend.close().await.unwrap();
    h.backend.close().await.unwrap();
}

pub async fn run_kv_backend_contract<F, Fut>(create: F)
where
    F: Fn() -> Fut,
    Fut: Future<Output = KvBackendContractHarness>,
{
    opens_a_missing_unit_as_empty_and_serves_load_all_immediately(create().await).await;
    round_trips_records_and_global_durably_across_reopen(create().await).await;
    put_record_overwrites_and_delete_record_is_idempotent(create().await).await;
    delete_records_removes_the_whole_batch_durably_and_is_idempotent(create().await).await;
    for version in [2, 4] {
        rejects_version_on_reopen_without_touching_the_data(create().await, version).await;
    }
    reads_an_accepted_older_version_without_changing_its_stamp_or_records(create().await).await;
    for operation in ["deleteRecord", "deleteRecords", "emptyBatch"] {
        keeps_the_older_stamp_for_a_no_op(create().await, operation).await;
    }
    for operation in ["putRecord", "deleteRecord", "deleteRecords", "setGlobal"] {
        commits_the_current_stamp_with_its_data(create().await, operation).await;
    }
    for write_v2 in [false, true] {
        reads_historical_values_through(create().await, write_v2).await;
    }
    rejects_operations_after_unit_close_and_close_is_idempotent(create().await).await;
}
