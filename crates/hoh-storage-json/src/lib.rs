//! Port of @deepseek-ai/dsh-storage-json (packages/storage/storage-json).
pub mod atomic;
pub mod format;
pub mod per_record_unit;
mod runtime;
pub mod single_unit;

use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Service, Undo, plugin, service};
use hoh_storage::{
    BackendFuture, KvFacet, KvLayout, KvUnit, KvUnitDescriptor, Storage, StorageBackend,
    StorageError, StorageErrorCode, UNIT_NAME_RE,
};
use indexmap::IndexMap;
use runtime::{drain, lock, plain, ready, spawn};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Config {
    /// Directory holding one `<unit>.json` file (or `<unit>/` tree) per unit.
    pub root: String,
}

#[derive(Default)]
struct BackendState {
    open: IndexMap<String, Arc<dyn KvUnit>>,
    opening: IndexMap<String, watch::Receiver<bool>>,
    closed: bool,
}
struct BackendInner {
    root: String,
    state: Mutex<BackendState>,
}
#[derive(Clone)]
pub struct JsonStorageBackend {
    inner: Arc<BackendInner>,
}
impl JsonStorageBackend {
    pub fn new(root: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(BackendInner {
                root: root.into(),
                state: Mutex::default(),
            }),
        }
    }
}
impl KvFacet for JsonStorageBackend {
    fn open(&self, descriptor: KvUnitDescriptor) -> BackendFuture<'_, Arc<dyn KvUnit>> {
        let (done, pending) = watch::channel(false);
        {
            let mut state = lock(&self.inner.state);
            if state.closed {
                return ready(Err(backend_closed()));
            }
            if let Err(error) = validate_descriptor(&descriptor) {
                return ready(Err(error.into()));
            }
            if state.open.contains_key(&descriptor.name)
                || state.opening.contains_key(&descriptor.name)
            {
                return ready(Err(plain(format!(
                    "unit '{}' is already open; a unit has exactly one live handle",
                    descriptor.name
                ))));
            }
            state.opening.insert(descriptor.name.clone(), pending);
        }
        let inner = self.inner.clone();
        spawn(async move {
            let result = inner.open_unit(descriptor.clone()).await;
            lock(&inner.state).opening.shift_remove(&descriptor.name);
            let _ = done.send(true);
            result
        })
    }
}
impl BackendInner {
    async fn open_unit(
        self: &Arc<Self>,
        descriptor: KvUnitDescriptor,
    ) -> Result<Arc<dyn KvUnit>, BoxError> {
        atomic::mkdir(&self.root).await?;
        let weak = Arc::downgrade(self);
        let name = descriptor.name.clone();
        let on_close: Arc<dyn Fn() -> Result<(), BoxError> + Send + Sync> = Arc::new(move || {
            if let Some(inner) = weak.upgrade() {
                lock(&inner.state).open.shift_remove(&name);
            }
            Ok(())
        });
        let unit = if descriptor.layout == Some(KvLayout::PerRecord) {
            per_record_unit::open_per_record_unit(descriptor.clone(), &self.root, on_close).await?
        } else {
            single_unit::open_single_unit(descriptor.clone(), &self.root, on_close).await?
        };
        {
            let mut state = lock(&self.state);
            if !state.closed {
                state.open.insert(descriptor.name, unit.clone());
                return Ok(unit);
            }
        }
        unit.close().await?;
        Err(backend_closed())
    }
}
impl StorageBackend for JsonStorageBackend {
    fn kv(&self) -> Option<&dyn KvFacet> {
        Some(self)
    }
    fn close(&self) -> BackendFuture<'_, ()> {
        let jobs = {
            let mut state = lock(&self.inner.state);
            state.closed = true;
            state.opening.values().cloned().collect()
        };
        let inner = self.inner.clone();
        spawn(async move {
            drain(jobs).await;
            let units: Vec<_> = lock(&inner.state).open.values().cloned().collect();
            for unit in units {
                unit.close().await?;
            }
            Ok(())
        })
    }
}
fn backend_closed() -> BoxError {
    Box::new(StorageError::new(
        StorageErrorCode::Closed,
        "json backend is closed",
    ))
}
fn validate_descriptor(descriptor: &KvUnitDescriptor) -> Result<(), StorageError> {
    if !UNIT_NAME_RE.is_match(&descriptor.name) {
        return Err(StorageError::new(
            StorageErrorCode::MalformedMedium,
            format!("invalid unit name '{}'", descriptor.name),
        ));
    }
    for table in &descriptor.tables {
        if !UNIT_NAME_RE.is_match(table) {
            return Err(StorageError::new(
                StorageErrorCode::MalformedMedium,
                format!("invalid table name '{table}' in unit '{}'", descriptor.name),
            ));
        }
    }
    Ok(())
}

#[derive(Inject)]
pub struct Deps {
    storage: Service<Storage>,
}
service!(pub JsonBackend: JsonStorageBackend = "storage.backend.json");

#[plugin(StorageJsonPlugin, name = "storage-json")]
pub async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
    let backend = Arc::new(JsonStorageBackend::new(config.get().root.clone()));
    let cleanup = backend.clone();
    ctx.try_effect("json storage backend", |undos| -> Result<(), BoxError> {
        let unregister = deps.storage.backend.register("json", backend.clone())?;
        undos.push(Undo::future(move || async move {
            unregister();
            cleanup.close().await
        }))?;
        Ok(())
    })?;
    ctx.provide::<JsonBackend>(backend).await?;
    Ok(())
}

#[cfg(test)]
#[path = "../tests/per_record_batch.rs"]
mod per_record_batch_tests;
