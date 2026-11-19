use std::sync::Arc;

#[cfg(unix)]
use std::{fs, os::unix::fs::PermissionsExt};

use hoh_error::BoxError;
use hoh_kernel::{Kernel, Plugin};
use hoh_storage::{
    KvLayout, KvUnit, KvUnitDescriptor, Storage, StorageBackend, StorageError, StoragePlugin,
};
use hoh_storage_sqlite::{
    Config, JournalMode, STORAGE_SQLITE_SCHEMA_VERSION, SqliteStorageBackend, StorageSqlite,
    StorageSqlitePlugin,
};
use rusqlite::{Connection, ErrorCode, OptionalExtension};
use schemars::schema_for;
use serde_json::{Value, json};
use tempfile::TempDir;

fn descriptor() -> KvUnitDescriptor {
    KvUnitDescriptor {
        name: "specimen".into(),
        version: 1,
        tables: vec!["records".into()],
        has_global: true,
        layout: None,
        compatible_versions: None,
    }
}

fn backend_at(path: &str) -> Arc<SqliteStorageBackend> {
    Arc::new(SqliteStorageBackend::new(Config {
        path: path.into(),
        journal_mode: JournalMode::Wal,
    }))
}

fn file_path(directory: &TempDir) -> String {
    directory.path().join("storage.db").to_str().unwrap().into()
}

