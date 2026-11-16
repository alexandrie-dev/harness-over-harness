//! Storage error vocabulary shared by the hub and backend implementers.
use hoh_error::BoxError;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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

/// The source extends plain Error, so this remains distinct from HarnessError.
#[derive(Clone, Debug, thiserror::Error)]
#[error("{message}")]
pub struct StorageError {
    code: StorageErrorCode,
    message: String,
    #[source]
    cause: Option<Arc<dyn std::error::Error + Send + Sync>>,
}

impl StorageError {
    pub const NAME: &'static str = "StorageError";
    pub fn new(code: StorageErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            cause: None,
        }
    }
    pub fn with_cause(mut self, cause: impl Into<BoxError>) -> Self {
        self.cause = Some(Arc::from(cause.into()));
        self
    }
    pub fn name(&self) -> &'static str {
        Self::NAME
    }
    pub fn code(&self) -> &'static str {
        self.code.as_str()
    }
    pub fn message(&self) -> &str {
        &self.message
    }
}

pub type StorageResult<T> = Result<T, StorageError>;
