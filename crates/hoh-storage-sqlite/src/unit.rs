//! One opened SQLite key-value unit.

use std::{
    collections::HashSet,
    error::Error,
    fmt,
    sync::{Arc, Mutex},
};

use hoh_error::{BoxError, SharedError};
use hoh_storage::{
    BackendFuture, KvSnapshot, KvUnit, KvUnitDescriptor, StorageError, StorageErrorCode,
};
use hoh_util_values::{js_json_parse, js_json_stringify};
use indexmap::IndexMap;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use crate::{BackendInner, lock, schema::record_table_name, sqlite_version};

struct UnitState {
    closed: bool,
    needs_version_stamp: bool,
}

#[derive(Debug)]
pub struct RollbackFailure {
    original: SharedError,
    rollback: SharedError,
}

impl fmt::Display for RollbackFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = &self.rollback;
        formatter.write_str("write and rollback failed")
    }
}

impl Error for RollbackFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.original.get_ref())
    }
}

impl RollbackFailure {
    /// The write error followed by the rollback error, matching AggregateError.errors.
    pub fn errors(&self) -> (&SharedError, &SharedError) {
        (&self.original, &self.rollback)
    }
}

pub struct SqliteKvUnit {
    backend: Arc<BackendInner>,
    descriptor: KvUnitDescriptor,
    generation: u64,
    state: Mutex<UnitState>,
}

impl SqliteKvUnit {
    pub(crate) fn new(
        backend: Arc<BackendInner>,
        descriptor: KvUnitDescriptor,
        stored_version: u64,
        generation: u64,
    ) -> Self {
        let needs_version_stamp = stored_version != descriptor.version;
        Self {
            backend,
            descriptor,
            generation,
            state: Mutex::new(UnitState {
                closed: false,
                needs_version_stamp,
            }),
        }
    }

    pub(crate) fn close_now(&self) {
        let mut unit = lock(&self.state);
        if unit.closed {
            return;
        }
        unit.closed = true;
        let mut backend = lock(&self.backend.state);
        if backend
            .units
            .get(&self.descriptor.name)
            .is_some_and(|slot| slot.generation == self.generation)
        {
            backend.units.remove(&self.descriptor.name);
        }
    }

