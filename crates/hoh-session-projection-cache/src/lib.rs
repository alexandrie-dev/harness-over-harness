//! Durable, fail-soft checkpoints for the session-projection registry.

mod service;
mod spec;

pub use service::{Config, SessionProjectionCachePlugin, SessionProjectionCacheService};
pub use spec::{
    CheckpointIdentity, CheckpointRecord, checkpoint_identity, checkpoint_record, checkpoint_row,
    projection_cache_domain_spec,
};

use hoh_error::BoxError;
use std::sync::{Mutex, MutexGuard};

pub type Result<T> = std::result::Result<T, BoxError>;

#[derive(Debug, thiserror::Error)]
pub enum ProjectionCacheError {
    #[error("{0}")]
    Message(String),
    #[error("{0}")]
    TypeError(String),
}

impl ProjectionCacheError {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Message(_) => "Error",
            Self::TypeError(_) => "TypeError",
        }
    }
}

fn error(message: impl Into<String>) -> BoxError {
    Box::new(ProjectionCacheError::Message(message.into()))
}

fn type_error(message: impl Into<String>) -> BoxError {
    Box::new(ProjectionCacheError::TypeError(message.into()))
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

hoh_kernel::service!(
    pub SessionProjectionCache: SessionProjectionCacheService = "sessionProjectionCache"
);