fn storage_error<'a>(error: &'a (dyn std::error::Error + 'static)) -> &'a StorageError {
    error.downcast_ref::<StorageError>().unwrap()
}

#[test]
fn config_schema_requires_path_and_defaults_to_the_four_journal_modes() {
    let schema = serde_json::to_value(schema_for!(Config)).unwrap();
    assert_eq!(schema["required"], json!(["path"]));
    assert_eq!(schema["properties"]["journalMode"]["default"], "wal");
    assert_eq!(
        schema["$defs"]["JournalMode"]["enum"],
        json!(["wal", "delete", "truncate", "persist"])
    );
}

async fn mutate(unit: &Arc<dyn KvUnit>, operation: &str) -> Result<(), BoxError> {
    match operation {
        "putRecord" => unit.put_record("records", "new", json!({"n":2})).await,
        "deleteRecords" => {
            unit.delete_records(
                "records",
                &["saved".into(), "missing".into(), "saved".into()],
            )
            .await
        }
        _ => panic!("unknown operation"),
    }
}

#[tokio::test]
async fn opens_memory_round_trips_values_and_uses_javascript_key_order() {
    let backend = backend_at(":memory:");
    let unit = backend.kv().unwrap().open(descriptor()).await.unwrap();
    for key in ["10", "b", "2", "01", "__proto__", "constructor"] {
        unit.put_record("records", key, json!({"key":key}))
            .await
            .unwrap();
    }
    unit.set_global(json!({"g":1})).await.unwrap();
    let snapshot = unit.load_all().await.unwrap();
    assert_eq!(
        snapshot.tables["records"]
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["2", "10", "b", "01", "__proto__", "constructor"]
    );
    assert_eq!(snapshot.global, json!({"g":1}));
    backend.close().await.unwrap();
}

async fn restores_batch_after(action: &str) {
    let directory = tempfile::tempdir().unwrap();
    let path = file_path(&directory);
    let backend = backend_at(&path);
    let unit = backend.kv().unwrap().open(descriptor()).await.unwrap();
    unit.put_record("records", "a", json!({"n":1}))
        .await
        .unwrap();
    unit.put_record("records", "b", json!({"n":2}))
        .await
        .unwrap();
    let observer = Connection::open(&path).unwrap();
    observer
        .execute_batch(&format!(
            "CREATE TRIGGER block_b BEFORE DELETE ON \"u_specimen_records\" WHEN old.key = 'b' BEGIN SELECT RAISE({action}, 'blocked'); END"
        ))
        .unwrap();
    let error = unit
        .delete_records("records", &["a".into(), "b".into()])
        .await
        .unwrap_err();
    assert!(error.to_string().contains("blocked"));
    assert_eq!(
        serde_json::to_value(&unit.load_all().await.unwrap().tables["records"]).unwrap(),
        json!({"a":{"n":1},"b":{"n":2}})
    );
    observer.execute_batch("DROP TRIGGER block_b").unwrap();
    unit.delete_records("records", &["a".into(), "b".into()])
        .await
        .unwrap();
    assert!(unit.load_all().await.unwrap().tables["records"].is_empty());
    backend.close().await.unwrap();
}

#[tokio::test]
async fn restores_a_batch_delete_after_sqlite_abort() {
    restores_batch_after("ABORT").await;
}

#[tokio::test]
async fn restores_a_batch_delete_after_sqlite_rollback() {
    restores_batch_after("ROLLBACK").await;
}

async fn busy_begin_preserves(layout: KvLayout, operation: &str) {
    let directory = tempfile::tempdir().unwrap();
    let path = file_path(&directory);
    let first = backend_at(&path);
    let unit = first.kv().unwrap().open(descriptor()).await.unwrap();
    unit.put_record("records", "saved", json!({"n":1}))
        .await
        .unwrap();
    first.close().await.unwrap();

    let observer = Connection::open(&path).unwrap();
    let backend = backend_at(&path);
    let mut current = descriptor();
    current.version = 2;
    current.compatible_versions = Some(vec![1]);
    current.layout = Some(layout);
    let unit = backend.kv().unwrap().open(current).await.unwrap();
    unit.delete_record("records", "missing").await.unwrap();
    observer.execute_batch("BEGIN IMMEDIATE").unwrap();
    let error = mutate(&unit, operation).await.unwrap_err();
    let sqlite = error.downcast_ref::<rusqlite::Error>().unwrap();
    assert!(matches!(
        sqlite,
        rusqlite::Error::SqliteFailure(code, _) if code.code == ErrorCode::DatabaseBusy
    ));
    let version: i64 = observer
        .query_row(
            "SELECT version FROM units WHERE name = 'specimen'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, 1);
    assert_eq!(
        serde_json::to_value(&unit.load_all().await.unwrap().tables["records"]).unwrap(),
        json!({"saved":{"n":1}})
    );
    observer.execute_batch("ROLLBACK").unwrap();
    mutate(&unit, operation).await.unwrap();
    let version: i64 = observer
        .query_row(
            "SELECT version FROM units WHERE name = 'specimen'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, 2);
    assert_eq!(
        serde_json::to_value(&unit.load_all().await.unwrap().tables["records"]).unwrap(),
        if operation == "putRecord" {
            json!({"saved":{"n":1},"new":{"n":2}})
        } else {
            json!({})
        }
    );
    backend.close().await.unwrap();
    let old_reader = backend_at(&path);
    let mut old = descriptor();
    old.layout = Some(layout);
    let error = old_reader.kv().unwrap().open(old).await.err().unwrap();
    assert_eq!(storage_error(error.as_ref()).code(), "version-mismatch");
    old_reader.close().await.unwrap();
}

macro_rules! busy_case {
    ($name:ident, $layout:expr, $operation:literal) => {
        #[tokio::test]
        async fn $name() {
            busy_begin_preserves($layout, $operation).await;
        }
    };
}

busy_case!(
    preserves_the_unit_stamp_when_begin_is_busy_with_layout_single_and_put_record_then_retries,
    KvLayout::Single,
    "putRecord"
);
busy_case!(
    preserves_the_unit_stamp_when_begin_is_busy_with_layout_single_and_delete_records_then_retries,
    KvLayout::Single,
    "deleteRecords"
);
busy_case!(
    preserves_the_unit_stamp_when_begin_is_busy_with_layout_per_record_and_put_record_then_retries,
    KvLayout::PerRecord,
    "putRecord"
);
busy_case!(
    preserves_the_unit_stamp_when_begin_is_busy_with_layout_per_record_and_delete_records_then_retries,
    KvLayout::PerRecord,
    "deleteRecords"
);

#[tokio::test]
async fn leaves_a_batch_unapplied_when_begin_fails_then_retries_on_the_same_unit() {
    let directory = tempfile::tempdir().unwrap();
    let path = file_path(&directory);
    let backend = backend_at(&path);
    let unit = backend.kv().unwrap().open(descriptor()).await.unwrap();
    unit.put_record("records", "a", json!({"n":1}))
        .await
        .unwrap();
    let observer = Connection::open(&path).unwrap();
    observer.execute_batch("BEGIN IMMEDIATE").unwrap();
    let error = unit
        .delete_records("records", &["a".into()])
        .await
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<rusqlite::Error>(),
        Some(rusqlite::Error::SqliteFailure(code, _)) if code.code == ErrorCode::DatabaseBusy
    ));
    observer.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        serde_json::to_value(&unit.load_all().await.unwrap().tables["records"]).unwrap(),
        json!({"a":{"n":1}})
    );
    unit.delete_records("records", &["a".into()]).await.unwrap();
    assert!(unit.load_all().await.unwrap().tables["records"].is_empty());
    backend.close().await.unwrap();
}