    fn with_connection<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, BoxError>,
    ) -> Result<T, BoxError> {
        let unit = lock(&self.state);
        if unit.closed {
            return Err(StorageError::new(
                StorageErrorCode::Closed,
                format!("kv unit '{}' is closed", self.descriptor.name),
            )
            .into());
        }
        let backend = lock(&self.backend.state);
        if let Some(failure) = &backend.failure {
            return Err(Box::new(failure.clone()));
        }
        let connection = backend.connection.as_ref().ok_or_else(|| {
            Box::new(StorageError::new(
                StorageErrorCode::Closed,
                "sqlite storage backend is closed",
            )) as BoxError
        })?;
        operation(connection)
    }

    fn write(
        &self,
        transactional: bool,
        operation: impl FnOnce(&Connection) -> rusqlite::Result<usize>,
    ) -> Result<(), BoxError> {
        let mut unit = lock(&self.state);
        if unit.closed {
            return Err(StorageError::new(
                StorageErrorCode::Closed,
                format!("kv unit '{}' is closed", self.descriptor.name),
            )
            .into());
        }
        let mut backend = lock(&self.backend.state);
        if let Some(failure) = &backend.failure {
            return Err(Box::new(failure.clone()));
        }
        let connection = backend.connection.as_ref().ok_or_else(|| {
            Box::new(StorageError::new(
                StorageErrorCode::Closed,
                "sqlite storage backend is closed",
            )) as BoxError
        })?;
        if !unit.needs_version_stamp && !transactional {
            operation(connection)?;
            return Ok(());
        }
        connection.execute_batch("BEGIN IMMEDIATE")?;
        let attempt = (|| -> rusqlite::Result<bool> {
            let changed = operation(connection)? > 0;
            if unit.needs_version_stamp && changed {
                connection.execute(
                    "UPDATE units SET version = ? WHERE name = ?",
                    params![
                        sqlite_version(self.descriptor.version).map_err(to_sql_error)?,
                        self.descriptor.name
                    ],
                )?;
            }
            connection.execute_batch("COMMIT")?;
            Ok(changed)
        })();
        match attempt {
            Ok(changed) => {
                if changed {
                    unit.needs_version_stamp = false;
                }
                Ok(())
            }
            Err(original) => {
                if connection.is_autocommit() {
                    return Err(original.into());
                }
                if let Err(rollback) = connection.execute_batch("ROLLBACK") {
                    let original = SharedError::new(Box::new(original) as BoxError);
                    let rollback = SharedError::new(Box::new(rollback) as BoxError);
                    let aggregate = RollbackFailure { original, rollback };
                    let failure = StorageError::new(
                        StorageErrorCode::UnknownState,
                        format!(
                            "kv unit '{}' write and rollback failed; the backend is unusable and must be closed and reopened",
                            self.descriptor.name
                        ),
                    )
                    .with_cause(Box::new(aggregate) as BoxError);
                    backend.failure = Some(failure.clone());
                    return Err(Box::new(failure));
                }
                Err(original.into())
            }
        }
    }

    fn ensure_usable(&self) -> Result<(), BoxError> {
        let unit = lock(&self.state);
        if unit.closed {
            return Err(StorageError::new(
                StorageErrorCode::Closed,
                format!("kv unit '{}' is closed", self.descriptor.name),
            )
            .into());
        }
        let backend = lock(&self.backend.state);
        if let Some(failure) = &backend.failure {
            return Err(Box::new(failure.clone()));
        }
        if backend.connection.is_none() {
            return Err(StorageError::new(
                StorageErrorCode::Closed,
                "sqlite storage backend is closed",
            )
            .into());
        }
        Ok(())
    }

    fn ensure_table(&self, table: &str) -> Result<String, BoxError> {
        self.ensure_usable()?;
        if !self.descriptor.tables.iter().any(|name| name == table) {
            return Err(std::io::Error::other(format!(
                "kv unit '{}' declared no table '{table}'",
                self.descriptor.name
            ))
            .into());
        }
        Ok(record_table_name(&self.descriptor.name, table))
    }
}

