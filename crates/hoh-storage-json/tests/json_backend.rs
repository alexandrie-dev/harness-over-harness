use hoh_error::{BoxError, HarnessError};
use hoh_kernel::Kernel;
use hoh_storage::{
    KvFacet, KvLayout, KvUnit, KvUnitDescriptor, Storage, StorageBackend, StorageError,
    StoragePlugin,
};
use hoh_storage_json::{JsonBackend, JsonStorageBackend, StorageJsonPlugin, atomic::NodeFsError};
use hoh_util_values::{js_json_parse, js_json_stringify, js_json_stringify_pretty};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc};
use tempfile::TempDir;
use tokio::fs;

#[path = "../../hoh-storage/tests/contract.rs"]
mod contract;
async fn create_harness() -> contract::KvBackendContractHarness {
    let root = Arc::new(tempfile::tempdir().unwrap());
    let backend = Arc::new(JsonStorageBackend::new(root.path().to_str().unwrap()));
    contract::KvBackendContractHarness {
        backend,
        reopen: Box::new(move || {
            let root = root.clone();
            Box::pin(async move {
                Ok(
                    Arc::new(JsonStorageBackend::new(root.path().to_str().unwrap()))
                        as Arc<dyn StorageBackend>,
                )
            })
        }),
    }
}
contract::run_kv_backend_contract!(contract, create_harness);