async fn failed_first_version_update(action: &str, operation: &str) {
    let directory = tempfile::tempdir().unwrap();
    let path = file_path(&directory);
    let first = backend_at(&path);
    let unit = first.kv().unwrap().open(descriptor()).await.unwrap();
    unit.put_record("records", "saved", json!({"n":1}))
        .await
        .unwrap();
    first.close().await.unwrap();
    let observer = Connection::open(&path).unwrap();
    let physical_version: i64 = observer
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    observer
        .execute_batch(&format!(
            "CREATE TRIGGER reject_unit_version BEFORE UPDATE ON units BEGIN SELECT RAISE({action}, 'version write failed'); END"
        ))
        .unwrap();
    let backend = backend_at(&path);
    let mut current = descriptor();
    current.version = 2;
    current.compatible_versions = Some(vec![1]);
    let unit = backend.kv().unwrap().open(current).await.unwrap();
    assert!(
        mutate(&unit, operation)
            .await
            .unwrap_err()
            .to_string()
            .contains("version write failed")
    );
    assert_eq!(
        serde_json::to_value(&unit.load_all().await.unwrap().tables["records"]).unwrap(),
        json!({"saved":{"n":1}})
    );
    let version: i64 = observer
        .query_row(
            "SELECT version FROM units WHERE name = 'specimen'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, 1);
    observer
        .execute_batch("DROP TRIGGER reject_unit_version")
        .unwrap();
    mutate(&unit, operation).await.unwrap();
    let version: i64 = observer
        .query_row(
            "SELECT version FROM units WHERE name = 'specimen'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, 2);
    assert_eq!(
        observer
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        physical_version
    );
    assert_eq!(
        serde_json::to_value(&unit.load_all().await.unwrap().tables["records"]).unwrap(),
        if operation == "putRecord" {
            json!({"saved":{"n":1},"new":{"n":2}})
        } else {
            json!({})
        }
    );
    backend.close().await.unwrap();
}

macro_rules! version_update_case {
    ($name:ident, $action:literal, $operation:literal) => {
        #[tokio::test]
        async fn $name() {
            failed_first_version_update($action, $operation).await;
        }
    };
}

version_update_case!(
    preserves_data_and_stamp_on_abort_during_the_first_put_record_version_update_then_retries,
    "ABORT",
    "putRecord"
);
version_update_case!(
    preserves_data_and_stamp_on_abort_during_the_first_delete_records_version_update_then_retries,
    "ABORT",
    "deleteRecords"
);
version_update_case!(
    preserves_data_and_stamp_on_rollback_during_the_first_put_record_version_update_then_retries,
    "ROLLBACK",
    "putRecord"
);
version_update_case!(preserves_data_and_stamp_on_rollback_during_the_first_delete_records_version_update_then_retries, "ROLLBACK", "deleteRecords");

#[tokio::test]
async fn materializes_strict_record_tables_and_stamps_the_schema_version() {
    let directory = tempfile::tempdir().unwrap();
    let path = file_path(&directory);
    let backend = backend_at(&path);
    backend.kv().unwrap().open(descriptor()).await.unwrap();
    backend.close().await.unwrap();
    let connection = Connection::open(&path).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, STORAGE_SQLITE_SCHEMA_VERSION);
    let sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'u_specimen_records'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(sql.contains("STRICT"));
}

#[tokio::test]
async fn rejects_a_mismatched_database_schema_version() {
    let directory = tempfile::tempdir().unwrap();
    let path = file_path(&directory);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("PRAGMA user_version = 999")
        .unwrap();
    drop(connection);
    let rejected = backend_at(&path)
        .kv()
        .unwrap()
        .open(descriptor())
        .await
        .err()
        .unwrap();
    assert_eq!(storage_error(rejected.as_ref()).code(), "version-mismatch");
}

#[tokio::test]
async fn rejects_invalid_unit_and_table_names_before_touching_the_medium() {
    let backend = backend_at(":memory:");
    let mut bad = descriptor();
    bad.name = "Bad-Name".into();
    assert_eq!(
        backend
            .kv()
            .unwrap()
            .open(bad)
            .await
            .err()
            .unwrap()
            .to_string(),
        "kv unit name 'Bad-Name' violates /^[a-z][a-z0-9_]*$/"
    );
    let mut bad = descriptor();
    bad.tables = vec!["ok".into(), "1bad".into()];
    assert_eq!(
        backend
            .kv()
            .unwrap()
            .open(bad)
            .await
            .err()
            .unwrap()
            .to_string(),
        "kv table name '1bad' in unit 'specimen' violates /^[a-z][a-z0-9_]*$/"
    );
    backend.close().await.unwrap();
}

#[tokio::test]
async fn rejects_a_second_open_of_the_same_unit_name() {
    let backend = backend_at(":memory:");
    backend.kv().unwrap().open(descriptor()).await.unwrap();
    assert!(
        backend
            .kv()
            .unwrap()
            .open(descriptor())
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("already open")
    );
    backend.close().await.unwrap();
}

#[tokio::test]
async fn allows_reopen_after_unit_close_and_rejects_open_on_a_closed_backend() {
    let backend = backend_at(":memory:");
    let first = backend.kv().unwrap().open(descriptor()).await.unwrap();
    first.close().await.unwrap();
    let reopened = backend.kv().unwrap().open(descriptor()).await.unwrap();
    reopened.put_record("records", "k", json!(1)).await.unwrap();
    backend.close().await.unwrap();
    let error = backend
        .kv()
        .unwrap()
        .open(descriptor())
        .await
        .err()
        .unwrap();
    assert_eq!(storage_error(error.as_ref()).code(), "closed");
}

#[tokio::test]
async fn round_trips_prototype_polluting_keys_as_own_records() {
    let backend = backend_at(":memory:");
    let unit = backend.kv().unwrap().open(descriptor()).await.unwrap();
    unit.put_record("records", "__proto__", json!({"evil":true}))
        .await
        .unwrap();
    unit.put_record("records", "constructor", json!({"n":1}))
        .await
        .unwrap();
    let records = &unit.load_all().await.unwrap().tables["records"];
    assert_eq!(records["__proto__"], json!({"evil":true}));
    assert_eq!(records["constructor"], json!({"n":1}));
    backend.close().await.unwrap();
}

#[tokio::test]
async fn failed_schema_materialization_remains_unstamped_and_can_be_repaired() {
    let directory = tempfile::tempdir().unwrap();
    let path = file_path(&directory);
    let setup = Connection::open(&path).unwrap();
    setup
        .execute_batch("CREATE TABLE squatter (x TEXT); CREATE INDEX unit_globals ON squatter(x)")
        .unwrap();
    drop(setup);
    let broken = backend_at(&path);
    assert!(broken.kv().unwrap().open(descriptor()).await.is_err());
    broken.close().await.unwrap();
    let repair = Connection::open(&path).unwrap();
    let version: i64 = repair
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 0);
    let units: String = repair
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'units'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(units, "units");
    repair.execute_batch("DROP INDEX unit_globals").unwrap();
    assert!(
        repair
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'unit_globals'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .unwrap()
            .is_none()
    );
    drop(repair);
    let repaired = backend_at(&path);
    let unit = repaired.kv().unwrap().open(descriptor()).await.unwrap();
    unit.put_record("records", "k", json!({"n":1}))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&unit.load_all().await.unwrap().tables["records"]).unwrap(),
        json!({"k":{"n":1}})
    );
    repaired.close().await.unwrap();
}