impl KvUnit for SqliteKvUnit {
    fn load_all(&self) -> BackendFuture<'_, KvSnapshot> {
        let result = self.with_connection(|connection| {
            let mut tables = IndexMap::new();
            for table in &self.descriptor.tables {
                let physical = record_table_name(&self.descriptor.name, table);
                let mut statement =
                    connection.prepare(&format!("SELECT key, value FROM \"{physical}\""))?;
                let mut rows = statement.query([])?;
                let mut entries = Vec::new();
                while let Some(row) = rows.next()? {
                    let key: String = row.get(0)?;
                    let text: String = row.get(1)?;
                    let value = parse_value(
                        &self.descriptor.name,
                        &text,
                        &format!("table '{table}' key '{key}'"),
                    )?;
                    entries.push((key, value));
                }
                tables.insert(table.clone(), js_order(entries));
            }
            let global = if self.descriptor.has_global {
                let text: Option<String> = connection
                    .query_row(
                        "SELECT value FROM unit_globals WHERE unit = ?",
                        [&self.descriptor.name],
                        |row| row.get(0),
                    )
                    .optional()?;
                match text {
                    Some(text) => parse_value(&self.descriptor.name, &text, "global slot")?,
                    None => Value::Null,
                }
            } else {
                Value::Null
            };
            Ok(KvSnapshot { tables, global })
        });
        Box::pin(async move { result })
    }

    fn put_record<'a>(
        &'a self,
        table: &'a str,
        key: &'a str,
        value: Value,
    ) -> BackendFuture<'a, ()> {
        let result = self.ensure_table(table).and_then(|physical| {
            let text = js_json_stringify(&value);
            self.write(false, |connection| {
                connection.execute(
                    &format!(
                        "INSERT INTO \"{physical}\" (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value"
                    ),
                    params![key, text],
                )
            })
        });
        Box::pin(async move { result })
    }

    fn delete_record<'a>(&'a self, table: &'a str, key: &'a str) -> BackendFuture<'a, ()> {
        let result = self.ensure_table(table).and_then(|physical| {
            self.write(false, |connection| {
                connection.execute(&format!("DELETE FROM \"{physical}\" WHERE key = ?"), [key])
            })
        });
        Box::pin(async move { result })
    }

    fn delete_records<'a>(&'a self, table: &'a str, keys: &'a [String]) -> BackendFuture<'a, ()> {
        let result = self.ensure_table(table).and_then(|physical| {
            if keys.is_empty() {
                return Ok(());
            }
            let mut seen = HashSet::new();
            let ordered = keys
                .iter()
                .filter(|key| seen.insert(key.as_str()))
                .collect::<Vec<_>>();
            self.write(true, |connection| {
                let mut statement =
                    connection.prepare(&format!("DELETE FROM \"{physical}\" WHERE key = ?"))?;
                let mut changes = 0;
                for key in ordered {
                    changes += statement.execute([key])?;
                }
                Ok(changes)
            })
        });
        Box::pin(async move { result })
    }

    fn set_global(&self, value: Value) -> BackendFuture<'_, ()> {
        let result = self.ensure_usable().and_then(|()| if self.descriptor.has_global {
            let text = js_json_stringify(&value);
            self.write(false, |connection| {
                connection.execute(
                    "INSERT INTO unit_globals (unit, value) VALUES (?, ?) ON CONFLICT(unit) DO UPDATE SET value = excluded.value",
                    params![self.descriptor.name, text],
                )
            })
        } else {
            Err(std::io::Error::other(format!(
                "kv unit '{}' declared no global slot",
                self.descriptor.name
            ))
            .into())
        });
        Box::pin(async move { result })
    }

    fn close(&self) -> BackendFuture<'_, ()> {
        self.close_now();
        Box::pin(async { Ok(()) })
    }
}

fn parse_value(unit: &str, text: &str, slot: &str) -> Result<Value, BoxError> {
    js_json_parse(text).map_err(|error| {
        StorageError::new(
            StorageErrorCode::MalformedMedium,
            format!("kv unit '{unit}' holds unparsable JSON at {slot}"),
        )
        .with_cause(Box::new(error) as BoxError)
        .into()
    })
}

fn js_order(mut entries: Vec<(String, Value)>) -> IndexMap<String, Value> {
    entries.sort_by_key(|(key, _)| array_index(key).map_or((1, 0), |index| (0, index)));
    entries.into_iter().collect()
}

fn array_index(key: &str) -> Option<u32> {
    let index = key.parse::<u32>().ok()?;
    (index != u32::MAX && index.to_string() == key).then_some(index)
}

fn to_sql_error(error: BoxError) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_javascript_array_indices_before_other_keys() {
        let ordered = js_order(vec![
            ("10".into(), Value::Null),
            ("b".into(), Value::Null),
            ("2".into(), Value::Null),
            ("01".into(), Value::Null),
        ]);
        assert_eq!(
            ordered.keys().map(String::as_str).collect::<Vec<_>>(),
            ["2", "10", "b", "01"]
        );
    }

    #[test]
    fn rollback_failure_retains_both_causes() {
        let aggregate = RollbackFailure {
            original: SharedError::new(std::io::Error::other("write failed")),
            rollback: SharedError::new(std::io::Error::other("rollback failed")),
        };
        let (original, rollback) = aggregate.errors();
        assert_eq!(original.to_string(), "write failed");
        assert_eq!(rollback.to_string(), "rollback failed");
    }
}
