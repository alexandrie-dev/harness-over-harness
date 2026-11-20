use hoh_error::BoxError;
use rusqlite::Connection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const SESSION_QUERY_SQLITE_SCHEMA_VERSION: i64 = 8;
pub const SESSION_QUERY_SQLITE_APPLICATION_ID: i64 = 0x4453_4851;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum JournalMode {
    #[default]
    Wal,
    Delete,
    Truncate,
    Persist,
}

impl JournalMode {
    fn sql(self) -> &'static str {
        match self {
            Self::Wal => "WAL",
            Self::Delete => "DELETE",
            Self::Truncate => "TRUNCATE",
            Self::Persist => "PERSIST",
        }
    }
}

const DERIVED_USER_TABLES: &[&str] = &[
    "search_state",
    "persisted_sessions",
    "persisted_docs",
    "persisted_docs_data",
    "persisted_docs_idx",
    "persisted_docs_content",
    "persisted_docs_docsize",
    "persisted_docs_config",
];

pub async fn open_search_database(
    path: &str,
    journal_mode: JournalMode,
) -> Result<Connection, BoxError> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || open_search_database_blocking(&path, journal_mode))
        .await
        .map_err(|cause| Box::new(cause) as BoxError)?
}

pub(crate) fn open_search_database_blocking(
    path: &str,
    journal_mode: JournalMode,
) -> Result<Connection, BoxError> {
    let actual = if path == ":memory:" {
        path.to_owned()
    } else {
        let cwd = std::env::current_dir()?.to_string_lossy().into_owned();
        hoh_util_workspace_path::node_path::resolve(&cwd, &[path])
    };
    if actual != ":memory:" {
        let path = Path::new(&actual);
        if let Some(parent) = path.parent() {
            create_owner_directory(parent)?;
        }
        create_owner_file(path)?;
    }
    let db = Connection::open(&actual)?;
    initialize(&db, &actual, journal_mode)?;
    Ok(db)
}

fn initialize(db: &Connection, actual: &str, journal_mode: JournalMode) -> Result<(), BoxError> {
    let application_id: i64 = db.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let user_tables = list_user_tables(db)?;
    if application_id != 0 && application_id != SESSION_QUERY_SQLITE_APPLICATION_ID {
        return Err(user_error(format!(
            "session-search database at \"{actual}\" belongs to another application"
        )));
    }
    if application_id == 0 && !user_tables.is_empty() {
        return Err(user_error(format!(
            "session-search database at \"{actual}\" is not an empty or recognized derived index"
        )));
    }
    if application_id == SESSION_QUERY_SQLITE_APPLICATION_ID {
        assert_derived_user_tables(actual, &user_tables)?;
        if version != SESSION_QUERY_SQLITE_SCHEMA_VERSION {
            reset_derived_schema(db, &user_tables)?;
        }
    }
    db.execute_batch(&format!("PRAGMA journal_mode = {};", journal_mode.sql()))?;
    ensure_persistent_schema(db)?;
    ensure_temporary_schema(db)?;
    Ok(())
}

fn list_user_tables(db: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut statement = db.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT GLOB 'sqlite_*' ORDER BY name",
    )?;
    statement.query_map([], |r| r.get(0))?.collect()
}

