use crate::{FsError, FsErrorCode};
use hoh_error::{BoxError, HarnessError, SharedError, node_error_code, node_fs_error_message};
use std::{io, sync::Arc};
/// Rust representation of a Node Error with an optional string errno code.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct ErrnoError {
    pub code: Option<String>,
    pub message: String,
    #[source]
    cause: Option<Arc<io::Error>>,
}
impl ErrnoError {
    pub fn new(code: Option<String>, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            cause: None,
        }
    }
    pub fn from_io(
        error: io::Error,
        syscall: &str,
        path: Option<&str>,
        destination: Option<&str>,
    ) -> Self {
        Self {
            code: node_error_code(&error).map(str::to_owned),
            message: node_fs_error_message(&error, syscall, path, destination),
            cause: Some(Arc::new(error)),
        }
    }
}
/// Native I/O must first use `ErrnoError::from_io` with its real syscall/paths.
/// Unbridged Rust errors have no Node `code` property and remain defects.
pub fn fs_error_from_errno(operation: &str, display_path: &str, error: BoxError) -> BoxError {
    if HarnessError::of(error.as_ref()).is_some_and(|e| e.is_named(FsError::NAME)) {
        return error;
    }
    let inner = error
        .downcast_ref::<SharedError>()
        .map(|e| e.get_ref() as &(dyn std::error::Error + 'static))
        .unwrap_or(error.as_ref());
    let code = if let Some(e) = inner.downcast_ref::<ErrnoError>() {
        e.code.as_deref()
    } else {
        HarnessError::of(inner).map(HarnessError::code)
    };
    let Some(code) = code else {
        return error;
    };
    let (code, detail) = match code {
        "ENOENT" | "ENOTDIR" => (FsErrorCode::NotFound, "not found".into()),
        "EACCES" | "EPERM" => (FsErrorCode::PermissionDenied, "permission denied".into()),
        "EISDIR" => (FsErrorCode::NotRegularFile, "not a regular file".into()),
        _ => (FsErrorCode::IoError, error.to_string()),
    };
    Box::new(
        FsError::new(
            format!("cannot {operation} \"{display_path}\": {detail}"),
            code,
        )
        .with_cause(error),
    )
}
