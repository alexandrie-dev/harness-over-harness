//! Plain Error/TypeError thrown by the SDK presentation layer.
use hoh_error::{BoxError, SharedError};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum SdkServerError {
    #[error("{0}")]
    Error(String),
    #[error("{0}")]
    TypeError(String),
    #[error("SDK server teardown failed")]
    Aggregate { errors: Vec<SharedError> },
}
impl SdkServerError {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Error(_) => "Error",
            Self::TypeError(_) => "TypeError",
            Self::Aggregate { .. } => "AggregateError",
        }
    }
}

/// An unknown or unregistered `hoh/*` method. The transport answers it with JSON-RPC
/// `-32601` (method not found): the window falls back on exactly that code when the engine
/// does not implement a `hoh/*` method yet. Other failures are `-32603`.
#[derive(Debug, thiserror::Error)]
#[error("unknown HoH method: {0}")]
pub struct HohMethodNotFound(pub String);

/// Rust carrier for a JavaScript value thrown by an optional owner package.
/// The SDK ownership rule deliberately inspects only `name` and `code`.
#[derive(Clone, Debug)]
pub struct ForeignErrorValue(Value);

impl ForeignErrorValue {
    pub fn new(value: Value) -> Self {
        Self(value)
    }

    pub fn field(&self, name: &str) -> Option<&str> {
        self.0.get(name).and_then(Value::as_str)
    }
}

impl std::fmt::Display for ForeignErrorValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&hoh_util_values::js_json_stringify(&self.0))
    }
}

impl std::error::Error for ForeignErrorValue {}

pub(crate) fn error(message: impl Into<String>) -> BoxError {
    Box::new(SdkServerError::Error(message.into()))
}
pub(crate) fn type_error(message: impl Into<String>) -> BoxError {
    Box::new(SdkServerError::TypeError(message.into()))
}
