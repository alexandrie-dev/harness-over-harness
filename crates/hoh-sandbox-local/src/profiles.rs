//! Linux profile builders for bubblewrap and the Landlock launcher.

use hoh_error::BoxError;
use hoh_node_addon_system::{LauncherGrants, grant_args};
use hoh_sandbox::{
    SandboxExecutionPolicy, SandboxPolicy, canonical_path, workspace_writable_roots,
};
use std::path::{Path, PathBuf};

fn execution_policy(policy: &SandboxPolicy) -> SandboxExecutionPolicy {
    policy.clone().into()
}

struct ProtectedPath {
    path: String,
    directory: bool,
}

fn is_missing(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(2 | 20))
}

async fn existing_protected_path(root: String) -> Result<ProtectedPath, BoxError> {
    match tokio::fs::metadata(&root).await {
        Ok(metadata) => Ok(ProtectedPath {
            path: root,
            directory: metadata.is_dir(),
        }),
        Err(error) if is_missing(&error) => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "sandbox-local: protected path does not exist: {root}; refusing to broaden its mask to an ancestor"
            ),
        )
        .into()),
        Err(error) => Err(error.into()),
    }
}

async fn canonical_protected_paths(policy: &SandboxPolicy) -> Result<Vec<ProtectedPath>, BoxError> {
    let mut roots = Vec::<ProtectedPath>::new();
    for root in policy.protected_roots.iter().flatten() {
        let root = canonical_path(root).await;
        let protected = existing_protected_path(root).await?;
        if roots
            .iter()
            .any(|parent| parent.directory && Path::new(&protected.path).starts_with(&parent.path))
        {
            continue;
        }
        if protected.directory {
            roots.retain(|child| !Path::new(&child.path).starts_with(&protected.path));
        }
        roots.push(protected);
    }
    Ok(roots)
}

pub async fn bwrap_profile_args(policy: &SandboxPolicy) -> Result<Vec<String>, BoxError> {
    let root_binding = if policy.unrestricted_writes {
        "--bind"
    } else {
        "--ro-bind"
    };
    let mut args = [
        root_binding,
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
    let policy_execution = execution_policy(policy);
    let roots = workspace_writable_roots(&policy_execution).await?;
    if !policy.unrestricted_writes && !roots.is_empty() {
        args.extend(["--tmpfs".into(), "/tmp".into()]);
        for root in roots {
            args.extend(["--bind".into(), root.clone(), root]);
        }
    }
    if let Some(ready) = &policy.ready_dir {
        args.extend(["--bind".into(), ready.clone(), ready.clone()]);
    }
    for protected in canonical_protected_paths(policy).await? {
        if protected.directory {
            args.extend([
                "--tmpfs".into(),
                protected.path.clone(),
                "--chmod".into(),
                "000".into(),
                protected.path,
            ]);
        } else {
            // Existing protected files are hidden without exposing their contents.
            args.extend(["--ro-bind".into(), "/dev/null".into(), protected.path]);
        }
    }
    Ok(args)
}

pub async fn landlock_profile_args(policy: &SandboxPolicy) -> Result<Vec<String>, BoxError> {
    if policy
        .protected_roots
        .as_ref()
        .is_some_and(|roots| !roots.is_empty())
    {
        return Err(std::io::Error::other(
            "sandbox-local: protected roots require the race-free bwrap backend",
        )
        .into());
    }
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
