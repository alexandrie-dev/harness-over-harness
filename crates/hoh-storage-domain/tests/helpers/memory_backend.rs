use hoh_error::{BoxError, SharedError};
use hoh_storage::{
    BackendFuture, KvFacet, KvSnapshot, KvUnit, KvUnitDescriptor, StorageBackend, StorageError,
    StorageErrorCode,
};
use indexmap::{IndexMap, IndexSet};
use serde_json::Value;
use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

#[derive(Default)]
pub struct MemoryMediaPool {
    pub state: Mutex<PoolState>,
}
#[derive(Default)]
pub struct PoolState {
    pub media: IndexMap<String, KvSnapshot>,
    pub versions: IndexMap<String, u64>,
    pub fail_next_writes: u64,
}
impl PoolState {
    fn consume_failure(&mut self) -> Result<(), BoxError> {
        if self.fail_next_writes > 0 {
            self.fail_next_writes -= 1;
            Err("injected write failure".into())
        } else {
            Ok(())
        }
    }
}
pub struct Gate {
    pub entered: tokio::sync::Semaphore,
    pub release: tokio::sync::Semaphore,
}
impl Gate {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: tokio::sync::Semaphore::new(0),
            release: tokio::sync::Semaphore::new(0),
        })
    }
}
#[derive(Default)]
pub struct Control {
    pub batches: AtomicU64,
    pub singles: AtomicU64,
    pub closes: AtomicU64,
    pub poison: Mutex<Option<SharedError>>,
    pub sparse: AtomicBool,
    pub backup: AtomicBool,
    pub gate: Mutex<Option<Arc<Gate>>>,
}
pub struct MemoryStorageBackend {
    pub pool: Arc<MemoryMediaPool>,
    pub control: Arc<Control>,
    open: Arc<Mutex<HashSet<String>>>,
    closed: AtomicBool,
}
impl MemoryStorageBackend {
    pub fn new(pool: Arc<MemoryMediaPool>) -> Arc<Self> {
        Arc::new(Self {
            pool,
            control: Arc::new(Control::default()),
            open: Arc::default(),
            closed: AtomicBool::new(false),
        })
    }
}
impl StorageBackend for MemoryStorageBackend {
    fn kv(&self) -> Option<&dyn KvFacet> {
        Some(self)
    }
    fn close(&self) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            self.closed.store(true, Ordering::Release);
            self.open.lock().unwrap().clear();
            Ok(())
        })
    }
}
impl KvFacet for MemoryStorageBackend {
    fn open(&self, descriptor: KvUnitDescriptor) -> BackendFuture<'_, Arc<dyn KvUnit>> {
        Box::pin(async move {
            if self.closed.load(Ordering::Acquire) {
                return Err(StorageError::new(
                    StorageErrorCode::Closed,
                    "memory backend is closed",
                )
                .into());
            }
            let mut open = self.open.lock().unwrap();
            if open.contains(&descriptor.name) {
                return Err(format!(
                    "memory unit '{}' is already open (double-open is a caller bug)",
                    descriptor.name
                )
                .into());
            }
            let mut state = self.pool.state.lock().unwrap();
            if let Some(stamped) = state.versions.get(&descriptor.name) {
                if *stamped != descriptor.version
                    && !descriptor
                        .compatible_versions
                        .as_ref()
                        .is_some_and(|versions| versions.contains(stamped))
                {
                    return Err(StorageError::new(
                        StorageErrorCode::VersionMismatch,
                        format!(
                            "memory unit '{}' is stamped v{stamped}, descriptor wants v{}",
                            descriptor.name, descriptor.version
                        ),
                    )
                    .into());
                }
            } else {
                state
                    .versions
                    .insert(descriptor.name.clone(), descriptor.version);
            }
            state
                .media
                .entry(descriptor.name.clone())
                .or_insert_with(|| KvSnapshot {
                    tables: IndexMap::new(),
                    global: Value::Null,
                });
            open.insert(descriptor.name.clone());
            Ok(Arc::new(MemoryKvUnit {
                pool: self.pool.clone(),
                control: self.control.clone(),
                open: self.open.clone(),
                descriptor,
                closed: AtomicBool::new(false),
            }) as Arc<dyn KvUnit>)
        })
    }
}
struct MemoryKvUnit {
    pool: Arc<MemoryMediaPool>,
    control: Arc<Control>,
    open: Arc<Mutex<HashSet<String>>>,
    descriptor: KvUnitDescriptor,
    closed: AtomicBool,
}
impl MemoryKvUnit {
    fn ensure_open(&self) -> Result<(), BoxError> {
        if self.closed.load(Ordering::Acquire) {
            Err(StorageError::new(
                StorageErrorCode::Closed,
                format!("memory unit '{}' is closed", self.descriptor.name),
            )
            .into())
        } else {
            Ok(())
        }
    }
    async fn wait_gate(&self) {
        let gate = self.control.gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.entered.add_permits(1);
            gate.release.acquire().await.unwrap().forget();
        }
    }
}
impl KvUnit for MemoryKvUnit {
    fn load_all(&self) -> BackendFuture<'_, KvSnapshot> {
        Box::pin(async move {
            self.ensure_open()?;
            let state = self.pool.state.lock().unwrap();
            let medium = &state.media[&self.descriptor.name];
            let tables = if self.control.sparse.load(Ordering::Acquire) {
                IndexMap::new()
            } else {
                self.descriptor
                    .tables
                    .iter()
                    .map(|table| {
                        (
                            table.clone(),
                            medium.tables.get(table).cloned().unwrap_or_default(),
                        )
                    })
                    .collect()
            };
            Ok(KvSnapshot {
                tables,
                global: medium.global.clone(),
            })
        })
    }
    fn put_record<'a>(
        &'a self,
        table: &'a str,
        key: &'a str,
        value: Value,
    ) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            self.ensure_open()?;
            self.wait_gate().await;
            let mut state = self.pool.state.lock().unwrap();
            state.consume_failure()?;
            state
                .media
                .get_mut(&self.descriptor.name)
                .unwrap()
                .tables
                .entry(table.into())
                .or_default()
                .insert(key.into(), value);
            state
                .versions
                .insert(self.descriptor.name.clone(), self.descriptor.version);
            Ok(())
        })
    }
    fn delete_record<'a>(&'a self, table: &'a str, key: &'a str) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            self.control.singles.fetch_add(1, Ordering::Relaxed);
            self.delete_records(table, &[key.into()]).await
        })
    }
    fn delete_records<'a>(&'a self, table: &'a str, keys: &'a [String]) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            self.ensure_open()?;
            self.control.batches.fetch_add(1, Ordering::Relaxed);
            if let Some(error) = self.control.poison.lock().unwrap().clone() {
                return Err(Box::new(error) as BoxError);
            }
            let mut state = self.pool.state.lock().unwrap();
            let Some(records) = state.media[&self.descriptor.name].tables.get(table) else {
                return Ok(());
            };
            let present = keys
                .iter()
                .filter(|key| records.contains_key(*key))
                .cloned()
                .collect::<IndexSet<_>>();
            if present.is_empty() {
                return Ok(());
            }
            state.consume_failure()?;
            let records = state
                .media
                .get_mut(&self.descriptor.name)
                .unwrap()
                .tables
                .get_mut(table)
                .unwrap();
            for key in present {
                records.shift_remove(&key);
            }
            state
                .versions
                .insert(self.descriptor.name.clone(), self.descriptor.version);
            Ok(())
        })
    }
    fn backup_record<'a>(
        &'a self,
        table: &'a str,
        key: &'a str,
    ) -> Option<BackendFuture<'a, String>> {
        self.control.backup.load(Ordering::Acquire).then(|| {
            Box::pin(async move {
                self.ensure_open()?;
                self.pool
                    .state
                    .lock()
                    .unwrap()
                    .media
                    .get_mut(&self.descriptor.name)
                    .unwrap()
                    .tables
                    .get_mut(table)
                    .unwrap()
                    .shift_remove(key);
                Ok(format!("{table}/{key}.json.bak.1"))
            }) as BackendFuture<'a, String>
        })
    }
    fn set_global(&self, value: Value) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            self.ensure_open()?;
            let mut state = self.pool.state.lock().unwrap();
            state.consume_failure()?;
            state.media.get_mut(&self.descriptor.name).unwrap().global = value;
            state
                .versions
                .insert(self.descriptor.name.clone(), self.descriptor.version);
            Ok(())
        })
    }
    fn close(&self) -> BackendFuture<'_, ()> {
        Box::pin(async move {
            if !self.closed.swap(true, Ordering::AcqRel) {
                self.open.lock().unwrap().remove(&self.descriptor.name);
                self.control.closes.fetch_add(1, Ordering::Relaxed);
            }
            Ok(())
        })
    }
}
pub struct NoKv;
impl StorageBackend for NoKv {
    fn close(&self) -> BackendFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}