fn descriptor(per_record: bool) -> KvUnitDescriptor {
    KvUnitDescriptor {
        name: if per_record { "recs" } else { "shape" }.into(),
        version: if per_record { 2 } else { 1 },
        tables: vec!["t".into()],
        has_global: true,
        layout: per_record.then_some(KvLayout::PerRecord),
        compatible_versions: None,
    }
}
async fn setup(per_record: bool) -> (TempDir, JsonStorageBackend, Arc<dyn KvUnit>) {
    let root = tempfile::tempdir().unwrap();
    let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
    let unit = backend.open(descriptor(per_record)).await.unwrap();
    (root, backend, unit)
}
fn code(error: &BoxError, expected: &str) {
    let error = error
        .downcast_ref::<StorageError>()
        .expect("StorageError, not a HarnessError");
    assert_eq!(error.name(), "StorageError");
    assert_eq!(error.code(), expected);
}
fn caller(error: &BoxError, text: &str) {
    assert!(error.downcast_ref::<StorageError>().is_none());
    assert!(HarnessError::of(error.as_ref()).is_none());
    assert!(error.to_string().contains(text), "{error}");
}
async fn read(path: impl AsRef<Path>) -> Value {
    js_json_parse(&fs::read_to_string(path).await.unwrap()).unwrap()
}
async fn write(path: impl AsRef<Path>, value: Value) {
    fs::write(path, js_json_stringify(&value)).await.unwrap();
}
async fn snapshot(unit: &Arc<dyn KvUnit>) -> Value {
    serde_json::to_value(unit.load_all().await.unwrap()).unwrap()
}
fn record(root: &TempDir, key: &str) -> std::path::PathBuf {
    root.path().join("recs/t").join(format!("{key}.json"))
}
async fn names(path: impl AsRef<Path>) -> Vec<String> {
    let mut reader = fs::read_dir(path).await.unwrap();
    let mut names = Vec::new();
    while let Some(entry) = reader.next_entry().await.unwrap() {
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    names
}

#[tokio::test]
async fn publishes_a_human_readable_pretty_printed_file() {
    let (root, backend, unit) = setup(false).await;
    unit.put_record("t", "k", json!({"hello":"world"}))
        .await
        .unwrap();
    assert_eq!(
        fs::read_to_string(root.path().join("shape.json"))
            .await
            .unwrap(),
        format!(
            "{}\n",
            js_json_stringify_pretty(
                &json!({"unit":{"name":"shape","version":1},"global":null,"tables":{"t":{"k":{"hello":"world"}}}}),
                2
            )
        )
    );
    backend.close().await.unwrap();
}
#[tokio::test]
async fn defers_materialization_until_the_first_write() {
    let (root, backend, _unit) = setup(false).await;
    assert_eq!(
        fs::read(root.path().join("shape.json"))
            .await
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::NotFound
    );
    backend.close().await.unwrap();
}
#[tokio::test]
async fn rejects_a_malformed_medium() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("shape.json"), "not json at all")
        .await
        .unwrap();
    let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
    let error = backend.open(descriptor(false)).await.err().unwrap();
    code(&error, "malformed-medium");
    assert!(std::error::Error::source(error.as_ref()).is_some());
    backend.close().await.unwrap();
}
#[tokio::test]
async fn rejects_a_foreign_unit_header() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path().join("shape.json"),
        json!({"unit":{"name":"other","version":1},"global":null,"tables":{}}),
    )
    .await;
    let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
    code(
        &backend.open(descriptor(false)).await.err().unwrap(),
        "malformed-medium",
    );
    backend.close().await.unwrap();
}
#[tokio::test]
async fn rejects_double_open_of_one_unit_as_a_plain_caller_error() {
    let (_root, backend, _unit) = setup(false).await;
    caller(
        &backend.open(descriptor(false)).await.err().unwrap(),
        "already open",
    );
    backend.close().await.unwrap();
}
#[tokio::test]
async fn rolls_back_memory_when_a_publish_fails() {
    let (root, backend, unit) = setup(false).await;
    unit.put_record("t", "k", json!({"v":"committed"}))
        .await
        .unwrap();
    unit.put_record("t", "kept", json!({"v":"also committed"}))
        .await
        .unwrap();
    unit.set_global(json!({"g":"committed"})).await.unwrap();
    unit.close().await.unwrap();
    let mut desc = descriptor(false);
    desc.version = 2;
    desc.compatible_versions = Some(vec![1]);
    let unit = backend.open(desc).await.unwrap();
    let path = root.path().join("shape.json");
    let backup = root.path().join("shape.committed.json");
    fs::rename(&path, &backup).await.unwrap();
    fs::create_dir(&path).await.unwrap();
    assert!(
        unit.put_record("t", "k", json!({"v":"rejected"}))
            .await
            .is_err()
    );
    assert!(
        unit.put_record("t", "k2", json!({"v":"also rejected"}))
            .await
            .is_err()
    );
    assert!(unit.delete_record("t", "k").await.is_err());
    assert!(
        unit.delete_records(
            "t",
            &["k".into(), "kept".into(), "missing".into(), "k".into()]
        )
        .await
        .is_err()
    );
    assert!(unit.set_global(json!({"g":"rejected"})).await.is_err());
    fs::remove_dir(&path).await.unwrap();
    fs::rename(backup, &path).await.unwrap();
    assert_eq!(
        snapshot(&unit).await,
        json!({"tables":{"t":{"k":{"v":"committed"},"kept":{"v":"also committed"}}},"global":{"g":"committed"}})
    );
    assert_eq!(read(&path).await["unit"]["version"], 1);
    unit.put_record("t", "k3", json!({"v":"later"}))
        .await
        .unwrap();
    let text = fs::read_to_string(&path).await.unwrap();
    assert!(!text.contains("rejected"));
    assert_eq!(read(&path).await["unit"]["version"], 2);
    backend.close().await.unwrap();
}
#[tokio::test]
async fn rejects_undeclared_table_and_global_access_as_caller_errors() {
    let root = tempfile::tempdir().unwrap();
    let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
    let mut desc = descriptor(false);
    desc.has_global = false;
    let unit = backend.open(desc).await.unwrap();
    caller(
        &unit
            .put_record("undeclared", "k", json!({}))
            .await
            .unwrap_err(),
        "does not declare table",
    );
    caller(
        &unit.set_global(json!({})).await.unwrap_err(),
        "does not declare a global slot",
    );
    backend.close().await.unwrap();
}
#[tokio::test]
async fn rejects_invalid_unit_and_table_names() {
    let root = tempfile::tempdir().unwrap();
    let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
    let mut desc = descriptor(false);
    desc.name = "Bad-Name".into();
    code(&backend.open(desc).await.err().unwrap(), "malformed-medium");
    let mut desc = descriptor(false);
    desc.tables = vec!["ok".into(), "not ok".into()];
    code(&backend.open(desc).await.err().unwrap(), "malformed-medium");
    backend.close().await.unwrap();
    code(
        &backend.open(descriptor(false)).await.err().unwrap(),
        "closed",
    );
}
#[tokio::test]
async fn opens_a_file_missing_a_declared_table_as_that_table_empty() {
    let root = tempfile::tempdir().unwrap();
    write(root.path().join("contract_unit.json"),json!({"unit":{"name":"contract_unit","version":3},"global":null,"tables":{"alpha":{"k":1}}})).await;
    let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
    let unit = backend.open(contract::descriptor()).await.unwrap();
    assert_eq!(
        snapshot(&unit).await,
        json!({"tables":{"alpha":{"k":1},"beta":{}},"global":null})
    );
    backend.close().await.unwrap();
}
#[tokio::test]
async fn propagates_non_enoent_read_failures() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("shape.json"))
        .await
        .unwrap();
    let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
    let error = backend.open(descriptor(false)).await.err().unwrap();
    assert_eq!(
        error.downcast_ref::<NodeFsError>().unwrap().code,
        Some("EISDIR")
    );
    assert_eq!(
        error.to_string(),
        "EISDIR: illegal operation on a directory, read"
    );
    backend.close().await.unwrap();
}
#[tokio::test]
async fn rejects_malformed_table_shapes_and_foreign_versions_distinctly() {
    let root = tempfile::tempdir().unwrap();
    let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
    for (value, expected) in [
        (
            json!({"unit":{"name":"shape","version":1},"global":null,"tables":{"t":["not","an","object"]}}),
            "malformed-medium",
        ),
        (
            json!({"unit":{"name":"shape","version":9},"global":null,"tables":{}}),
            "version-mismatch",
        ),
        (
            json!({"unit":{"name":"shape","version":1},"global":null}),
            "malformed-medium",
        ),
        (json!("just a string"), "malformed-medium"),
        (json!([{"t":{"k":1}}]), "malformed-medium"),
    ] {
        write(root.path().join("shape.json"), value).await;
        code(
            &backend.open(descriptor(false)).await.err().unwrap(),
            expected,
        );
    }
    backend.close().await.unwrap();
}
#[tokio::test]
async fn rejects_a_whole_unit_document_whose_tables_is_an_array() {
    let root = tempfile::tempdir().unwrap();
    write(root.path().join("shape.json"),json!({"unit":{"name":"shape","version":1},"global":null,"tables":[{"t":{"k":{"hello":"world"}}}]})).await;
    let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
    code(
        &backend.open(descriptor(false)).await.err().unwrap(),
        "malformed-medium",
    );
    assert_eq!(
        backend
            .open(descriptor(false))
            .await
            .err()
            .unwrap()
            .to_string(),
        "unit 'shape': tables is not an object"
    );
    backend.close().await.unwrap();
}
#[tokio::test]
async fn opens_a_whole_unit_document_whose_declared_tables_hold_records() {
    let root = tempfile::tempdir().unwrap();
    write(root.path().join("shape.json"),json!({"unit":{"name":"shape","version":1},"global":{"g":1},"tables":{"t":{"k":{"hello":"world"}}}})).await;
    let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
    let unit = backend.open(descriptor(false)).await.unwrap();
    assert_eq!(
        snapshot(&unit).await,
        json!({"tables":{"t":{"k":{"hello":"world"}}},"global":{"g":1}})
    );
    backend.close().await.unwrap();
}
#[tokio::test]
async fn registers_on_the_hub_via_apply_and_closes_on_dispose() {
    let root = tempfile::tempdir().unwrap();
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<StoragePlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let fiber = ctx
        .plugin::<StorageJsonPlugin>(json!({"root":root.path().to_str().unwrap()}))
        .unwrap();
    fiber.settled().await.unwrap();
    let hub = ctx.get::<Storage>().unwrap();
    let backend = hub.backend.get("json").unwrap();
    let provided = ctx.get::<JsonBackend>().unwrap();
    assert!(std::ptr::eq(
        Arc::as_ptr(&backend).cast::<()>(),
        (&*provided as *const JsonStorageBackend).cast::<()>(),
    ));
    let unit = backend.kv().unwrap().open(descriptor(false)).await.unwrap();
    unit.put_record("t", "k", json!({"v":1})).await.unwrap();
    fiber.dispose().await;
    assert!(hub.backend.get("json").is_err());
    assert!(ctx.get::<JsonBackend>().is_none());
    code(
        &unit.put_record("t", "x", json!({})).await.unwrap_err(),
        "closed",
    );
    kernel.shutdown().await;
}
#[tokio::test]
async fn close_drains_in_flight_writes_and_blocks_in_flight_opens() {
    let (root, backend, unit) = setup(false).await;
    let big = unit.put_record("t", "big", json!({"blob":"x".repeat(4*1024*1024)}));
    unit.close().await.unwrap();
    big.await.unwrap();
    assert!(
        read(root.path().join("shape.json")).await["tables"]["t"]
            .get("big")
            .is_some()
    );
    let other = JsonStorageBackend::new(root.path().to_str().unwrap());
    let opening = other.open(descriptor(false));
    let closing = other.close();
    code(&opening.await.err().unwrap(), "closed");
    closing.await.unwrap();
    backend.close().await.unwrap();
}

