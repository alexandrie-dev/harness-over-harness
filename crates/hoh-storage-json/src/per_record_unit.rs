//! Stateless per-record tree; staged deletion, restoration and quarantine.
use crate::{
    atomic::{self, fs_error, mkdir, read_file, write_atomic},
    format::{UnitState, accepted, parse_record, serialize_record},
    runtime::{Lifecycle, plain, ready, spawn},
};
use futures_util::future::{join_all, try_join_all};
use hoh_error::BoxError;
use hoh_storage::{
    BackendFuture, KvSnapshot, KvUnit, KvUnitDescriptor, StorageError, StorageErrorCode,
};
use hoh_util_values::{js_json_parse, js_json_stringify};
use hoh_util_workspace_path::node_path::{dirname, join};
use indexmap::{IndexMap, IndexSet};
use serde_json::Value;
use std::sync::Arc;
use tokio::fs;

pub async fn open_per_record_unit(
    descriptor: KvUnitDescriptor,
    root: &str,
    on_close: Arc<dyn Fn() -> Result<(), BoxError> + Send + Sync>,
) -> Result<Arc<dyn KvUnit>, BoxError> {
    let dir = join(&[root, &descriptor.name]);
    Ok(Arc::new(PerRecordJsonUnit::new(descriptor, dir, on_close)))
}

pub(crate) trait DirectoryIo: Send + Sync {
    fn rename<'a>(&'a self, from: &'a str, to: &'a str) -> BackendFuture<'a, ()>;
    fn sync<'a>(&'a self, dir: &'a str) -> BackendFuture<'a, ()>;
}
struct Disk;
impl DirectoryIo for Disk {
    fn rename<'a>(&'a self, from: &'a str, to: &'a str) -> BackendFuture<'a, ()> {
        Box::pin(atomic::rename(from, to))
    }
    fn sync<'a>(&'a self, dir: &'a str) -> BackendFuture<'a, ()> {
        Box::pin(atomic::fsync_directory(dir))
    }
}
struct Inner {
    descriptor: KvUnitDescriptor,
    dir: String,
    life: Lifecycle,
    io: Arc<dyn DirectoryIo>,
}
pub struct PerRecordJsonUnit {
    inner: Arc<Inner>,
}
impl PerRecordJsonUnit {
    pub fn new(
        descriptor: KvUnitDescriptor,
        dir: String,
        on_close: Arc<dyn Fn() -> Result<(), BoxError> + Send + Sync>,
    ) -> Self {
        Self::with_io(descriptor, dir, on_close, Arc::new(Disk))
    }
    pub(crate) fn with_io(
        descriptor: KvUnitDescriptor,
        dir: String,
        on_close: Arc<dyn Fn() -> Result<(), BoxError> + Send + Sync>,
        io: Arc<dyn DirectoryIo>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                life: Lifecycle::new(descriptor.name.clone(), on_close),
                descriptor,
                dir,
                io,
            }),
        }
    }
    fn remove_records(&self, table: &str, keys: &[String]) -> BackendFuture<'static, ()> {
        self.inner.life.start(|| {
            let dir = self.inner.table_dir(table)?;
            let keys: IndexSet<String> = keys.iter().cloned().collect();
            for key in &keys {
                assert_safe_key(&self.inner.descriptor.name, key)?;
            }
            let inner = self.inner.clone();
            Ok(async move { inner.remove_documents(&dir, &keys).await })
        })
    }
}
impl Inner {
    fn table_dir(&self, table: &str) -> Result<String, BoxError> {
        if !self
            .descriptor
            .tables
            .iter()
            .any(|declared| declared == table)
        {
            return Err(plain(format!(
                "unit '{}' does not declare table '{table}'",
                self.descriptor.name
            )));
        }
        Ok(join(&[&self.dir, table]))
    }
    async fn write_document(&self, path: &str, value: Value) -> Result<(), BoxError> {
        let text = serialize_record(self.descriptor.version, &value);
        mkdir(&dirname(path)).await?;
        write_atomic(path, &text).await
    }
    async fn remove_documents(&self, dir: &str, keys: &IndexSet<String>) -> Result<(), BoxError> {
        let mut staged = Vec::new();
        let result = async {
            for key in keys {
                let from = join(&[dir, &format!("{key}.json")]);
                let to = format!("{from}.deleting");
                match self.io.rename(&from, &to).await {
                    Ok(()) => staged.push((from, to)),
                    Err(error) if atomic::missing(&error) => {}
                    Err(error) => return Err(error),
                }
            }
            if !staged.is_empty() {
                self.io.sync(dir).await?;
            }
            Ok(())
        }
        .await;
        if let Err(error) = result {
            let mut restore_failures = Vec::new();
            for (from, to) in &staged {
                if let Err(error) = self.io.rename(to, from).await {
                    restore_failures.push(error);
                }
            }
            if !staged.is_empty()
                && let Err(error) = self.io.sync(dir).await
            {
                restore_failures.push(error);
            }
            if restore_failures.is_empty() {
                return Err(error);
            }
            let mut errors = vec![error];
            errors.extend(restore_failures);
            let error = StorageError::new(
                StorageErrorCode::UnknownState,
                format!(
                    "unit '{}' could not restore a failed batch delete; its state is unknown",
                    self.descriptor.name
                ),
            )
            .with_cause(BatchDeleteError { errors });
            self.life.quarantine(error.clone());
            return Err(Box::new(error));
        }
        // Source Promise.allSettled ignores cleanup failures, including directories.
        let _ = join_all(staged.into_iter().map(|(_, to)| fs::remove_file(to))).await;
        Ok(())
    }
}

