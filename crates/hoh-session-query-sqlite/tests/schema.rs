use hoh_session_query_sqlite::{
    JournalMode, SESSION_QUERY_SQLITE_APPLICATION_ID, SESSION_QUERY_SQLITE_SCHEMA_VERSION,
    schema::open_search_database,
};
use rusqlite::Connection;

#[tokio::test]
async fn initializes_application_schema_and_fts5() {
    let db = open_search_database(":memory:", JournalMode::Wal)
        .await
        .unwrap();
    assert_eq!(
        db.query_row("PRAGMA application_id", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        SESSION_QUERY_SQLITE_APPLICATION_ID
    );
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        SESSION_QUERY_SQLITE_SCHEMA_VERSION
    );
    db.execute("INSERT INTO persisted_docs (text, session_id, seq, type, time, surface, codepoint_length) VALUES ('café AI', 's', 0, 'user/message', 1, 'current', 7)", []).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM persisted_docs WHERE persisted_docs MATCH '\"CAFE\"'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn refuses_foreign_and_unrecognized_databases() {
    let dir = tempfile::tempdir().unwrap();
    let foreign = dir.path().join("foreign.db");
    let db = Connection::open(&foreign).unwrap();
    db.execute_batch("PRAGMA application_id = 7; CREATE TABLE foreign_data (id INTEGER);")
        .unwrap();
    drop(db);
    assert_eq!(
        open_search_database(foreign.to_str().unwrap(), JournalMode::Wal)
            .await
            .unwrap_err()
            .to_string(),
        format!(
            "session-search database at \"{}\" belongs to another application",
            foreign.display()
        )
    );

    let unknown = dir.path().join("unknown.db");
    let db = Connection::open(&unknown).unwrap();
    db.execute_batch("CREATE TABLE canonical_data (id INTEGER);")
        .unwrap();
    drop(db);
    assert!(
        open_search_database(unknown.to_str().unwrap(), JournalMode::Wal)
            .await
            .unwrap_err()
            .to_string()
            .contains("not an empty or recognized derived index")
    );
}

#[tokio::test]
async fn invalid_path_reports_the_node_style_argument_code() {
    let path = "invalid\0search.db";
    let error = open_search_database(path, JournalMode::Wal)
        .await
        .unwrap_err();
    let error = hoh_error::HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(error.code(), "ERR_INVALID_ARG_VALUE");
    let cwd = std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let resolved =
        hoh_util_workspace_path::node_path::resolve(&cwd, &[path]).replace('\0', "\\x00");
    assert_eq!(
        error.message(),
        format!(
            "The argument 'path' must be a string, Uint8Array, or URL without null bytes. Received '{resolved}'"
        )
    );
}

#[tokio::test]
async fn invalid_path_uses_node_inspect_quote_and_control_escaping() {
    let error = open_search_database("/tmp/it's\0\n", JournalMode::Wal)
        .await
        .unwrap_err();
    let error = hoh_error::HarnessError::of(error.as_ref()).unwrap();
    assert_eq!(
        error.message(),
        "The argument 'path' must be a string, Uint8Array, or URL without null bytes. Received \"/tmp/it's\\x00\\n\""
    );
}

#[tokio::test]
async fn resets_recognized_incompatible_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    let db = Connection::open(&path).unwrap();
    db.execute_batch(&format!("PRAGMA application_id = {SESSION_QUERY_SQLITE_APPLICATION_ID}; PRAGMA user_version = 1; CREATE TABLE search_state (old INTEGER);")) .unwrap();
    drop(db);
    let db = open_search_database(path.to_str().unwrap(), JournalMode::Delete)
        .await
        .unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        SESSION_QUERY_SQLITE_SCHEMA_VERSION
    );
    assert_eq!(
        db.query_row(
            "SELECT global_generation FROM search_state WHERE singleton = 1",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[cfg(unix)]
#[tokio::test]
async fn creates_database_owner_only_without_changing_existing_parent() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let first = dir.path().join("nested");
    let parent = first.join("deeper");
    let path = parent.join("search.db");
    let db = open_search_database(path.to_str().unwrap(), JournalMode::Wal)
        .await
        .unwrap();
    db.execute_batch(
        "CREATE TABLE sidecar_probe (id INTEGER); INSERT INTO sidecar_probe VALUES (1);",
    )
    .unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(&first).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&parent).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(
        std::fs::metadata(format!("{}-wal", path.display()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(format!("{}-shm", path.display()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    drop(db);
}

#[cfg(unix)]
#[tokio::test]
async fn creates_persistent_journal_owner_only_and_preserves_existing_database_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("persist.db");
    let db = open_search_database(path.to_str().unwrap(), JournalMode::Persist)
        .await
        .unwrap();
    db.execute_batch(
        "CREATE TABLE journal_probe (id INTEGER); INSERT INTO journal_probe VALUES (1);",
    )
    .unwrap();
    assert_eq!(
        std::fs::metadata(format!("{}-journal", path.display()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    drop(db);

    let existing = dir.path().join("existing.db");
    std::fs::write(&existing, []).unwrap();
    std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o640)).unwrap();
    drop(
        open_search_database(existing.to_str().unwrap(), JournalMode::Delete)
            .await
            .unwrap(),
    );
    assert_eq!(
        std::fs::metadata(existing).unwrap().permissions().mode() & 0o777,
        0o640
    );
}