#[tokio::test]
async fn stores_one_version_stamped_document_per_record_and_defers_materialization() {
    let (root, backend, unit) = setup(true).await;
    assert_eq!(
        snapshot(&unit).await,
        json!({"tables":{"t":{}},"global":null})
    );
    assert!(names(root.path()).await.is_empty());
    unit.put_record("t", "k1", json!({"v":1})).await.unwrap();
    unit.put_record("t", "k2", json!({"v":2})).await.unwrap();
    unit.set_global(json!("G")).await.unwrap();
    assert_eq!(
        fs::read_to_string(record(&root, "k1")).await.unwrap(),
        "{\n  \"version\": 2,\n  \"record\": {\n    \"v\": 1\n  }\n}\n"
    );
    assert_eq!(
        names(root.path().join("recs/t")).await,
        ["k1.json", "k2.json"]
    );
    assert_eq!(
        read(root.path().join("recs/global.json")).await,
        json!({"version":2,"record":"G"})
    );
    assert_eq!(
        snapshot(&unit).await,
        json!({"tables":{"t":{"k1":{"v":1},"k2":{"v":2}}},"global":"G"})
    );
    backend.close().await.unwrap();
}
#[tokio::test]
async fn overwrites_and_deletes_one_document_at_a_time_and_persists_across_reopen() {
    let (_root, backend, unit) = setup(true).await;
    unit.put_record("t", "k", json!({"v":1})).await.unwrap();
    unit.put_record("t", "k", json!({"v":2})).await.unwrap();
    unit.delete_record("t", "missing").await.unwrap();
    unit.close().await.unwrap();
    let unit = backend.open(descriptor(true)).await.unwrap();
    assert_eq!(
        snapshot(&unit).await,
        json!({"tables":{"t":{"k":{"v":2}}},"global":null})
    );
    unit.delete_record("t", "k").await.unwrap();
    assert_eq!(
        snapshot(&unit).await,
        json!({"tables":{"t":{}},"global":null})
    );
    backend.close().await.unwrap();
}
#[tokio::test]
async fn deletes_a_batch_of_documents_as_one_staged_set() {
    let (root, backend, unit) = setup(true).await;
    for n in 1..=3 {
        unit.put_record("t", &format!("k{n}"), json!({"v":n}))
            .await
            .unwrap();
    }
    unit.delete_records(
        "t",
        &["k1".into(), "k3".into(), "missing".into(), "k1".into()],
    )
    .await
    .unwrap();
    assert_eq!(
        snapshot(&unit).await,
        json!({"tables":{"t":{"k2":{"v":2}}},"global":null})
    );
    assert_eq!(names(root.path().join("recs/t")).await, ["k2.json"]);
    backend.close().await.unwrap();
}
#[tokio::test]
async fn restores_a_batch_when_a_staged_rename_fails_and_rejects_unsafe_keys_first() {
    let (root, backend, unit) = setup(true).await;
    unit.put_record("t", "k1", json!({"v":1})).await.unwrap();
    unit.put_record("t", "k2", json!({"v":2})).await.unwrap();
    let blocked = record(&root, "k2").with_extension("json.deleting");
    fs::create_dir(&blocked).await.unwrap();
    fs::write(blocked.join("occupied"), "").await.unwrap();
    assert!(
        unit.delete_records("t", &["k1".into(), "k2".into()])
            .await
            .is_err()
    );
    assert_eq!(
        snapshot(&unit).await,
        json!({"tables":{"t":{"k1":{"v":1},"k2":{"v":2}}},"global":null})
    );
    fs::remove_dir_all(blocked).await.unwrap();
    caller(
        &unit
            .delete_records("t", &["ok".into(), "a/b".into()])
            .await
            .unwrap_err(),
        "not path-safe",
    );
    backend.close().await.unwrap();
}
#[tokio::test]
async fn rejects_unsafe_keys_and_undeclared_tables_and_enforces_the_closed_guard() {
    let (_root, backend, unit) = setup(true).await;
    caller(
        &unit.put_record("t", "a/b", json!({})).await.unwrap_err(),
        "not path-safe",
    );
    caller(
        &unit.delete_record("t", "..").await.unwrap_err(),
        "not path-safe",
    );
    caller(
        &unit.put_record("bogus", "k", json!({})).await.unwrap_err(),
        "does not declare table",
    );
    unit.close().await.unwrap();
    code(
        &unit.put_record("t", "k", json!({})).await.unwrap_err(),
        "closed",
    );
    code(&unit.delete_record("t", "k").await.unwrap_err(), "closed");
    code(&unit.set_global(json!("x")).await.unwrap_err(), "closed");
    code(&unit.load_all().await.unwrap_err(), "closed");
    backend.close().await.unwrap();
}
#[tokio::test]
async fn discards_foreign_documents_stale_malformed_non_object_and_unsafe_key_on_open() {
    let (root, backend, unit) = setup(true).await;
    unit.put_record("t", "good", json!({"v":1})).await.unwrap();
    unit.close().await.unwrap();
    write(
        record(&root, "stale"),
        json!({"version":1,"record":{"v":0}}),
    )
    .await;
    fs::write(record(&root, "broken"), "{oops").await.unwrap();
    write(record(&root, "scalar"), json!(5)).await;
    write(
        record(&root, "unsafe%2Fkey"),
        json!({"version":2,"record":{"v":0}}),
    )
    .await;
    fs::write(root.path().join("recs/t/not-json.txt"), "ignored")
        .await
        .unwrap();
    write(
        root.path().join("recs/global.json"),
        json!({"version":1,"record":"old"}),
    )
    .await;
    fs::create_dir(root.path().join("recs/stray-dir"))
        .await
        .unwrap();
    fs::write(root.path().join("recs/stray.txt"), "ignored")
        .await
        .unwrap();
    let unit = backend.open(descriptor(true)).await.unwrap();
    assert_eq!(
        snapshot(&unit).await,
        json!({"tables":{"t":{"good":{"v":1}}},"global":null})
    );
    backend.close().await.unwrap();
}
#[tokio::test]
async fn propagates_non_enoent_tree_read_failures_and_refuses_an_undeclared_global_slot() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("recs"), "not a directory")
        .await
        .unwrap();
    let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
    let unit = backend.open(descriptor(true)).await.unwrap();
    let error = unit.load_all().await.unwrap_err();
    assert_eq!(
        error.downcast_ref::<NodeFsError>().unwrap().code,
        Some("ENOTDIR")
    );
    assert_eq!(
        error.to_string(),
        format!(
            "ENOTDIR: not a directory, scandir '{}'",
            root.path().join("recs").display()
        )
    );
    unit.close().await.unwrap();
    let mut desc = descriptor(true);
    desc.name = "plain".into();
    desc.version = 1;
    desc.has_global = false;
    let unit = backend.open(desc).await.unwrap();
    caller(
        &unit.set_global(json!("x")).await.unwrap_err(),
        "does not declare a global slot",
    );
    backend.close().await.unwrap();
}
#[tokio::test]
async fn per_record_close_drains_in_flight_writes_and_an_unreadable_record_reads_as_absent() {
    let (root, backend, unit) = setup(true).await;
    let blob = "x".repeat(4 * 1024 * 1024);
    let big = unit.put_record("t", "big", json!({"blob":blob}));
    unit.close().await.unwrap();
    unit.close().await.unwrap();
    big.await.unwrap();
    assert_eq!(
        read(record(&root, "big")).await["record"],
        json!({"blob":blob})
    );
    backend.close().await.unwrap();
}
#[tokio::test]
async fn reads_an_unreadable_record_document_as_absent() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("recs/t/locked.json"))
        .await
        .unwrap();
    let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
    let unit = backend.open(descriptor(true)).await.unwrap();
    assert_eq!(
        snapshot(&unit).await,
        json!({"tables":{"t":{}},"global":null})
    );
    backend.close().await.unwrap();
}
#[tokio::test]
async fn bootstraps_an_empty_per_record_tree_from_a_legacy_whole_unit_file_and_preserves_it() {
    let root = tempfile::tempdir().unwrap();
    let legacy = js_json_stringify(
        &json!({"unit":{"name":"recs","version":2},"global":null,"tables":{"t":{"old1":{"v":1},"old2":{"v":2}},"undeclared":{"k":{"v":0}}}}),
    );
    let path = root.path().join("recs.json");
    fs::write(&path, &legacy).await.unwrap();
    let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
    let unit = backend.open(descriptor(true)).await.unwrap();
    let expected = json!({"tables":{"t":{"old1":{"v":1},"old2":{"v":2}}},"global":null});
    assert_eq!(snapshot(&unit).await, expected);
    assert_eq!(fs::read_to_string(&path).await.unwrap(), legacy);
    unit.close().await.unwrap();
    let unit = backend.open(descriptor(true)).await.unwrap();
    assert_eq!(snapshot(&unit).await, expected);
    backend.close().await.unwrap();
}
#[tokio::test]
async fn bootstraps_from_a_legacy_file_only_when_its_stored_version_is_accepted() {
    for compatible in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let legacy = js_json_stringify(
            &json!({"unit":{"name":"recs","version":3},"global":null,"tables":{"t":{"old":{"v":1}}}}),
        );
        fs::write(root.path().join("recs.json"), &legacy)
            .await
            .unwrap();
        let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
        let mut desc = descriptor(true);
        if compatible {
            desc.version = 4;
            desc.compatible_versions = Some(vec![3]);
        }
        let unit = backend.open(desc).await.unwrap();
        if compatible {
            assert_eq!(
                snapshot(&unit).await,
                json!({"tables":{"t":{"old":{"v":1}}},"global":null})
            );
            assert_eq!(
                read(record(&root, "old")).await,
                json!({"version":4,"record":{"v":1}})
            );
        } else {
            assert_eq!(
                snapshot(&unit).await,
                json!({"tables":{"t":{}},"global":null})
            );
            assert!(fs::read(record(&root, "old")).await.is_err());
        }
        assert_eq!(
            fs::read_to_string(root.path().join("recs.json"))
                .await
                .unwrap(),
            legacy
        );
        unit.close().await.unwrap();
        backend.close().await.unwrap();
    }
}
#[tokio::test]
async fn backup_record_moves_the_document_aside_reads_see_it_absent_and_a_write_recreates_it() {
    let (root, backend, unit) = setup(true).await;
    unit.put_record("t", "k", json!({"v":1})).await.unwrap();
    let moved = unit.backup_record("t", "k").unwrap().await.unwrap();
    let suffix = moved.rsplit_once("k.json.bak.").unwrap().1;
    assert_eq!(suffix.len(), 12);
    assert!(suffix.bytes().all(|b| b.is_ascii_digit()));
    assert_eq!(read(&moved).await, json!({"version":2,"record":{"v":1}}));
    assert!(fs::read(record(&root, "k")).await.is_err());
    assert_eq!(
        snapshot(&unit).await,
        json!({"tables":{"t":{}},"global":null})
    );
    unit.put_record("t", "k", json!({"v":2})).await.unwrap();
    assert_eq!(
        snapshot(&unit).await,
        json!({"tables":{"t":{"k":{"v":2}}},"global":null})
    );
    caller(
        &unit.backup_record("t", "a/b").unwrap().await.unwrap_err(),
        "not path-safe",
    );
    unit.close().await.unwrap();
    code(
        &unit.backup_record("t", "k").unwrap().await.unwrap_err(),
        "closed",
    );
    backend.close().await.unwrap();
}
#[tokio::test]
async fn reads_per_record_documents_stamped_with_a_declared_compat_version_and_stamps_writes_current()
 {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("recs/t"))
        .await
        .unwrap();
    let original = js_json_stringify(&json!({"version":1,"record":{"v":"old"}}));
    let untouched = js_json_stringify(&json!({"version":1,"record":{"v":"keep"}}));
    fs::write(record(&root, "oldrec"), &original).await.unwrap();
    fs::write(record(&root, "untouched"), &untouched)
        .await
        .unwrap();
    write(
        record(&root, "ancient"),
        json!({"version":0,"record":{"v":"no"}}),
    )
    .await;
    let global = js_json_stringify(&json!({"version":1,"record":"old global"}));
    let global_path = root.path().join("recs/global.json");
    fs::write(&global_path, &global).await.unwrap();
    let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
    let mut desc = descriptor(true);
    desc.compatible_versions = Some(vec![1]);
    let unit = backend.open(desc).await.unwrap();
    assert_eq!(
        snapshot(&unit).await,
        json!({"tables":{"t":{"oldrec":{"v":"old"},"untouched":{"v":"keep"}}},"global":"old global"})
    );
    unit.delete_record("t", "missing").await.unwrap();
    // Value cannot contain a throwing JS toJSON; a target-directory denial
    // exercises a rejected replacement while preserving the old bytes.
    let saved = record(&root, "oldrec").with_extension("saved");
    fs::rename(record(&root, "oldrec"), &saved).await.unwrap();
    fs::create_dir(record(&root, "oldrec")).await.unwrap();
    assert!(
        unit.put_record("t", "oldrec", json!({"v":"rejected"}))
            .await
            .is_err()
    );
    fs::remove_dir(record(&root, "oldrec")).await.unwrap();
    fs::rename(saved, record(&root, "oldrec")).await.unwrap();
    assert_eq!(
        fs::read_to_string(record(&root, "oldrec")).await.unwrap(),
        original
    );
    unit.delete_records("t", &["oldrec".into(), "missing".into(), "oldrec".into()])
        .await
        .unwrap();
    assert!(fs::read(record(&root, "oldrec")).await.is_err());
    unit.put_record("t", "oldrec", json!({"v":"new"}))
        .await
        .unwrap();
    assert_eq!(
        read(record(&root, "oldrec")).await,
        json!({"version":2,"record":{"v":"new"}})
    );
    assert_eq!(fs::read_to_string(&global_path).await.unwrap(), global);
    unit.set_global(json!("new global")).await.unwrap();
    assert_eq!(
        read(&global_path).await,
        json!({"version":2,"record":"new global"})
    );
    assert_eq!(
        fs::read_to_string(record(&root, "untouched"))
            .await
            .unwrap(),
        untouched
    );
    backend.close().await.unwrap();
}
#[tokio::test]
async fn ignores_the_legacy_whole_unit_file_when_any_new_document_path_exists() {
    let root = tempfile::tempdir().unwrap();
    let legacy = js_json_stringify(
        &json!({"unit":{"name":"recs","version":2},"global":null,"tables":{"t":{"old":{"v":1}}}}),
    );
    fs::write(root.path().join("recs.json"), &legacy)
        .await
        .unwrap();
    fs::create_dir_all(root.path().join("recs/t"))
        .await
        .unwrap();
    fs::write(record(&root, "broken"), "{oops").await.unwrap();
    let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
    let unit = backend.open(descriptor(true)).await.unwrap();
    assert_eq!(
        snapshot(&unit).await,
        json!({"tables":{"t":{}},"global":null})
    );
    assert!(fs::read(record(&root, "old")).await.is_err());
    assert_eq!(
        fs::read_to_string(root.path().join("recs.json"))
            .await
            .unwrap(),
        legacy
    );
    backend.close().await.unwrap();
}
#[tokio::test]
async fn leaves_a_foreign_shapeless_or_malformed_legacy_file_alone() {
    for text in [
        js_json_stringify(&json!({"unit":{"name":"other","version":3},"tables":{}})),
        js_json_stringify(&json!({"tables":{"t":{"k":{"v":1}}}})),
        "not json at all".into(),
        js_json_stringify(&json!({"unit":{"name":"recs","version":2},"tables":"not an object"})),
        js_json_stringify(&json!({"unit":{"name":"recs","version":2},"tables":null})),
    ] {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("recs.json"), &text)
            .await
            .unwrap();
        let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
        let unit = backend.open(descriptor(true)).await.unwrap();
        assert_eq!(
            snapshot(&unit).await,
            json!({"tables":{"t":{}},"global":null})
        );
        assert_eq!(
            fs::read_to_string(root.path().join("recs.json"))
                .await
                .unwrap(),
            text
        );
        backend.close().await.unwrap();
    }
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("recs.json")).await.unwrap();
    let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
    let unit = backend.open(descriptor(true)).await.unwrap();
    assert_eq!(
        unit.load_all()
            .await
            .unwrap_err()
            .downcast_ref::<NodeFsError>()
            .unwrap()
            .code,
        Some("EISDIR")
    );
    backend.close().await.unwrap();
}

