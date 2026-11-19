#[path = "../../hoh-storage/tests/contract.rs"]
mod contract;

use std::sync::Arc;

use hoh_storage::{BackendFuture, KvFacet, StorageBackend};
use hoh_storage_sqlite::{Config, JournalMode, SqliteStorageBackend};
use tempfile::TempDir;

struct KeptBackend {
    inner: SqliteStorageBackend,
    _directory: Arc<TempDir>,
}

impl StorageBackend for KeptBackend {
    fn kv(&self) -> Option<&dyn KvFacet> {
        self.inner.kv()
    }

    fn close(&self) -> BackendFuture<'_, ()> {
        self.inner.close()
    }
}

fn backend(path: &str, directory: Arc<TempDir>) -> Arc<dyn StorageBackend> {
    Arc::new(KeptBackend {
        inner: SqliteStorageBackend::new(Config {
            path: path.into(),
            journal_mode: JournalMode::Wal,
        }),
        _directory: directory,
    })
}

async fn create_harness() -> contract::KvBackendContractHarness {
    let directory = Arc::new(tempfile::tempdir().unwrap());
    let path = directory.path().join("storage.db");
    let path = path.to_str().unwrap().to_owned();
    contract::KvBackendContractHarness {
        backend: backend(&path, directory.clone()),
        reopen: Box::new(move || {
            let backend = backend(&path, directory.clone());
            Box::pin(async move { Ok(backend) })
        }),
    }
}

contract::run_kv_backend_contract!(contract, create_harness);
