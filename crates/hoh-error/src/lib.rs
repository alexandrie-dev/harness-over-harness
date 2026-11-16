//! One concrete type for every machine-routable harness failure.
use std::{any::Any, error::Error, fmt, sync::Arc};

pub type BoxError = Box<dyn Error + Send + Sync + 'static>;

/// An error value whose identity survives throwing, cloning and forwarding.
#[derive(Clone, Debug)]
pub struct SharedError(Arc<dyn Error + Send + Sync + 'static>);

impl SharedError {
    pub fn new(error: impl Into<BoxError>) -> Self {
        let error = error.into();
        match error.downcast::<Self>() {
            Ok(shared) => *shared,
            Err(error) => Self(Arc::from(error)),
        }
    }
    pub fn ptr_eq(&self, other: &SharedError) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn downcast_ref<T: Error + 'static>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }
    pub fn get_ref(&self) -> &(dyn Error + Send + Sync + 'static) {
        self.0.as_ref()
    }
    pub fn of<'a>(error: &'a (dyn Error + 'static)) -> Option<&'a SharedError> {
        error.downcast_ref()
    }
}
impl fmt::Display for SharedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.get_ref(), f)
    }
}
impl Error for SharedError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.0.source()
    }
}

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
        error
            .downcast_ref()
            .or_else(|| SharedError::of(error)?.downcast_ref())
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
