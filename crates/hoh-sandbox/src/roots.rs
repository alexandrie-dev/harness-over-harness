use crate::{SandboxExecutionPolicy, SandboxMode};
use hoh_error::{HarnessError, node_fs_error_message};
use hoh_util_values::js_json_stringify;
use hoh_util_workspace_path::node_path;
use indexmap::IndexSet;
use std::path::Path;
pub struct SandboxInvalidRootError;
impl SandboxInvalidRootError {
    pub const NAME: &str = "SandboxInvalidRootError";
    #[allow(clippy::new_ret_no_self)]
    pub fn new(path: &str, detail: &str, subject: Option<&str>) -> HarnessError {
        HarnessError::named(
            Self::NAME,
            format!(
                "Invalid {} {}: {detail}",
                subject.unwrap_or("additional writable directory"),
                js_json_stringify(&path.into())
            ),
            "SANDBOX_INVALID_ROOT",
        )
    }
}
fn is_missing(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(2 | 20))
}

/// Resolve every existing symlink component while preserving a not-yet-created suffix.
///
/// Unlike `std::fs::canonicalize`, this is useful for protected paths that the model must not be
/// able to create later. Errors other than a missing/non-directory suffix remain fail-closed.
pub async fn canonical_path_checked(path: &str) -> std::io::Result<String> {
    match tokio::fs::canonicalize(path).await {
        Ok(path) => return Ok(path.to_string_lossy().into_owned()),
        Err(error) if !is_missing(&error) => return Err(error),
        Err(_) => {}
    }

    let original = Path::new(path);
    let mut ancestor = original;
    let physical = loop {
        match tokio::fs::canonicalize(ancestor).await {
            Ok(path) => break path,
            Err(error) if is_missing(&error) => {
                ancestor = ancestor.parent().ok_or(error)?;
            }
            Err(error) => return Err(error),
        }
    };
    let suffix = original
        .strip_prefix(ancestor)
        .expect("an ancestor selected from the same path is always a prefix");
    Ok(node_path::normalize(
        &physical.join(suffix).to_string_lossy(),
    ))
}

/// The source's `canonicalPath` (`roots.ts:62-73`): the native realpath, or the spelling as-is when
/// resolution fails. A missing root therefore matches nothing until it exists, even behind a
/// symlinked prefix. Protected storage needs the physical prefix instead and uses
/// [`canonical_path_checked`].
pub async fn canonical_path(path: &str) -> String {
    match tokio::fs::canonicalize(path).await {
        Ok(physical) => physical.to_string_lossy().into_owned(),
        Err(_) => path.to_owned(),
    }
}
fn inspection_error(
    path: &str,
    physical: &str,
    syscall: &str,
    error: std::io::Error,
) -> HarnessError {
    SandboxInvalidRootError::new(
        path,
        &format!(
            "cannot read the directory: Error: {}",
            node_fs_error_message(&error, syscall, Some(physical), None)
        ),
        None,
    )
}
async fn additional_root(path: &str) -> Result<String, HarnessError> {
    let physical = tokio::fs::canonicalize(path)
        .await
        .map_err(|e| inspection_error(path, path, "realpath", e))?
        .to_string_lossy()
        .into_owned();
    let info = tokio::fs::metadata(&physical)
        .await
        .map_err(|e| inspection_error(path, &physical, "stat", e))?;
    if !info.is_dir() {
        return Err(SandboxInvalidRootError::new(
            path,
            "expected an existing directory",
            None,
        ));
    }
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/".into());
    if !node_path::relative(&cwd, path, &physical).is_empty() {
        return Err(SandboxInvalidRootError::new(
            path,
            &format!(
                "use the physical directory path {}; symlinked grants are not supported",
                js_json_stringify(&physical.clone().into())
            ),
            None,
        ));
    }
    Ok(physical)
}
pub async fn workspace_writable_roots(
    policy: &SandboxExecutionPolicy,
) -> Result<Vec<String>, HarnessError> {
    if policy.mode != SandboxMode::WorkspaceWrite {
        return Ok(vec![]);
    }
    let mut additional = Vec::new();
    for root in policy.additional_writable_roots.iter().flatten() {
        additional.push(additional_root(root).await?);
    }
    let mut roots = IndexSet::new();
    roots.insert(canonical_path(&policy.workspace_root).await);
    roots.extend(additional);
    Ok(roots.into_iter().collect())
}
pub async fn writable_roots(policy: &SandboxExecutionPolicy) -> Result<Vec<String>, HarnessError> {
    if policy.mode != SandboxMode::WorkspaceWrite {
        return Ok(vec![]);
    }
    let mut roots: IndexSet<_> = workspace_writable_roots(policy)
        .await?
        .into_iter()
        .collect();
    let temp = ["TMPDIR", "TMP", "TEMP"]
        .into_iter()
        .find_map(|key| std::env::var(key).ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "/tmp".into());
    let temp = if temp.len() > 1 {
        temp.strip_suffix('/').unwrap_or(&temp)
    } else {
        &temp
    };
    roots.insert(canonical_path("/tmp").await);
    roots.insert(canonical_path(temp).await);
    Ok(roots.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reports_inspection_failures_that_do_not_establish_a_missing_or_non_directory_path() {
        let e = inspection_error(
            "/private",
            "/private",
            "realpath",
            std::io::Error::from_raw_os_error(13),
        );
        assert!(e.message().contains("cannot read the directory"));
        assert!(e.message().contains("EACCES"));
        assert_eq!(e.code(), "SANDBOX_INVALID_ROOT");
    }
}
