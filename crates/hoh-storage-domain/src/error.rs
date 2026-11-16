use hoh_error::{BoxError, SharedError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{any::Any, fmt};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DomainErrorCode {
    AlreadyOpen,
    FacetUnsupported,
    InvalidRecord,
    MissingKey,
    Closed,
}
impl DomainErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AlreadyOpen => "already-open",
            Self::FacetUnsupported => "facet-unsupported",
            Self::InvalidRecord => "invalid-record",
            Self::MissingKey => "missing-key",
            Self::Closed => "closed",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvalidRecordDetail {
    pub table: String,
    pub key: String,
}
#[derive(Clone, Debug, Default)]
pub struct DomainErrorOptions {
    pub cause: Option<SharedError>,
    pub detail: Option<InvalidRecordDetail>,
}
#[derive(Clone, Debug, thiserror::Error)]
#[error("{message}")]
pub struct DomainError {
    code: DomainErrorCode,
    message: String,
    pub detail: Option<InvalidRecordDetail>,
    #[source]
    cause: Option<SharedError>,
}
impl DomainError {
    pub const NAME: &'static str = "DomainError";
    pub fn new(code: DomainErrorCode, message: impl Into<String>) -> Self {
        Self::with_options(code, message, DomainErrorOptions::default())
    }
    pub fn with_options(
        code: DomainErrorCode,
        message: impl Into<String>,
        options: DomainErrorOptions,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            detail: options.detail,
            cause: options.cause,
        }
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
    pub fn cause(&self) -> Option<&SharedError> {
        self.cause.as_ref()
    }
}

/// Native Error messages for declaration mistakes and Rust type mismatches.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct PlainError(pub String);

/// Zod's issue-array message. Schema owners supply the same issues as their
/// source decoder; this type preserves their order and visible formatting.
#[derive(Clone, Debug)]
pub struct ZodError {
    pub issues: Vec<Value>,
}
impl fmt::Display for ZodError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hoh_util_values::js_json_stringify_pretty(
            &Value::Array(self.issues.clone()),
            2,
        ))
    }
}
impl std::error::Error for ZodError {}

pub(crate) fn plain(message: impl Into<String>) -> SharedError {
    SharedError::new(PlainError(message.into()))
}
pub(crate) fn panic_error(payload: Box<dyn Any + Send>) -> BoxError {
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("event listener panicked");
    Box::new(PlainError(message.into()))
}
pub(crate) fn js_error_string(error: &(dyn std::error::Error + 'static)) -> String {
    if let Some(shared) = error.downcast_ref::<SharedError>() {
        return js_error_string(shared.get_ref());
    }
    let name = if error.is::<ZodError>() {
        "ZodError"
    } else if error.is::<DomainError>() {
        "DomainError"
    } else if error.is::<hoh_storage::StorageError>() {
        "StorageError"
    } else if let Some(error) = hoh_error::HarnessError::of(error) {
        error.name()
    } else {
        "Error"
    };
    format!("{name}: {error}")
}
