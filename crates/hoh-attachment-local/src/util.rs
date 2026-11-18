use hoh_attachment::{AttachmentError, AttachmentErrorCode};
use hoh_error::{BoxError, HarnessError};
use hoh_timeout::AbortSignal;
use sha2::{Digest, Sha256};
use std::{
    io,
    sync::{Mutex, MutexGuard},
};

pub(crate) fn error(message: impl Into<String>, code: AttachmentErrorCode) -> HarnessError {
    AttachmentError::new(message, code)
}
pub(crate) fn lock<T>(state: &Mutex<T>) -> MutexGuard<'_, T> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
pub(crate) fn digest(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}
pub(crate) fn hex(data: &[u8]) -> String {
    data.iter().map(|byte| format!("{byte:02x}")).collect()
}
pub(crate) fn check(signal: Option<&AbortSignal>) -> Result<(), BoxError> {
    if let Some(signal) = signal {
        signal.throw_if_aborted()?;
    }
    Ok(())
}
pub(crate) async fn cancellable<T>(
    signal: Option<&AbortSignal>,
    future: impl Future<Output = Result<T, io::Error>>,
) -> Result<T, BoxError> {
    check(signal)?;
    if let Some(signal) = signal {
        tokio::select! { biased; reason = signal.cancelled() => Err(reason.into()), result = future => Ok(result?) }
    } else {
        Ok(future.await?)
    }
}
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub(crate) struct FsError {
    message: String,
    #[source]
    source: io::Error,
}
pub(crate) fn fs_error(
    source: io::Error,
    syscall: &str,
    path: &str,
    dest: Option<&str>,
) -> BoxError {
    // Node's descriptor operations carry only the syscall, unlike path operations.
    let path = (!matches!(syscall, "read" | "write" | "fsync")).then_some(path);
    Box::new(FsError {
        message: hoh_error::node_fs_error_message(&source, syscall, path, dest),
        source,
    })
}
pub(crate) fn node_io_cause(cause: BoxError, syscall: &str, path: &str) -> BoxError {
    match cause.downcast::<io::Error>() {
        Ok(error) => {
            let syscall = if syscall == "open" && error.kind() == io::ErrorKind::IsADirectory {
                "read"
            } else {
                syscall
            };
            fs_error(*error, syscall, path, None)
        }
        Err(error) => error,
    }
}
pub(crate) fn uuid() -> Result<String, BoxError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}
pub(crate) fn absolute(path: &str) -> Result<String, BoxError> {
    let cwd = std::env::current_dir()?;
    Ok(hoh_util_workspace_path::node_path::resolve(
        &cwd.to_string_lossy(),
        &[path],
    ))
}
