//! Linux profile builders for bubblewrap and the Landlock launcher.

use hoh_error::BoxError;
use hoh_node_addon_system::{LauncherGrants, grant_args};
use hoh_sandbox::{SandboxExecutionPolicy, SandboxPolicy, workspace_writable_roots};

fn execution_policy(policy: &SandboxPolicy) -> SandboxExecutionPolicy {
    policy.clone().into()
}

pub async fn bwrap_profile_args(policy: &SandboxPolicy) -> Result<Vec<String>, BoxError> {
    let mut args = [
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--unshare-pid",
        "--proc",
        "/proc",
        "--die-with-parent",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    let policy = execution_policy(policy);
    let roots = workspace_writable_roots(&policy).await?;
    if !roots.is_empty() {
        args.extend(["--tmpfs".into(), "/tmp".into()]);
        for root in roots {
            args.extend(["--bind".into(), root.clone(), root]);
        }
    }
    Ok(args)
}

pub async fn landlock_profile_args(policy: &SandboxPolicy) -> Result<Vec<String>, BoxError> {
    let policy = execution_policy(policy);
    let mut read_write = vec![PathBuf::from("/dev/null")];
    let roots = workspace_writable_roots(&policy).await?;
    if !roots.is_empty() {
        read_write.push(PathBuf::from("/tmp"));
        read_write.extend(roots.into_iter().map(PathBuf::from));
    }
    grant_args(&LauncherGrants {
        read_only: vec![PathBuf::from("/")],
        read_write,
    })
    .into_iter()
    .map(|value| {
        value.into_string().map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "sandbox-local: Landlock profile path is not valid UTF-8",
            )
            .into()
        })
    })
    .collect()
}

use std::path::PathBuf;