#[tokio::test]
async fn preserves_javascript_key_order_and_number_bytes() {
    let (root, backend, unit) = setup(false).await;
    unit.put_record("t", "10", json!({"n":1e20,"zero":-0.0,"tiny":1e-7}))
        .await
        .unwrap();
    unit.put_record("t", "b", json!(true)).await.unwrap();
    unit.put_record("t", "2", json!(1.0)).await.unwrap();
    let snapshot = unit.load_all().await.unwrap();
    assert_eq!(
        snapshot.tables["t"]
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["2", "10", "b"]
    );
    let text = fs::read_to_string(root.path().join("shape.json"))
        .await
        .unwrap();
    assert!(text.contains("\"n\": 100000000000000000000"));
    assert!(text.contains("\"zero\": 0"));
    assert!(text.contains("\"tiny\": 1e-7"));
    assert!(text.find("\"2\": 1").unwrap() < text.find("\"10\": {").unwrap());
    assert!(text.find("\"10\": {").unwrap() < text.find("\"b\": true").unwrap());
    assert!(text.ends_with("}\n"));
    backend.close().await.unwrap();
}

#[tokio::test]
async fn per_record_readdir_has_node_byte_order() {
    let (_root, backend, unit) = setup(true).await;
    for key in ["z", "A", "a", "_", "10", "2"] {
        unit.put_record("t", key, json!(key)).await.unwrap();
    }
    let state = unit.load_all().await.unwrap();
    // readdir byte order is then exposed through an ordinary JS object.
    assert_eq!(
        state.tables["t"]
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["2", "10", "A", "_", "a", "z"]
    );
    backend.close().await.unwrap();
}

