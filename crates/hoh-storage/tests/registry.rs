use hoh_kernel::Kernel;
use hoh_storage::{
    BackendFuture, BackendRegistry, Storage, StorageBackend, StorageError, StoragePlugin,
    storage_backend_service_key,
};
use std::sync::Arc;

struct FakeBackend;
impl StorageBackend for FakeBackend {
    fn close(&self) -> BackendFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}
fn fake_backend() -> Arc<dyn StorageBackend> {
    Arc::new(FakeBackend)
}

#[test]
fn registers_resolves_and_disposes_names() {
    let registry = BackendRegistry::new();
    let backend = fake_backend();
    let dispose = registry.register("json", backend.clone()).unwrap();
    assert!(Arc::ptr_eq(&registry.get("json").unwrap(), &backend));
    assert_eq!(registry.names(), ["json"]);
    dispose();
    assert!(registry.names().is_empty());
    let error = registry.get("json").err().unwrap();
    assert_eq!(error.code(), "backend-not-found");
    assert_eq!(error.name(), "StorageError");
    assert_eq!(
        error.to_string(),
        "storage backend 'json' is not registered (registered: none)"
    );
}

#[test]
fn rejects_duplicate_names() {
    let registry = BackendRegistry::new();
    let _dispose = registry.register("json", fake_backend()).unwrap();
    let error = registry.register("json", fake_backend()).err().unwrap();
    assert_eq!(error.code(), "duplicate-backend");
    assert_eq!(
        error.message(),
        "storage backend 'json' is already registered"
    );
}

#[test]
fn derives_stable_lifecycle_service_keys_for_named_backends() {
    assert_eq!(storage_backend_service_key("json"), "storage.backend.json");
    assert_eq!(
        storage_backend_service_key("tenant-a"),
        "storage.backend.tenant-a"
    );
}

#[tokio::test]
async fn mounts_on_the_context_and_exposes_registry_plus_form_mounting() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fiber = ctx.plugin::<StoragePlugin>(serde_json::json!({})).unwrap();
    fiber.settled().await.unwrap();
    let storage = ctx.get::<Storage>().unwrap();
    let facility: Arc<dyn std::any::Any + Send + Sync> =
        Arc::new(serde_json::json!({"marker":true}));
    let dispose = storage.mount("domain", facility.clone()).unwrap();
    assert!(Arc::ptr_eq(&storage.form("domain").unwrap(), &facility));
    assert!(Arc::ptr_eq(&storage.domain().unwrap(), &facility));
    let error = storage.mount("domain", facility).err().unwrap();
    assert_eq!(error.code(), "duplicate-mount");
    assert_eq!(error.message(), "storage form 'domain' is already mounted");
    dispose();
    assert_eq!(
        storage.form("domain").err().unwrap().code(),
        "form-not-mounted"
    );
    assert_eq!(storage.domain().err().unwrap().code(), "form-not-mounted");
    kernel.shutdown().await;
}

#[tokio::test]
async fn ignores_a_stale_disposer_after_dispose_and_re_mount_re_register() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fiber = ctx.plugin::<StoragePlugin>(serde_json::json!({})).unwrap();
    fiber.settled().await.unwrap();
    let storage = ctx.get::<Storage>().unwrap();
    let first = Arc::new(serde_json::json!({"first":true}));
    let second: Arc<dyn std::any::Any + Send + Sync> = Arc::new(serde_json::json!({"second":true}));
    let stale_mount = storage.mount("domain", first).unwrap();
    stale_mount();
    let _unmount = storage.mount("domain", second.clone()).unwrap();
    stale_mount();
    assert!(Arc::ptr_eq(&storage.form("domain").unwrap(), &second));
    let backend_a = fake_backend();
    let backend_b = fake_backend();
    let stale_register = storage.backend.register("json", backend_a).unwrap();
    stale_register();
    let _unregister = storage.backend.register("json", backend_b.clone()).unwrap();
    stale_register();
    assert!(Arc::ptr_eq(
        &storage.backend.get("json").unwrap(),
        &backend_b
    ));
    kernel.shutdown().await;
}

#[test]
fn preserves_diagnostic_name_order_and_error_cause() {
    let registry = BackendRegistry::new();
    let _first = registry.register("z", fake_backend()).unwrap();
    let remove = registry.register("a", fake_backend()).unwrap();
    let _third = registry.register("b", fake_backend()).unwrap();
    assert_eq!(
        registry.get("unknown").err().unwrap().message(),
        "storage backend 'unknown' is not registered (registered: z, a, b)"
    );
    remove();
    let _last = registry.register("a", fake_backend()).unwrap();
    assert_eq!(registry.names(), ["z", "b", "a"]);
    let error = StorageError::new(hoh_storage::StorageErrorCode::Closed, "closed")
        .with_cause(std::io::Error::other("cause"));
    assert_eq!(
        std::error::Error::source(&error).unwrap().to_string(),
        "cause"
    );
}
