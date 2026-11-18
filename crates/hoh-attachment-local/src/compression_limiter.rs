//! FIFO instance-owned concurrency limiter.
use hoh_error::{BoxError, SharedError};
use std::sync::Arc;
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct CompressionLimiter {
    pub concurrency: usize,
    slots: Arc<Semaphore>,
}
impl CompressionLimiter {
    pub fn new(concurrency: usize) -> Self {
        Self {
            concurrency,
            slots: Arc::new(Semaphore::new(concurrency)),
        }
    }
    pub async fn run<T>(
        &self,
        task: impl AsyncFnOnce() -> Result<T, BoxError>,
    ) -> Result<T, BoxError> {
        let _permit = self.slots.acquire().await?;
        task().await
    }
}
pub enum CompressionRejection {
    Error(BoxError),
    Value(serde_json::Value),
}
#[derive(Debug, thiserror::Error)]
#[error("Image compression task rejected with a non-Error value.")]
pub struct NonErrorCompressionFailure {
    pub cause: serde_json::Value,
}
pub fn compression_failure(reason: CompressionRejection) -> SharedError {
    match reason {
        CompressionRejection::Error(error) => SharedError::new(error),
        CompressionRejection::Value(cause) => {
            SharedError::new(NonErrorCompressionFailure { cause })
        }
    }
}