#[test]
fn format_keeps_null_records_and_rejects_bom_and_unsupported_stamps() {
    use hoh_storage_json::format::{parse, parse_record};
    assert_eq!(
        parse_record("{\"version\":2,\"record\":null}", &[2]),
        Some(Value::Null)
    );
    assert_eq!(parse_record("{\"version\":2}", &[2]), None);
    assert_eq!(
        parse_record("\u{feff}{\"version\":2,\"record\":1}", &[2]),
        None
    );
    assert_eq!(
        parse_record(
            "{\"version\":9007199254740994,\"record\":1}",
            &[9_007_199_254_740_994]
        ),
        None
    );
    assert_eq!(
        parse_record(
            "{\"version\":9007199254740994.0,\"record\":1}",
            &[9_007_199_254_740_994]
        ),
        None
    );
    assert_eq!(
        parse(
            "{\"unit\":{\"name\":\"shape\",\"version\":1.5},\"tables\":{}}",
            &descriptor(false)
        )
        .unwrap_err()
        .code(),
        "version-mismatch"
    );
}

#[tokio::test]
async fn global_path_suppresses_bootstrap_and_legacy_global_is_not_migrated() {
    for materialized in [false, true] {
        let root = tempfile::tempdir().unwrap();
        write(root.path().join("recs.json"),json!({"unit":{"name":"recs","version":2},"global":"legacy global","tables":{"t":{"old":1}}})).await;
        if materialized {
            fs::create_dir(root.path().join("recs")).await.unwrap();
            fs::write(root.path().join("recs/global.json"), "invalid")
                .await
                .unwrap();
        }
        let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
        let unit = backend.open(descriptor(true)).await.unwrap();
        let state = unit.load_all().await.unwrap();
        assert_eq!(state.global, Value::Null);
        assert_eq!(state.tables["t"].is_empty(), materialized);
        backend.close().await.unwrap();
    }
}

