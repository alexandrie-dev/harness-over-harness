use crate::diagnostic::{McpLocalError, McpLocalErrorCode};
use hoh_error::BoxError;
use std::{future::Future, time::Duration};

pub async fn with_deadline<T>(
    future: impl Future<Output = Result<T, BoxError>>,
    remaining_ms: u64,
    timeout_message: impl Into<String>,
) -> Result<T, BoxError> {
    match tokio::time::timeout(Duration::from_millis(remaining_ms.max(1)), future).await {
        Ok(result) => result,
        Err(_) => Err(Box::new(McpLocalError::new(
            timeout_message,
            McpLocalErrorCode::Timeout,
        ))),
    }
}
