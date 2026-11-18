//! One authoritative in-memory state, atomically projected to one document.
use crate::{
    atomic::{read_file, write_atomic},
    format::{UnitState, parse, serialize},
    runtime::{Lifecycle, lock, plain, ready},
};
use hoh_error::BoxError;
use hoh_storage::{BackendFuture, KvSnapshot, KvUnit, KvUnitDescriptor};
use hoh_util_workspace_path::node_path::join;
use indexmap::IndexMap;
use serde_json::Value;
use std::sync::{Arc, Mutex};

pub async fn open_single_unit(
    descriptor: KvUnitDescriptor,
    root: &str,
    on_close: Arc<dyn Fn() -> Result<(), BoxError> + Send + Sync>,
) -> Result<Arc<dyn KvUnit>, BoxError> {
    let path = join(&[root, &format!("{}.json", descriptor.name)]);
    let mut state = match read_file(&path).await? {
        Some(text) => parse(&text, &descriptor)?,
        None => UnitState::empty(&descriptor),
    };
    state.version = descriptor.version;
    Ok(Arc::new(SingleJsonUnit {
        life: Lifecycle::new(descriptor.name.clone(), on_close),
        descriptor,
        path,
        state: Arc::new(Mutex::new(state)),
    }))
}

struct SingleJsonUnit {
    descriptor: KvUnitDescriptor,
    path: String,
    state: Arc<Mutex<UnitState>>,
    life: Lifecycle,
}
impl SingleJsonUnit {
    fn records<'a>(
        &self,
        state: &'a mut UnitState,
        table: &str,
    ) -> Result<&'a mut IndexMap<String, Value>, BoxError> {
        state.tables.get_mut(table).ok_or_else(|| {
            plain(format!(
                "unit '{}' does not declare table '{table}'",
                self.descriptor.name
            ))
        })
    }
    fn remove_records(&self, table: &str, keys: &[String]) -> BackendFuture<'static, ()> {
        self.life.start(|| {
            let mut state = lock(&self.state);
            let records = self.records(&mut state, table)?;
            let mut removed = IndexMap::new();
            for key in keys {
                if let Some(value) = records.shift_remove(key) {
                    removed.insert(key.clone(), value);
                }
            }
            let text = (!removed.is_empty()).then(|| serialize(&self.descriptor.name, &state));
            let (path, state, table) = (self.path.clone(), self.state.clone(), table.to_owned());
            Ok(async move {
                if let Some(text) = text
                    && let Err(error) = write_atomic(&path, &text).await
                {
                    // Declared tables cannot disappear; rollback re-inserts at the end.
                    lock(&state)
                        .tables
                        .get_mut(&table)
                        .expect("declared table exists")
                        .extend(removed);
                    return Err(error);
                }
                Ok(())
            })
        })
    }
}
impl KvUnit for SingleJsonUnit {
    fn load_all(&self) -> BackendFuture<'_, KvSnapshot> {
        ready(self.life.checked(|| Ok(lock(&self.state).snapshot())))
    }
    fn put_record<'a>(
        &'a self,
        table: &'a str,
        key: &'a str,
        value: Value,
    ) -> BackendFuture<'a, ()> {
        self.life.start(|| {
            let mut state = lock(&self.state);
            let previous = self.records(&mut state, table)?.insert(key.into(), value);
            let text = serialize(&self.descriptor.name, &state);
            let (path, state, table, key) = (
                self.path.clone(),
                self.state.clone(),
                table.to_owned(),
                key.to_owned(),
            );
            Ok(async move {
                if let Err(error) = write_atomic(&path, &text).await {
                    let mut state = lock(&state);
                    // This declared table cannot disappear during a unit's life.
                    let records = state.tables.get_mut(&table).expect("declared table exists");
                    if let Some(previous) = previous {
                        records.insert(key, previous);
                    } else {
                        records.shift_remove(&key);
                    }
                    return Err(error);
                }
                Ok(())
            })
        })
    }
    fn delete_record<'a>(&'a self, table: &'a str, key: &'a str) -> BackendFuture<'a, ()> {
        self.remove_records(table, &[key.to_owned()])
    }
    fn delete_records<'a>(&'a self, table: &'a str, keys: &'a [String]) -> BackendFuture<'a, ()> {
        self.remove_records(table, keys)
    }
    fn set_global(&self, value: Value) -> BackendFuture<'_, ()> {
        self.life.start(|| {
            if !self.descriptor.has_global {
                return Err(plain(format!(
                    "unit '{}' does not declare a global slot",
                    self.descriptor.name
                )));
            }
            let mut state = lock(&self.state);
            let previous = std::mem::replace(&mut state.global, value);
            let text = serialize(&self.descriptor.name, &state);
            let (path, state) = (self.path.clone(), self.state.clone());
            Ok(async move {
                if let Err(error) = write_atomic(&path, &text).await {
                    lock(&state).global = previous;
                    return Err(error);
                }
                Ok(())
            })
        })
    }
    fn close(&self) -> BackendFuture<'_, ()> {
        self.life.close()
    }
}
