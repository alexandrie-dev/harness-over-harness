//! SQLite-backed storage plugin.

pub mod schema;
pub mod unit;

use std::{
    collections::HashMap,
    io,
    sync::{Arc, Mutex, MutexGuard, Weak},
};

use hoh_error::{BoxError, SharedError};
use hoh_kernel::{Cfg, Context, Inject, Service, Undo, plugin, service};
use hoh_storage::{
    BackendFuture, KvFacet, KvUnit, KvUnitDescriptor, Storage, StorageBackend, StorageError,
    StorageErrorCode, UNIT_NAME_RE,
};
use rusqlite::{Connection, OptionalExtension, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use schema::{JournalMode, STORAGE_SQLITE_SCHEMA_VERSION};
use schema::{open_database, record_table_name};
use unit::SqliteKvUnit;

pub const NAME: &str = "storage-sqlite";
pub const INJECT: [&str; 1] = ["storage"];

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub path: String,
    #[serde(default)]
    pub journal_mode: JournalMode,
}

enum StoredOpenError {
    Storage(StorageError),
    Other(SharedError),
}

impl StoredOpenError {
    fn from_box(error: BoxError) -> Self {
        match error.downcast::<StorageError>() {
            Ok(error) => Self::Storage(*error),
            Err(error) => Self::Other(SharedError::new(error)),
        }
    }

    fn boxed(&self) -> BoxError {
        match self {
            Self::Storage(error) => Box::new(error.clone()),
            Self::Other(error) => Box::new(error.clone()),
        }
    }
}

struct OpenUnit {
    generation: u64,
    unit: Weak<SqliteKvUnit>,
}

pub(crate) struct BackendState {
    pub(crate) connection: Option<Connection>,
    open_error: Option<StoredOpenError>,
    closing: bool,
    close_result: Option<Result<(), SharedError>>,
    pub(crate) failure: Option<StorageError>,
    pub(crate) units: HashMap<String, OpenUnit>,
    next_generation: u64,
}

pub(crate) struct BackendInner {
    pub(crate) state: Mutex<BackendState>,
    close_lock: Mutex<()>,
}

#[derive(Clone)]
pub struct SqliteStorageBackend {
    pub(crate) inner: Arc<BackendInner>,
}

impl SqliteStorageBackend {
    pub fn new(config: Config) -> Self {
        let (connection, open_error) = match open_database(&config.path, config.journal_mode) {
            Ok(connection) => (Some(connection), None),
            Err(error) => (None, Some(StoredOpenError::from_box(error))),
        };
        Self {
            inner: Arc::new(BackendInner {
                state: Mutex::new(BackendState {
                    connection,
                    open_error,
                    closing: false,
                    close_result: None,
                    failure: None,
                    units: HashMap::new(),
                    next_generation: 0,
                }),
                close_lock: Mutex::new(()),
            }),
        }
    }

    fn open_unit(&self, descriptor: KvUnitDescriptor) -> Result<Arc<dyn KvUnit>, BoxError> {
        let mut state = lock(&self.inner.state);
        if state.closing {
            return Err(StorageError::new(
                StorageErrorCode::Closed,
                "sqlite storage backend is closed",
            )
            .into());
        }
        if let Some(failure) = &state.failure {
            return Err(Box::new(failure.clone()));
        }
        validate_descriptor(&descriptor)?;
        if state.units.contains_key(&descriptor.name) {
            return Err(io::Error::other(format!(
                "kv unit '{}' is already open (double-open is a caller bug)",
                descriptor.name
            ))
            .into());
        }
        let name = descriptor.name.clone();
        let generation = state.next_generation;
        state.next_generation = state.next_generation.wrapping_add(1);
        state.units.insert(
            name.clone(),
            OpenUnit {
                generation,
                unit: Weak::new(),
            },
        );
        let result = materialize_unit(&self.inner, &mut state, descriptor, generation);
        if result.is_err()
            && state
                .units
                .get(&name)
                .is_some_and(|slot| slot.generation == generation)
        {
            state.units.remove(&name);
        }
        result.map(|unit| unit as Arc<dyn KvUnit>)
    }

    fn close_now(&self) -> Result<(), BoxError> {
        let _close = lock(&self.inner.close_lock);
        {
            let state = lock(&self.inner.state);
            if let Some(result) = &state.close_result {
                return result.clone().map_err(|error| Box::new(error) as BoxError);
            }
        }
        let units = {
            let mut state = lock(&self.inner.state);
            state.closing = true;
            state
                .units
                .values()
                .filter_map(|slot| slot.unit.upgrade())
                .collect::<Vec<_>>()
        };
        for unit in units {
            unit.close_now();
        }
        let connection = lock(&self.inner.state).connection.take();
        let result = connection.map_or(Ok(()), |connection| {
            connection
                .close()
                .map_err(|(_, error)| SharedError::new(Box::new(error) as BoxError))
        });
        lock(&self.inner.state).close_result = Some(result.clone());
        result.map_err(|error| Box::new(error) as BoxError)
    }
}