fn assert_derived_user_tables(path: &str, user_tables: &[String]) -> Result<(), BoxError> {
    let unknown = user_tables
        .iter()
        .filter(|name| !DERIVED_USER_TABLES.contains(&name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(user_error(format!(
            "session-search database at \"{path}\" has unrecognized user tables: {}",
            unknown.join(", ")
        )));
    }
    Ok(())
}

fn reset_derived_schema(db: &Connection, user_tables: &[String]) -> rusqlite::Result<()> {
    for name in user_tables {
        db.execute_batch(&format!(
            "DROP TABLE IF EXISTS \"{}\";",
            name.replace('"', "\"\"")
        ))?;
    }
    db.execute_batch("PRAGMA user_version = 0")
}

fn ensure_persistent_schema(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(&format!(
        "PRAGMA application_id = {SESSION_QUERY_SQLITE_APPLICATION_ID};
         CREATE TABLE IF NOT EXISTS search_state (
           singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
           global_generation INTEGER NOT NULL
         ) STRICT;
         INSERT OR IGNORE INTO search_state (singleton, global_generation) VALUES (1, 0);
         CREATE TABLE IF NOT EXISTS persisted_sessions (
           id TEXT PRIMARY KEY, version INTEGER NOT NULL, created_at INTEGER NOT NULL,
           cwd TEXT, parent_session TEXT, seed_length INTEGER, delegation_depth INTEGER,
           agent_preset TEXT, revision TEXT NOT NULL, generation INTEGER NOT NULL
         ) STRICT;
         CREATE VIRTUAL TABLE IF NOT EXISTS persisted_docs USING fts5(
           text, session_id UNINDEXED, seq UNINDEXED, type UNINDEXED, time UNINDEXED,
           surface UNINDEXED, codepoint_length UNINDEXED, tokenize = 'unicode61'
         );
         PRAGMA user_version = {SESSION_QUERY_SQLITE_SCHEMA_VERSION};"
    ))
}

fn ensure_temporary_schema(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS live_sessions (
           id TEXT PRIMARY KEY, version INTEGER NOT NULL, created_at INTEGER NOT NULL,
           cwd TEXT, parent_session TEXT, seed_length INTEGER, delegation_depth INTEGER,
           agent_preset TEXT, fingerprint TEXT NOT NULL, persisted INTEGER NOT NULL CHECK (persisted IN (0, 1)),
           generation INTEGER NOT NULL
         ) STRICT;
         CREATE VIRTUAL TABLE IF NOT EXISTS temp.live_docs USING fts5(
           text, session_id UNINDEXED, seq UNINDEXED, type UNINDEXED, time UNINDEXED,
           surface UNINDEXED, codepoint_length UNINDEXED, tokenize = 'unicode61'
         );",
    )
}

fn user_error(message: String) -> BoxError {
    Box::new(std::io::Error::other(message))
}

fn fs_error(source: std::io::Error, syscall: &str, path: &Path) -> BoxError {
    let path = path.to_string_lossy();
    let (message, code) = if path.contains('\0') {
        let mut inspected = inspect_string(&path);
        if inspected.encode_utf16().count() > 128 {
            let head = inspected.encode_utf16().take(128).collect::<Vec<_>>();
            inspected = format!("{}...", String::from_utf16_lossy(&head));
        }
        (
            format!(
                "The argument 'path' must be a string, Uint8Array, or URL without null bytes. Received {inspected}"
            ),
            "ERR_INVALID_ARG_VALUE",
        )
    } else {
        (
            hoh_error::node_fs_error_message(&source, syscall, Some(&path), None),
            hoh_error::node_error_code(&source).unwrap_or("UNKNOWN"),
        )
    };
    Box::new(hoh_error::HarnessError::new(message, code).with_cause(source))
}

fn inspect_string(value: &str) -> String {
    let quote = if !value.contains('\'') {
        '\''
    } else if !value.contains('"') {
        '"'
    } else if !value.contains('`') && !value.contains("${") {
        '`'
    } else {
        '\''
    };
    let mut out = String::from(quote);
    for character in value.chars() {
        match character {
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            '\\' => out.push_str("\\\\"),
            value if value == quote && quote == '\'' => out.push_str("\\'"),
            value if (value as u32) < 32 || (127..160).contains(&(value as u32)) => {
                out.push_str(&format!("\\x{:02X}", value as u32));
            }
            value => out.push(value),
        }
    }
    out.push(quote);
    out
}

#[cfg(unix)]
fn create_owner_directory(path: &Path) -> Result<(), BoxError> {
    use std::os::unix::fs::DirBuilderExt;
    if path.exists() {
        return Ok(());
    }
    let mut builder = std::fs::DirBuilder::new();
    builder
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|error| fs_error(error, "mkdir", path))?;
    Ok(())
}

#[cfg(not(unix))]
fn create_owner_directory(path: &Path) -> Result<(), BoxError> {
    std::fs::create_dir_all(path).map_err(|error| fs_error(error, "mkdir", path))?;
    Ok(())
}

#[cfg(unix)]
fn create_owner_file(path: &Path) -> Result<(), BoxError> {
    use std::os::unix::fs::OpenOptionsExt;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(fs_error(e, "open", path)),
    }
}

#[cfg(not(unix))]
fn create_owner_file(path: &Path) -> Result<(), BoxError> {
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(fs_error(e, "open", path)),
    }
}

pub(crate) fn read_generation(db: &Connection) -> rusqlite::Result<i64> {
    db.query_row(
        "SELECT global_generation FROM search_state WHERE singleton = 1",
        [],
        |r| r.get(0),
    )
}
