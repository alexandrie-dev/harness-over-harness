//! Stable, machine-routable harness failures and standard Rust cause chains.
use std::error::Error;

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct HarnessError {
    pub name: String,
    pub code: String,
    pub message: String,
    #[source]
    pub cause: Option<Box<dyn Error + Send + Sync>>,
}

impl HarnessError {
    pub fn new(message: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            name: "HarnessError".into(),
            code: code.into(),
            message: message.into(),
            cause: None,
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn with_cause(mut self, cause: impl Error + Send + Sync + 'static) -> Self {
        self.cause = Some(Box::new(cause));
        self
    }
}

/// Crate-local errors embed a `HarnessError` and expose it at routing boundaries.
/// `Error::source` remains available for arbitrary Rust causes.
pub trait CodedError: Error + Send + Sync {
    fn harness_error(&self) -> &HarnessError;
    fn code(&self) -> &str {
        &self.harness_error().code
    }
    fn message(&self) -> &str {
        &self.harness_error().message
    }
    fn name(&self) -> &str {
        &self.harness_error().name
    }
}

impl CodedError for HarnessError {
    fn harness_error(&self) -> &HarnessError {
        self
    }
}