/// Rust representation of the source AggregateError, in original failure order.
#[derive(Debug, thiserror::Error)]
#[error("batch delete failed and restoration failed")]
pub struct BatchDeleteError {
    pub errors: Vec<BoxError>,
}

impl KvUnit for PerRecordJsonUnit {
    fn load_all(&self) -> BackendFuture<'_, KvSnapshot> {
        if let Err(error) = self.inner.life.checked(|| Ok(())) {
            return ready(Err(error));
        }
        let inner = self.inner.clone();
        // Source loadAll is not in inFlight, even when it bootstraps legacy data.
        spawn(async move {
            Ok(load_per_record_state(&inner.descriptor, &inner.dir)
                .await?
                .snapshot())
        })
    }
    fn put_record<'a>(
        &'a self,
        table: &'a str,
        key: &'a str,
        value: Value,
    ) -> BackendFuture<'a, ()> {
        self.inner.life.start(|| {
            assert_safe_key(&self.inner.descriptor.name, key)?;
            let path = join(&[&self.inner.table_dir(table)?, &format!("{key}.json")]);
            let inner = self.inner.clone();
            Ok(async move { inner.write_document(&path, value).await })
        })
    }
    fn delete_record<'a>(&'a self, table: &'a str, key: &'a str) -> BackendFuture<'a, ()> {
        self.remove_records(table, &[key.to_owned()])
    }
    fn delete_records<'a>(&'a self, table: &'a str, keys: &'a [String]) -> BackendFuture<'a, ()> {
        self.remove_records(table, keys)
    }
    fn backup_record<'a>(
        &'a self,
        table: &'a str,
        key: &'a str,
    ) -> Option<BackendFuture<'a, String>> {
        Some(self.inner.life.start(|| {
            assert_safe_key(&self.inner.descriptor.name, key)?;
            let path = join(&[&self.inner.table_dir(table)?, &format!("{key}.json")]);
            let moved = format!("{path}.bak.{}", backup_stamp(&jiff::ZonedDateTime::now()));
            let inner = self.inner.clone();
            Ok(async move {
                inner.io.rename(&path, &moved).await?;
                Ok(moved)
            })
        }))
    }
    fn set_global(&self, value: Value) -> BackendFuture<'_, ()> {
        self.inner.life.start(|| {
            if !self.inner.descriptor.has_global {
                return Err(plain(format!(
                    "unit '{}' does not declare a global slot",
                    self.inner.descriptor.name
                )));
            }
            let path = join(&[&self.inner.dir, "global.json"]);
            let inner = self.inner.clone();
            Ok(async move { inner.write_document(&path, value).await })
        })
    }
    fn close(&self) -> BackendFuture<'_, ()> {
        self.inner.life.close()
    }
}

