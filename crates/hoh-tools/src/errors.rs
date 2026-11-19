//! Plain (non-`HarnessError`) failures of the package: the source throws bare `Error`, `TypeError`
//! and `RangeError` values for registration and scheduling misuse.

use std::panic::{AssertUnwindSafe, catch_unwind};

use hoh_error::BoxError;

/// A bare `Error`, `TypeError` or `RangeError` of the source.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ToolsError {
    #[error("{0}")]
    Error(String),
    #[error("{0}")]
    TypeError(String),
    #[error("{0}")]
    RangeError(String),
}

impl ToolsError {
    /// The JavaScript class name of the failure.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Error(_) => "Error",
            Self::TypeError(_) => "TypeError",
            Self::RangeError(_) => "RangeError",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Error(message) | Self::TypeError(message) | Self::RangeError(message) => message,
        }
    }
}

/// The text a panic payload carries, as the error a thrown JavaScript value would be.
pub(crate) fn panic_error(payload: Box<dyn std::any::Any + Send>) -> BoxError {
    let text = payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned());
    match text {
        Some(text) => text.into(),
        None => "callback panicked".into(),
    }
}

/// Run a user callback, turning a panic into the error a throwing JavaScript callback produces.
pub(crate) fn catch_panic<T>(f: impl FnOnce() -> T) -> Result<T, BoxError> {
    catch_unwind(AssertUnwindSafe(f)).map_err(panic_error)
}
