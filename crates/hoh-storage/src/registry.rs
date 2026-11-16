//! Named backend registry. Unregistering never closes the medium.
use crate::error::StorageResult;
use crate::{StorageBackend, StorageError, StorageErrorCode};
use indexmap::IndexMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct BackendRegistry {
    backends: Arc<Mutex<IndexMap<String, Arc<dyn StorageBackend>>>>,
}

impl BackendRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &self,
        name: impl Into<String>,
        backend: Arc<dyn StorageBackend>,
    ) -> StorageResult<impl Fn() + Send + Sync + 'static> {
        let name = name.into();
        let mut backends = crate::lock(&self.backends);
        if backends.contains_key(&name) {
            return Err(StorageError::new(
                StorageErrorCode::DuplicateBackend,
                format!("storage backend '{name}' is already registered"),
            ));
        }
        backends.insert(name.clone(), backend.clone());
        let registry = self.clone();
        Ok(move || {
            let mut backends = crate::lock(&registry.backends);
            if backends
                .get(&name)
                .is_some_and(|current| Arc::ptr_eq(current, &backend))
            {
                backends.shift_remove(&name);
            }
        })
    }

    pub fn get(&self, name: &str) -> StorageResult<Arc<dyn StorageBackend>> {
        let backends = crate::lock(&self.backends);
        backends.get(name).cloned().ok_or_else(|| {
            let names = backends.keys().cloned().collect::<Vec<_>>().join(", ");
            StorageError::new(
                StorageErrorCode::BackendNotFound,
                format!(
                    "storage backend '{name}' is not registered (registered: {})",
                    if names.is_empty() { "none" } else { &names }
                ),
            )
        })
    }

    pub fn names(&self) -> Vec<String> {
        crate::lock(&self.backends).keys().cloned().collect()
    }
}