async fn rejects_malformed_json(global: bool) {
    let directory = tempfile::tempdir().unwrap();
    let path = file_path(&directory);
    let backend = backend_at(&path);
    let unit = backend.kv().unwrap().open(descriptor()).await.unwrap();
    unit.put_record("records", "good", json!({"n":1}))
        .await
        .unwrap();
    unit.set_global(json!({"g":1})).await.unwrap();
    backend.close().await.unwrap();
    let connection = Connection::open(&path).unwrap();
    if global {
        connection
            .execute(
                "UPDATE unit_globals SET value = '][' WHERE unit = 'specimen'",
                [],
            )
            .unwrap();
    } else {
        connection
            .execute(
                "UPDATE u_specimen_records SET value = '{not json' WHERE key = 'good'",
                [],
            )
            .unwrap();
    }
    drop(connection);
    let reopened = backend_at(&path);
    let unit = reopened.kv().unwrap().open(descriptor()).await.unwrap();
    let error = unit.load_all().await.unwrap_err();
    assert_eq!(storage_error(error.as_ref()).code(), "malformed-medium");
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn rejects_unparsable_stored_json_with_malformed_medium() {
    rejects_malformed_json(false).await;
}

#[tokio::test]
async fn rejects_an_unparsable_global_slot_with_malformed_medium() {
    rejects_malformed_json(true).await;
}

#[tokio::test]
async fn rejects_global_and_table_operations_not_declared_by_descriptor() {
    let backend = backend_at(":memory:");
    let mut local = descriptor();
    local.has_global = false;
    let unit = backend.kv().unwrap().open(local).await.unwrap();
    assert!(
        unit.set_global(json!({"g":1}))
            .await
            .unwrap_err()
            .to_string()
            .contains("declared no global slot")
    );
    assert!(
        unit.put_record("undeclared", "k", json!(1))
            .await
            .unwrap_err()
            .to_string()
            .contains("declared no table")
    );
    assert_eq!(unit.load_all().await.unwrap().global, Value::Null);
    backend.close().await.unwrap();
}

#[tokio::test]
async fn closed_guard_precedes_table_and_global_slot_validation() {
    let backend = backend_at(":memory:");
    let mut local = descriptor();
    local.has_global = false;
    let unit = backend.kv().unwrap().open(local).await.unwrap();
    unit.close().await.unwrap();
    for error in [
        unit.put_record("undeclared", "k", json!(1))
            .await
            .unwrap_err(),
        unit.set_global(json!(1)).await.unwrap_err(),
    ] {
        assert_eq!(storage_error(error.as_ref()).code(), "closed");
    }
    backend.close().await.unwrap();
}

#[tokio::test]
async fn close_drains_a_completed_failed_open_and_is_idempotent() {
    let directory = tempfile::tempdir().unwrap();
    let path = file_path(&directory);
    let first = backend_at(&path);
    first
        .kv()
        .unwrap()
        .open(descriptor())
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    first.close().await.unwrap();
    let backend = backend_at(&path);
    let mut wrong = descriptor();
    wrong.version = 99;
    let pending = backend.kv().unwrap().open(wrong);
    backend.close().await.unwrap();
    let error = pending.await.err().unwrap();
    assert_eq!(storage_error(error.as_ref()).code(), "version-mismatch");
    backend.close().await.unwrap();
}

#[tokio::test]
#[cfg(unix)]
async fn creates_owner_only_paths_and_preserves_an_existing_file_mode() {
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("private");
    let path = parent.join("storage.db");
    let backend = backend_at(path.to_str().unwrap());
    backend.kv().unwrap().open(descriptor()).await.unwrap();
    backend.close().await.unwrap();
    assert_eq!(
        fs::metadata(&parent).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let existing = root.path().join("existing.db");
    fs::write(&existing, []).unwrap();
    fs::set_permissions(&existing, fs::Permissions::from_mode(0o644)).unwrap();
    let backend = backend_at(existing.to_str().unwrap());
    backend.kv().unwrap().open(descriptor()).await.unwrap();
    backend.close().await.unwrap();
    assert_eq!(
        fs::metadata(existing).unwrap().permissions().mode() & 0o777,
        0o644
    );
}

#[tokio::test]
async fn rejects_a_path_containing_a_nul_before_sqlite_open() {
    let backend = backend_at("bad\0path");
    let error = backend
        .kv()
        .unwrap()
        .open(descriptor())
        .await
        .err()
        .unwrap();
    assert!(error.to_string().to_ascii_lowercase().contains("nul"));
    backend.close().await.unwrap();
}

#[tokio::test]
#[cfg(unix)]
async fn propagates_filesystem_errors_other_than_an_existing_database_file() {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o500)).unwrap();
    let path = file_path(&directory);
    let backend = backend_at(&path);
    let error = backend
        .kv()
        .unwrap()
        .open(descriptor())
        .await
        .err()
        .unwrap();
    assert_eq!(
        error
            .downcast_ref::<hoh_error::SharedError>()
            .and_then(|error| error.downcast_ref::<std::io::Error>())
            .map(std::io::Error::kind),
        Some(std::io::ErrorKind::PermissionDenied)
    );
    backend.close().await.unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
}

