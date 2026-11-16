//! One concrete type for every machine-routable harness failure.
use std::{any::Any, error::Error, fmt, sync::Arc};

pub type BoxError = Box<dyn Error + Send + Sync + 'static>;

#[derive(Debug, Clone)]
pub struct HarnessError {
    name: &'static str,
    code: String,
    message: String,
    cause: Option<Arc<dyn Error + Send + Sync>>,
    detail: Option<Arc<dyn Any + Send + Sync>>,
}

impl HarnessError {
    pub fn new(message: impl Into<String>, code: impl Into<String>) -> Self {
        Self::named("HarnessError", message, code)
    }
    pub fn named(name: &'static str, message: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            name,
            code: code.into(),
            message: message.into(),
            cause: None,
            detail: None,
        }
    }
    pub fn with_cause(mut self, cause: impl Into<BoxError>) -> Self {
        self.cause = Some(Arc::from(cause.into()));
        self
    }
    pub fn with_detail<T: Any + Send + Sync>(mut self, detail: T) -> Self {
        self.detail = Some(Arc::new(detail));
        self
    }
    pub fn name(&self) -> &str {
        self.name
    }
    pub fn code(&self) -> &str {
        &self.code
    }
    pub fn message(&self) -> &str {
        &self.message
    }
    pub fn cause(&self) -> Option<&(dyn Error + Send + Sync + 'static)> {
        self.cause.as_deref()
    }
    pub fn detail<T: Any>(&self) -> Option<&T> {
        self.detail.as_deref()?.downcast_ref()
    }
    pub fn is_named(&self, name: &str) -> bool {
        self.name == name
    }
    pub fn of<'a>(error: &'a (dyn Error + 'static)) -> Option<&'a HarnessError> {
        error.downcast_ref()
    }
}
impl fmt::Display for HarnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl Error for HarnessError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.cause().map(|cause| cause as &(dyn Error + 'static))
    }
}
