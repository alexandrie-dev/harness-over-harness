use crate::{SandboxExecutionPolicy, SandboxMode};
use hoh_error::{HarnessError, node_fs_error_message};
use hoh_util_values::js_json_stringify;
use hoh_util_workspace_path::node_path;
use indexmap::IndexSet;
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
pub async fn canonical_path(path: &str) -> String {
    tokio::fs::canonicalize(path)
        .await
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.into())
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
