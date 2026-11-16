//! Port of @deepseek-ai/dsh-storage (packages/storage/storage).
//! Storage hub: named backends and extensible data-form facilities, without I/O.
pub mod backend;
pub mod error;
pub mod registry;
pub use backend::{
    BackendFuture, KvFacet, KvLayout, KvSnapshot, KvUnit, KvUnitDescriptor, StorageBackend,
    UNIT_NAME_RE,
};
pub use error::{StorageError, StorageErrorCode};
pub use registry::BackendRegistry;

use error::StorageResult;
use hoh_error::BoxError;
use hoh_kernel::{Context, plugin, service};
use indexmap::IndexMap;
use std::{
    any::Any,
    sync::{Arc, Mutex, MutexGuard},
};

pub fn storage_backend_service_key(name: &str) -> String {
    format!("storage.backend.{name}")
}

/// Open extension map: form owners store their own concrete facilities.
pub type StorageForms = IndexMap<String, Arc<dyn Any + Send + Sync>>;

#[derive(Clone, Default)]
pub struct StorageService {
    pub backend: BackendRegistry,
    forms: Arc<Mutex<StorageForms>>,
}

impl StorageService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mount(
        &self,
        form: impl Into<String>,
        facility: Arc<dyn Any + Send + Sync>,
    ) -> StorageResult<impl Fn() + Send + Sync + 'static> {
        let form = form.into();
        let mut forms = lock(&self.forms);
        if forms.contains_key(&form) {
            return Err(StorageError::new(
                StorageErrorCode::DuplicateMount,
                format!("storage form '{form}' is already mounted"),
            ));
        }
        forms.insert(form.clone(), facility.clone());
        let storage = self.clone();
        Ok(move || {
            let mut forms = lock(&storage.forms);
            if forms
                .get(&form)
                .is_some_and(|current| Arc::ptr_eq(current, &facility))
            {
                forms.shift_remove(&form);
            }
        })
    }

    pub fn form(&self, form: &str) -> StorageResult<Arc<dyn Any + Send + Sync>> {
        lock(&self.forms).get(form).cloned().ok_or_else(|| {
            StorageError::new(
                StorageErrorCode::FormNotMounted,
                format!("storage form '{form}' is not mounted"),
            )
        })
    }

    pub fn domain(&self) -> StorageResult<Arc<dyn Any + Send + Sync>> {
        self.form("domain")
    }
}

service!(pub Storage: StorageService = "storage");

#[plugin(StoragePlugin, name = "storage")]
pub async fn apply(ctx: Context) -> Result<(), BoxError> {
    ctx.provide::<Storage>(Arc::new(StorageService::new()))
        .await?;
    Ok(())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
