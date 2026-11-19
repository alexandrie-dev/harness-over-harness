//! Eager projection folds and retained readers over immutable Session logs.
mod owner;
mod registration;
mod registry;
mod shim;
mod state_json;
pub mod types;
use hoh_error::BoxError;
pub use registration::{ProjectionHandle, ProjectionRegistration};
pub use registry::SessionProjectionRegistry;
use serde_json::Value;
use std::sync::{Mutex, MutexGuard};
pub use types::*;
pub type Result<T> = std::result::Result<T, BoxError>;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ProjectionError(pub String);
impl ProjectionError {
    pub fn name(&self) -> &'static str {
        "Error"
    }
}
pub(crate) fn error(message: impl Into<String>) -> BoxError {
    Box::new(ProjectionError(message.into()))
}
pub(crate) fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
pub(crate) fn quote(value: &str) -> String {
    hoh_util_values::js_json_stringify(&Value::String(value.into()))
}
hoh_kernel::service!(pub SessionProjections: SessionProjectionRegistry = "sessionProjections");

#[cfg(test)]
#[path = "../tests/compatibility.rs"]
mod compatibility_tests;
#[cfg(test)]
#[path = "../tests/registration_reader.rs"]
mod registration_reader_tests;
#[cfg(test)]
#[path = "../tests/registry.rs"]
mod registry_tests;
#[cfg(test)]
#[path = "../tests/summary.rs"]
mod summary_tests;
#[cfg(test)]
#[path = "../tests/support.rs"]
mod test_support;