fn safe_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}
fn assert_safe_key(unit: &str, key: &str) -> Result<(), BoxError> {
    if safe_key(key) {
        Ok(())
    } else {
        Err(plain(format!(
            "unit '{unit}': per-record key '{key}' is not path-safe (must match /^[a-zA-Z0-9_-]+$/)"
        )))
    }
}
fn backup_stamp(now: &jiff::ZonedDateTime) -> String {
    format!(
        "{}{:02}{:02}{:02}{:02}",
        now.year(),
        now.month(),
        now.day(),
        now.hour(),
        now.minute()
    )
}
async fn entries(path: &str, missing_ok: bool) -> Result<Option<Vec<fs::DirEntry>>, BoxError> {
    let mut reader = match fs::read_dir(path).await {
        Ok(reader) => reader,
        Err(error) if missing_ok && error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => return Err(fs_error(error, "scandir", Some(path), None)),
    };
    let mut result = Vec::new();
    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|error| fs_error(error, "scandir", Some(path), None))?
    {
        result.push(entry);
    }
    // Node's readdir (unlike opendir) sorts by bytes on Unix.
    result.sort_by_key(|entry| entry.file_name());
    Ok(Some(result))
}
async fn read_record(path: &str, versions: &[u64]) -> Option<Value> {
    parse_record(&read_file(path).await.ok()??, versions)
}
async fn load_per_record_state(
    descriptor: &KvUnitDescriptor,
    dir: &str,
) -> Result<UnitState, BoxError> {
    let mut versions = vec![descriptor.version];
    versions.extend(descriptor.compatible_versions.iter().flatten());
    let mut state = UnitState::empty(descriptor);
    let mut has_new_state = false;
    enum Loaded {
        Table(String, IndexMap<String, Value>),
        Global(Option<Value>),
        Ignored,
    }
    let loaded = try_join_all(
        entries(dir, true)
            .await?
            .unwrap_or_default()
            .into_iter()
            .map(|entry| {
                let versions = &versions;
                async move {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let path = join(&[dir, &name]);
                    let is_directory = entry
                        .file_type()
                        .await
                        .map_err(|error| fs_error(error, "lstat", Some(&path), None))?
                        .is_dir();
                    if is_directory && descriptor.tables.contains(&name) {
                        let records = join_all(
                            entries(&path, false)
                                .await?
                                .unwrap_or_default()
                                .into_iter()
                                .map(|file| {
                                    let path = path.clone();
                                    async move {
                                        let name = file.file_name().to_string_lossy().into_owned();
                                        let key = name
                                            .strip_suffix(".json")
                                            .filter(|key| safe_key(key))?
                                            .to_owned();
                                        let record =
                                            read_record(&join(&[&path, &name]), versions).await?;
                                        Some((key, record))
                                    }
                                }),
                        )
                        .await
                        .into_iter()
                        .flatten()
                        .collect();
                        return Ok::<_, BoxError>(Loaded::Table(name, records));
                    }
                    if name == "global.json" && descriptor.has_global {
                        return Ok(Loaded::Global(read_record(&path, versions).await));
                    }
                    Ok(Loaded::Ignored)
                }
            }),
    )
    .await?;
    for loaded in loaded {
        match loaded {
            Loaded::Table(name, records) => {
                state.tables.insert(name, records);
                has_new_state = true;
            }
            Loaded::Global(global) => {
                if let Some(global) = global {
                    state.global = global;
                }
                has_new_state = true;
            }
            Loaded::Ignored => {}
        }
    }
    if !has_new_state {
        bootstrap_legacy_unit(descriptor, dir, &mut state).await?;
    }
    Ok(state)
}
async fn bootstrap_legacy_unit(
    descriptor: &KvUnitDescriptor,
    dir: &str,
    state: &mut UnitState,
) -> Result<(), BoxError> {
    let legacy = join(&[&dirname(dir), &format!("{}.json", descriptor.name)]);
    let Some(text) = read_file(&legacy).await? else {
        return Ok(());
    };
    let Ok(document) = js_json_parse(&text) else {
        return Ok(());
    };
    // Source's property access is outside its JSON.parse catch.
    if document.is_null() {
        return Err(plain("Cannot read properties of null (reading 'unit')"));
    }
    if document
        .get("unit")
        .and_then(|unit| unit.get("name"))
        .and_then(Value::as_str)
        != Some(&descriptor.name)
    {
        return Ok(());
    }
    if !document
        .get("unit")
        .and_then(|unit| unit.get("version"))
        .is_some_and(|stamp| accepted(stamp, descriptor))
    {
        return Ok(());
    }
    let Some(tables) = document
        .get("tables")
        .filter(|tables| tables.is_object() || tables.is_array())
    else {
        return Ok(());
    };
    for (table, records) in object_entries(tables)? {
        let Some(target) = state.tables.get_mut(&table) else {
            continue;
        };
        for (key, value) in object_entries(&records)? {
            // Bootstrap deliberately does not apply the live-write safe-key guard.
            let path = join(&[dir, &table, &format!("{key}.json")]);
            mkdir(&dirname(&path)).await?;
            write_atomic(&path, &serialize_record(descriptor.version, &value)).await?;
            target.insert(key, value);
        }
    }
    Ok(())
}
fn object_entries(value: &Value) -> Result<Vec<(String, Value)>, BoxError> {
    match value {
        Value::Object(_) => {
            // Object.entries visits integer keys first; use the shared JS writer.
            let ordered =
                js_json_parse(&js_json_stringify(value)).expect("stringified JSON is valid");
            Ok(ordered
                .as_object()
                .expect("object remains object")
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect())
        }
        Value::Array(values) => Ok(values
            .iter()
            .enumerate()
            .map(|(index, value)| (index.to_string(), value.clone()))
            .collect()),
        Value::String(value) => Ok(value
            .encode_utf16()
            .enumerate()
            .map(|(index, unit)| {
                (
                    index.to_string(),
                    Value::String(
                        char::from_u32(u32::from(unit))
                            .unwrap_or(char::REPLACEMENT_CHARACTER)
                            .to_string(),
                    ),
                )
            })
            .collect()),
        Value::Null => Err(plain("Cannot convert undefined or null to object")),
        _ => Ok(Vec::new()),
    }
}