#[tokio::test]
async fn registers_backend_and_typed_service_then_closes_on_plugin_dispose() {
    assert_eq!(StorageSqlitePlugin::NAME, "storage-sqlite");
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<StoragePlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let fiber = ctx
        .plugin::<StorageSqlitePlugin>(json!({"path":":memory:"}))
        .unwrap();
    fiber.settled().await.unwrap();
    let registered = ctx.get::<Storage>().unwrap().backend.get("sqlite").unwrap();
    let provided = ctx.get::<StorageSqlite>().unwrap();
    assert!(std::ptr::eq(
        Arc::as_ptr(&registered).cast::<()>(),
        (&*provided as *const SqliteStorageBackend).cast::<()>(),
    ));
    let unit = registered.kv().unwrap().open(descriptor()).await.unwrap();
    unit.put_record("records", "k", json!({"n":1}))
        .await
        .unwrap();
    fiber.dispose().await;
    assert!(ctx.get::<Storage>().unwrap().backend.names().is_empty());
    assert!(ctx.get::<StorageSqlite>().is_none());
    let error = registered
        .kv()
        .unwrap()
        .open(descriptor())
        .await
        .err()
        .unwrap();
    assert_eq!(storage_error(error.as_ref()).code(), "closed");
    kernel.shutdown().await;
}
