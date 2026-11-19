//! Canonical-path containment with a filesystem-identity fallback.

use std::path::{MAIN_SEPARATOR_STR, Path};

use hoh_error::BoxError;
use hoh_fs::{ErrnoError, fs_error_from_errno};

fn comparable_path(path: &str, case_sensitive: bool) -> String {
    if case_sensitive {
        path.to_owned()
    } else {
        path.to_lowercase()
    }
}

fn is_lexically_under(path: &str, root: &str, case_sensitive: bool) -> bool {
    let target = comparable_path(path, case_sensitive);
    let root = comparable_path(root, case_sensitive);
    if target == root {
        return true;
    }
    let prefix = if root.ends_with(MAIN_SEPARATOR_STR) {
        root
    } else {
        format!("{root}{MAIN_SEPARATOR_STR}")
    };
    target.starts_with(&prefix)
}

fn is_missing(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(2 | 20))
}

async fn metadata_if_present(path: &str) -> Result<Option<std::fs::Metadata>, BoxError> {
    match tokio::fs::metadata(path).await {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if is_missing(&error) => Ok(None),
        Err(error) => {
            let error: BoxError = Box::new(ErrnoError::from_io(error, "stat", Some(path), None));
            Err(fs_error_from_errno("stat", path, error))
        }
    }
}

#[cfg(unix)]
fn same_identity(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_identity(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    // The current HoH filesystem backend is Linux-only. Keep a conservative
    // fallback for unsupported hosts rather than weakening containment.
    std::ptr::eq(left, right)
}

/// Return whether a canonical target equals or descends from a writable root.
///
/// `case_sensitive` defaults to the host convention. When lexical spellings
/// differ, existing ancestors are compared to the root by filesystem identity.
pub async fn is_path_under(
    path: &str,
    root: &str,
    case_sensitive: Option<bool>,
) -> Result<bool, BoxError> {
    let case_sensitive = case_sensitive.unwrap_or(!cfg!(windows));
    if is_lexically_under(path, root, case_sensitive) {
        return Ok(true);
    }

    let Some(root_info) = metadata_if_present(root).await? else {
        return Ok(false);
    };
    let mut ancestor = Path::new(path);
    loop {
        if let Some(ancestor_path) = ancestor.to_str()
            && metadata_if_present(ancestor_path)
                .await?
                .is_some_and(|info| same_identity(&info, &root_info))
        {
            return Ok(true);
        }
        let Some(parent) = ancestor.parent() else {
            return Ok(false);
        };
        if parent == ancestor {
            return Ok(false);
        }
        ancestor = parent;
    }
}