fn materialize_unit(
    inner: &Arc<BackendInner>,
    state: &mut BackendState,
    descriptor: KvUnitDescriptor,
    generation: u64,
) -> Result<Arc<SqliteKvUnit>, BoxError> {
    if let Some(error) = &state.open_error {
        return Err(error.boxed());
    }
    if let Some(failure) = &state.failure {
        return Err(Box::new(failure.clone()));
    }
    let connection = state.connection.as_ref().ok_or_else(|| {
        Box::new(StorageError::new(
            StorageErrorCode::Closed,
            "sqlite storage backend is closed",
        )) as BoxError
    })?;
    let row: Option<i64> = connection
        .query_row(
            "SELECT version FROM units WHERE name = ?",
            [&descriptor.name],
            |row| row.get(0),
        )
        .optional()?;
    let stored_version = match row {
        None => {
            let version = sqlite_version(descriptor.version)?;
            connection.execute(
                "INSERT INTO units (name, version) VALUES (?, ?)",
                params![descriptor.name, version],
            )?;
            descriptor.version
        }
        Some(version) => {
            let accepted = u64::try_from(version).ok().is_some_and(|version| {
                version == descriptor.version
                    || descriptor
                        .compatible_versions
                        .as_ref()
                        .is_some_and(|versions| versions.contains(&version))
            });
            if !accepted {
                return Err(StorageError::new(
                    StorageErrorCode::VersionMismatch,
                    format!(
                        "kv unit '{}' is stamped version {version} on the medium, incompatible with descriptor version {}",
                        descriptor.name, descriptor.version
                    ),
                )
                .into());
            }
            u64::try_from(version).expect("accepted SQLite version is non-negative")
        }
    };
    for table in &descriptor.tables {
        connection.execute_batch(&format!(
            r#"CREATE TABLE IF NOT EXISTS "{}" (
              key   TEXT PRIMARY KEY,
              value TEXT NOT NULL
            ) STRICT"#,
            record_table_name(&descriptor.name, table)
        ))?;
    }
    let name = descriptor.name.clone();
    let unit = Arc::new(SqliteKvUnit::new(
        inner.clone(),
        descriptor,
        stored_version,
        generation,
    ));
    if let Some(slot) = state.units.get_mut(&name)
        && slot.generation == generation
    {
        slot.unit = Arc::downgrade(&unit);
    }
    Ok(unit)
}

fn validate_descriptor(descriptor: &KvUnitDescriptor) -> Result<(), BoxError> {
    if !UNIT_NAME_RE.is_match(&descriptor.name) {
        return Err(io::Error::other(format!(
            "kv unit name '{}' violates /{}/",
            descriptor.name,
            UNIT_NAME_RE.as_str()
        ))
        .into());
    }
    for table in &descriptor.tables {
        if !UNIT_NAME_RE.is_match(table) {
            return Err(io::Error::other(format!(
                "kv table name '{table}' in unit '{}' violates /{}/",
                descriptor.name,
                UNIT_NAME_RE.as_str()
            ))
            .into());
        }
    }
    Ok(())
}

pub(crate) fn sqlite_version(version: u64) -> Result<i64, BoxError> {
    i64::try_from(version)
        .map_err(|_| io::Error::other(format!("kv unit version {version} exceeds SQLite INTEGER")))
        .map_err(Into::into)
}

impl StorageBackend for SqliteStorageBackend {
    fn kv(&self) -> Option<&dyn KvFacet> {
        Some(self)
    }

    fn close(&self) -> BackendFuture<'_, ()> {
        let result = self.close_now();
        Box::pin(async move { result })
    }
}

impl KvFacet for SqliteStorageBackend {
    fn open(&self, descriptor: KvUnitDescriptor) -> BackendFuture<'_, Arc<dyn KvUnit>> {
        let result = self.open_unit(descriptor);
        Box::pin(async move { result })
    }
}

service!(pub StorageSqlite: SqliteStorageBackend = "storage.backend.sqlite");

#[derive(Inject)]
pub struct Deps {
    storage: Service<Storage>,
}

#[plugin(StorageSqlitePlugin, name = "storage-sqlite")]
pub async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
    let backend = Arc::new(SqliteStorageBackend::new((*config.get()).clone()));
    let registered = backend.clone();
    ctx.try_effect("storage-sqlite.registerBackend", move |undos| {
        let unregister = deps
            .storage
            .backend
            .register("sqlite", registered.clone())?;
        undos.push(Undo::future(move || {
            let backend = registered.clone();
            async move {
                unregister();
                backend.close().await
            }
        }))?;
        Ok::<(), BoxError>(())
    })?;
    ctx.provide::<StorageSqlite>(backend).await?;
    Ok(())
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use serde_json::json;

    use super::*;

    fn descriptor(name: &str) -> KvUnitDescriptor {
        KvUnitDescriptor {
            name: name.into(),
            version: 1,
            tables: vec!["records".into()],
            has_global: true,
            layout: None,
            compatible_versions: None,
        }
    }

    fn retained_error(error: &BoxError) -> &StorageError {
        error.downcast_ref::<StorageError>().unwrap()
    }

    #[tokio::test]
    async fn retained_unknown_state_rejects_all_units_and_new_opens() {
        let backend = SqliteStorageBackend::new(Config {
            path: ":memory:".into(),
            journal_mode: JournalMode::Wal,
        });
        let first = backend.open_unit(descriptor("first")).unwrap();
        let sibling = backend.open_unit(descriptor("sibling")).unwrap();
        let sentinel = SharedError::new(io::Error::other("rollback failed"));
        lock(&backend.inner.state).failure = Some(
            StorageError::new(
                StorageErrorCode::UnknownState,
                "unknown transaction outcome",
            )
            .with_cause(Box::new(sentinel.clone()) as BoxError),
        );

        let read = sibling.load_all().await.err().unwrap();
        let write = first
            .put_record("records", "k", json!(1))
            .await
            .err()
            .unwrap();
        let open = backend.open_unit(descriptor("later")).err().unwrap();
        for error in [&read, &write, &open] {
            assert_eq!(retained_error(error).code(), "unknown-state");
            assert_eq!(
                retained_error(error).source().unwrap().to_string(),
                "rollback failed"
            );
        }
        backend.close().await.unwrap();
    }
}
