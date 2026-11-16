#[path = "contract.rs"]
#[allow(dead_code)] // These self-tests use only the shared fixture/error helpers.
mod contract;

use hoh_error::HarnessError;
use hoh_storage::{BackendFuture, KvUnitDescriptor, StorageError, StorageErrorCode};
use serde_json::json;

#[test]
fn contract_descriptor_preserves_source_fields_and_optional_defaults() {
    let descriptor = contract::descriptor();
    let value = serde_json::to_value(&descriptor).unwrap();
    assert_eq!(
        value,
        json!({
            "name": "contract_unit", "version": 3,
            "tables": ["alpha", "beta"], "hasGlobal": true
        })
    );
    assert_eq!(
        value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["name", "version", "tables", "hasGlobal"]
    );
    let restored: KvUnitDescriptor = serde_json::from_value(value).unwrap();
    assert_eq!(restored, descriptor);
    assert!(restored.layout.is_none());
    assert!(restored.compatible_versions.is_none());
}

#[tokio::test]
async fn contract_error_checks_recognize_backend_future_failures() {
    for code in [StorageErrorCode::VersionMismatch, StorageErrorCode::Closed] {
        let result: BackendFuture<'_, ()> = Box::pin(async move {
            Err(StorageError::new(code, "backend diagnostic")
                .with_cause(std::io::Error::other("medium failure"))
                .into())
        });
        let boxed = result.await.unwrap_err();
        assert!(HarnessError::of(boxed.as_ref()).is_none());
        let error = contract::storage_error(boxed.as_ref());
        assert_eq!(error.name(), "StorageError");
        assert_eq!(error.code(), code.as_str());
        assert_eq!(error.message(), "backend diagnostic");
        assert_eq!(
            std::error::Error::source(error).unwrap().to_string(),
            "medium failure"
        );
    }
}