#[tokio::test]
async fn opening_slot_is_reserved_before_polling_and_failed_open_can_retry() {
    let root = tempfile::tempdir().unwrap();
    let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
    let opening = backend.open(descriptor(false));
    caller(
        &backend.open(descriptor(false)).await.err().unwrap(),
        "already open",
    );
    let unit = opening.await.unwrap();
    unit.close().await.unwrap();
    fs::write(root.path().join("shape.json"), "broken")
        .await
        .unwrap();
    assert!(backend.open(descriptor(false)).await.is_err());
    fs::remove_file(root.path().join("shape.json"))
        .await
        .unwrap();
    backend.open(descriptor(false)).await.unwrap();
    backend.close().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn uses_private_file_and_directory_modes_and_cleans_failed_publish_temps() {
    use std::os::unix::fs::PermissionsExt;
    let (root, backend, unit) = setup(true).await;
    unit.put_record("t", "k", json!(1)).await.unwrap();
    assert_eq!(
        fs::metadata(record(&root, "k"))
            .await
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    for path in ["recs", "recs/t"] {
        assert_eq!(
            fs::metadata(root.path().join(path))
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    fs::create_dir(record(&root, "blocked")).await.unwrap();
    assert!(unit.put_record("t", "blocked", json!(1)).await.is_err());
    assert_eq!(
        names(root.path().join("recs/t")).await,
        ["blocked.json", "k.json"]
    );
    backend.close().await.unwrap();
}

#[tokio::test]
async fn plugin_requires_root_and_storage_injection() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let invalid = ctx.plugin::<StorageJsonPlugin>(json!({})).unwrap();
    let root = tempfile::tempdir().unwrap();
    let fiber = ctx
        .plugin::<StorageJsonPlugin>(json!({"root":root.path().to_str().unwrap()}))
        .unwrap();
    assert_eq!(fiber.state(), hoh_kernel::FiberState::Pending);
    ctx.plugin::<StoragePlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let error = invalid.settled().await.expect_err("root is required");
    assert!(error.to_string().contains("root"));
    fiber.settled().await.unwrap();
    assert!(ctx.get::<JsonBackend>().is_some());
    kernel.shutdown().await;
}

#[tokio::test]
async fn public_openers_propagate_the_owners_close_error_without_replacing_it() {
    #[derive(Debug, thiserror::Error)]
    #[error("close sentinel")]
    struct Sentinel;
    for per_record in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let failure = hoh_error::SharedError::new(Sentinel);
        let captured = failure.clone();
        let callback: Arc<dyn Fn() -> Result<(), BoxError> + Send + Sync> =
            Arc::new(move || Err(captured.clone().into()));
        let unit = if per_record {
            hoh_storage_json::per_record_unit::open_per_record_unit(
                descriptor(true),
                root.path().to_str().unwrap(),
                callback,
            )
            .await
            .unwrap()
        } else {
            hoh_storage_json::single_unit::open_single_unit(
                descriptor(false),
                root.path().to_str().unwrap(),
                callback,
            )
            .await
            .unwrap()
        };
        let error = unit.close().await.unwrap_err();
        assert!(
            error
                .downcast_ref::<hoh_error::SharedError>()
                .unwrap()
                .ptr_eq(&failure)
        );
        unit.close().await.unwrap();
        code(&unit.load_all().await.unwrap_err(), "closed");
    }
}

#[tokio::test]
async fn empty_root_does_not_fall_back_to_the_working_directory() {
    let backend = JsonStorageBackend::new("");
    let error = backend
        .open(descriptor(false))
        .await
        .err()
        .expect("Node mkdir rejects an empty path");
    assert!(error.downcast_ref::<NodeFsError>().is_some());
    #[cfg(unix)]
    {
        assert_eq!(
            error.downcast_ref::<NodeFsError>().unwrap().code,
            Some("ENOENT")
        );
        assert_eq!(
            error.to_string(),
            "ENOENT: no such file or directory, mkdir ''"
        );
    }
    backend.close().await.unwrap();
}

#[tokio::test]
async fn ignores_crash_residue_without_removing_it_on_reopen() {
    for per_record in [false, true] {
        let (root, backend, unit) = setup(per_record).await;
        unit.put_record("t", "live", json!({"v":1})).await.unwrap();
        let expected = unit.load_all().await.unwrap();
        unit.close().await.unwrap();
        let dir = if per_record {
            root.path().join("recs/t")
        } else {
            root.path().to_owned()
        };
        let tmp = dir.join(".00000000-0000-4000-8000-000000000000.tmp");
        let deleting = dir.join("ghost.json.deleting");
        let residue = if per_record {
            json!({"version":2,"record":{"ghost":true}})
        } else {
            json!({"unit":{"name":"shape","version":1},"global":"ghost","tables":{"t":{"ghost":true}}})
        };
        let bytes = js_json_stringify(&residue).into_bytes();
        fs::write(&tmp, &bytes).await.unwrap();
        fs::write(&deleting, &bytes).await.unwrap();
        let reopened = backend.open(descriptor(per_record)).await.unwrap();
        assert_eq!(reopened.load_all().await.unwrap(), expected);
        assert_eq!(fs::read(&tmp).await.unwrap(), bytes);
        assert_eq!(fs::read(&deleting).await.unwrap(), bytes);
        backend.close().await.unwrap();
    }
}

#[tokio::test]
async fn partial_legacy_bootstrap_keeps_only_published_records_after_reopen() {
    let root = tempfile::tempdir().unwrap();
    // The second record's basename exceeds NAME_MAX. No declared table
    // directory exists yet, so the first load is eligible for bootstrap.
    let too_long = "x".repeat(300);
    let records: serde_json::Map<String, Value> = [
        ("first".into(), json!({"v":1})),
        (too_long, json!({"v":2})),
        ("third".into(), json!({"v":3})),
    ]
    .into_iter()
    .collect();
    let legacy = js_json_stringify(
        &json!({"unit":{"name":"recs","version":2},"global":null,"tables":{"t":records}}),
    );
    let path = root.path().join("recs.json");
    fs::write(&path, &legacy).await.unwrap();
    let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
    let unit = backend.open(descriptor(true)).await.unwrap();
    assert!(unit.load_all().await.is_err());
    assert_eq!(
        read(record(&root, "first")).await,
        json!({"version":2,"record":{"v":1}})
    );
    assert_eq!(names(root.path().join("recs/t")).await, ["first.json"]);
    assert_eq!(fs::read_to_string(&path).await.unwrap(), legacy);
    backend.close().await.unwrap();
    let backend = JsonStorageBackend::new(root.path().to_str().unwrap());
    let unit = backend.open(descriptor(true)).await.unwrap();
    // Source behavior: the table directory created by record 1 now suppresses
    // bootstrap for the entire unit, so neither record 2 nor record 3 is retried.
    let expected = json!({"tables":{"t":{"first":{"v":1}}},"global":null});
    assert_eq!(snapshot(&unit).await, expected);
    assert_eq!(snapshot(&unit).await, expected);
    assert_eq!(fs::read_to_string(&path).await.unwrap(), legacy);
    backend.close().await.unwrap();
}

#[tokio::test]
async fn failed_batch_publish_reinserts_removed_keys_in_removal_order() {
    let (root, backend, unit) = setup(false).await;
    for key in ["one", "two", "three", "four"] {
        unit.put_record("t", key, json!(key)).await.unwrap();
    }
    let path = root.path().join("shape.json");
    let backup = root.path().join("committed.json");
    let before = fs::read(&path).await.unwrap();
    fs::rename(&path, &backup).await.unwrap();
    fs::create_dir(&path).await.unwrap();
    assert!(
        unit.delete_records(
            "t",
            &[
                "three".into(),
                "one".into(),
                "three".into(),
                "missing".into()
            ]
        )
        .await
        .is_err()
    );
    let state = unit.load_all().await.unwrap();
    assert_eq!(
        state.tables["t"]
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["two", "four", "three", "one"]
    );
    for key in ["one", "two", "three", "four"] {
        assert_eq!(state.tables["t"][key], json!(key));
    }
    fs::remove_dir(&path).await.unwrap();
    fs::rename(backup, &path).await.unwrap();
    assert_eq!(fs::read(&path).await.unwrap(), before);
    unit.put_record("t", "after", json!("after")).await.unwrap();
    let document = read(&path).await;
    assert_eq!(
        document["tables"]["t"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["two", "four", "three", "one", "after"]
    );
    backend.close().await.unwrap();
}
