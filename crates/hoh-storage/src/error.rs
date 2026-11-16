//! Storage error vocabulary shared by the hub and backend implementers.
use hoh_error::HarnessError;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StorageErrorCode {
    BackendNotFound,
    FormNotMounted,
    DuplicateBackend,
    DuplicateMount,
    VersionMismatch,
    MalformedMedium,
    UnknownState,
    Closed,
}

impl StorageErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BackendNotFound => "backend-not-found",
            Self::FormNotMounted => "form-not-mounted",
            Self::DuplicateBackend => "duplicate-backend",
            Self::DuplicateMount => "duplicate-mount",
            Self::VersionMismatch => "version-mismatch",
            Self::MalformedMedium => "malformed-medium",
            Self::UnknownState => "unknown-state",
            Self::Closed => "closed",
        }
    }
}

/// Constructor for the source error class; thrown values use the shared type.
pub struct StorageError;

impl StorageError {
    pub const NAME: &'static str = "StorageError";
    #[allow(clippy::new_ret_no_self)] // Error-class factory required by PORTING.md §7.
    pub fn new(code: StorageErrorCode, message: impl Into<String>) -> HarnessError {
        HarnessError::named(Self::NAME, message, code.as_str())
    }
}

pub type StorageResult<T> = Result<T, HarnessError>;
