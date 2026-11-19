//! Model-facing diagnostics for guarded-mutation failures. Providers and policies retain
//! operation-specific causes, while this package owns the stable message shown to the model.

use hoh_error::{BoxError, HarnessError};
use hoh_fs::{FsError, FsErrorCode};

/// Render the stable model-facing diagnostic for a guarded-mutation failure.
///
/// `FS_STALE_VERSION` keeps the provider's reason and appends its re-read remedy. `FS_NOT_OBSERVED`
/// replaces operation-specific policy/provider text with one path-aware reason and read remedy. The
/// original error remains the cause, and both diagnostics preserve its code for machine routing.
/// Anything else passes through untouched.
///
/// `error` is a filesystem error; `display_path` is the resolved target path shown to the model.
pub fn remediate_fs_error(error: &HarnessError, display_path: &str) -> HarnessError {
    if !error.is_named(FsError::NAME) {
        return error.clone();
    }
    match error.code() {
        "FS_NOT_OBSERVED" => FsError::new(
            format!("cannot modify \"{display_path}\": file has not been read — read the file, then retry"),
            FsErrorCode::NotObserved,
        )
        .with_cause(error.clone()),
        "FS_STALE_VERSION" => FsError::new(
            format!("{} — re-read the file, then retry", error.message()),
            FsErrorCode::StaleVersion,
        )
        .with_cause(error.clone()),
        _ => error.clone(),
    }
}

/// Remediate a caught value, preserving non-filesystem failures: only a filesystem error with one
/// of the two guarded-mutation codes is replaced; everything else is returned as it came.
pub fn remediate_caught(error: BoxError, display_path: &str) -> BoxError {
    match HarnessError::of(error.as_ref()) {
        Some(found) if found.is_named(FsError::NAME) => {
            if matches!(found.code(), "FS_NOT_OBSERVED" | "FS_STALE_VERSION") {
                Box::new(remediate_fs_error(found, display_path))
            } else {
                error
            }
        }
        _ => error,
    }
}
