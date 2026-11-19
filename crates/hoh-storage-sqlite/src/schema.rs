//! Physical SQLite layout and open-time configuration.

use std::{fs, io::ErrorKind, path::Path, time::Duration};

use hoh_error::BoxError;
use hoh_storage::{StorageError, StorageErrorCode};
use hoh_util_workspace_path::node_path;
use rusqlite::Connection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const STORAGE_SQLITE_SCHEMA_VERSION: i64 = 1;

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
    const fn sql(self) -> &'static str {
        match self {
            Self::Wal => "WAL",
            Self::Delete => "DELETE",
            Self::Truncate => "TRUNCATE",
            Self::Persist => "PERSIST",
        }
    }
}

/// Open and configure a storage database, or the special in-process database.
pub fn open_database(path: &str, journal_mode: JournalMode) -> Result<Connection, BoxError> {
    let actual = if path == ":memory:" {
        path.to_owned()
    } else {
        let cwd = std::env::current_dir()?;
        node_path::resolve(&cwd.to_string_lossy(), &[path])
    };
    if actual != ":memory:" {
        create_parent_directories(&actual)?;
        create_database_file(&actual)?;
    }
    let connection = Connection::open(&actual)?;
    connection.busy_timeout(Duration::ZERO)?;
    if let Err(error) = configure_database(&connection, &actual, journal_mode) {
        // No prepared statements escape configuration, so close should succeed.
        let _ = connection.close();
        return Err(error);
    }
    Ok(connection)
}

fn create_parent_directories(path: &str) -> Result<(), BoxError> {
    let parent = node_path::dirname(path);
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(Path::new(&parent))?;
    Ok(())
}

fn create_database_file(path: &str) -> Result<(), BoxError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(file) => {
            drop(file);
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn configure_database(
    connection: &Connection,
    path: &str,
    journal_mode: JournalMode,
) -> Result<(), BoxError> {
    connection.execute_batch("PRAGMA foreign_keys = ON")?;
    connection.execute_batch(&format!("PRAGMA journal_mode = {}", journal_mode.sql()))?;
    let on_disk: i64 =
        connection.query_row("PRAGMA user_version", [], |row| row.get("user_version"))?;
    if on_disk != 0 && on_disk != STORAGE_SQLITE_SCHEMA_VERSION {
        return Err(StorageError::new(
            StorageErrorCode::VersionMismatch,
            format!(
                "storage database at \"{path}\" has schema version {on_disk}, incompatible with this build ({STORAGE_SQLITE_SCHEMA_VERSION})"
            ),
        )
        .into());
    }
    connection.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS units (
          name    TEXT PRIMARY KEY,
          version INTEGER NOT NULL
        ) STRICT;
        CREATE TABLE IF NOT EXISTS unit_globals (
          unit  TEXT PRIMARY KEY REFERENCES units(name),
          value TEXT NOT NULL
        ) STRICT;
        "#,
    )?;
    if on_disk == 0 {
        connection.execute_batch(&format!(
            "PRAGMA user_version = {STORAGE_SQLITE_SCHEMA_VERSION}"
        ))?;
    }
    Ok(())
}

pub fn record_table_name(unit: &str, table: &str) -> String {
    format!("u_{unit}_{table}")
}
