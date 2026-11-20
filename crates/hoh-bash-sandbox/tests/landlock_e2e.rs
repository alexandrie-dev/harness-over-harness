//! KEYLESS consumer-integration proof: the REAL `LocalSandboxProvider` (bwrap rung forced off, so
//! the workspace `landlock-run` launcher confines) underneath the REAL `SandboxBashExecutor`,
//! driven through the executor's public run/start paths. Verifies the WORLD (files exist or
//! don't) plus the stamped result facts; the backend-only confinement proofs live with
//! `hoh-sandbox-local`.
//!
//! Self-skips when the running kernel does not enforce Landlock (a failure instead under
//! `HOH_REQUIRE_NATIVE_SANDBOX=1`).

mod support;

use std::{fs, path::Path, sync::Arc, time::Duration};

use hoh_node_addon_system::{LandlockEnforcement, launcher_path, probe};
use hoh_sandbox::{SandboxEnforcement, SandboxExecutionPolicy, SandboxMode};
use hoh_sandbox_local::{Config, LocalSandboxProvider, SandboxInternals};
use hoh_shell::{ShellExecRequest, ShellSandboxInfo};
use serde_json::json;
use support::*;

fn native_unavailable(reason: &str) {
    if matches!(
        std::env::var("HOH_REQUIRE_NATIVE_SANDBOX").as_deref(),
        Ok("1")
    ) {
        panic!("HOH_REQUIRE_NATIVE_SANDBOX=1: {reason}");
    }
    eprintln!("SKIP: {reason}");
}

/// The kernel's enforcement level from the probe — stamped facts below must match it.
fn landlock() -> Option<(std::path::PathBuf, SandboxEnforcement)> {
    let launcher = match launcher_path() {
        Ok(launcher) => launcher,
        Err(cause) => {
            native_unavailable(&format!("Landlock launcher unavailable: {cause}"));
            return None;
        }
    };
    match probe(Some(&launcher), Some(Duration::from_secs(5))) {
        LandlockEnforcement::Full => Some((launcher, SandboxEnforcement::Full)),
        LandlockEnforcement::Partial => Some((launcher, SandboxEnforcement::Partial)),
        LandlockEnforcement::Unusable => {
            native_unavailable("Landlock functional probe reported unusable");
            None
        }
    }
}

fn home() -> String {
    std::env::var("HOH_NATIVE_SANDBOX_TEST_ROOT")
        .or_else(|_| std::env::var("HOME"))
        .expect("HOH_NATIVE_SANDBOX_TEST_ROOT or HOME is set")
}

fn temp_dir(base: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("dsh-landlock-e2e-")
        .tempdir_in(base)
        .unwrap()
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

async fn sandboxed_bash(launcher: std::path::PathBuf, workspace: &str, mode: &str) -> Real {
    let provider = LocalSandboxProvider::new_with_internals(
        Config::default(),
        SandboxInternals {
            platform: Some("linux".into()),
            probe_bwrap: Some(Arc::new(|| false)),
            landlock_launcher: Some(launcher),
            ..SandboxInternals::default()
        },
    )
    .unwrap();
    setup_real(
        Arc::new(provider),
        json!({"cwd": workspace, "timeoutMs": 30_000}),
        mode,
        workspace,
    )
    .await
}

fn facts(
    mode: SandboxMode,
    denied: bool,
    enforcement: SandboxEnforcement,
) -> Option<ShellSandboxInfo> {
    Some(ShellSandboxInfo {
        mode,
        denied,
        enforcement: Some(enforcement),
        runner_failed: None,
    })
}

#[tokio::test]
async fn read_only_denies_a_write_the_file_must_not_exist_the_result_carries_denial_and_enforcement_facts()
 {
    let Some((launcher, enforcement)) = landlock() else {
        return;
    };
    let workdir = temp_dir(&std::env::temp_dir().to_string_lossy());
    let root = text(workdir.path());
    let bash = sandboxed_bash(launcher, &root, "read-only").await;
    let result = bash
        .run(bash.resolve(request(&format!("echo hi > {root}/denied.txt"))))
        .await
        .unwrap();
    assert_ne!(result.exit_code, Some(0));
    assert_eq!(
        result.sandbox,
        facts(SandboxMode::ReadOnly, true, enforcement)
    );
    assert!(!workdir.path().join("denied.txt").exists());
    bash.close().await;
}

#[tokio::test]
async fn workspace_write_lands_a_write_inside_the_workspace_root_and_still_denies_one_beside_it() {
    let Some((launcher, enforcement)) = landlock() else {
        return;
    };
    let workdir = temp_dir(&home());
    let outside = temp_dir(&home());
    let root = text(workdir.path());
    let bash = sandboxed_bash(launcher, &root, "workspace-write").await;

    let inside = bash
        .run(bash.resolve(request(&format!("printf landlock-ok > {root}/allowed.txt"))))
        .await
        .unwrap();
    assert_eq!(inside.exit_code, Some(0));
    assert_eq!(
        inside.sandbox,
        facts(SandboxMode::WorkspaceWrite, false, enforcement)
    );
    assert_eq!(
        fs::read_to_string(workdir.path().join("allowed.txt")).unwrap(),
        "landlock-ok"
    );

    let denied = bash
        .run(bash.resolve(request(&format!(
            "echo hi > {}/denied.txt",
            text(outside.path())
        ))))
        .await
        .unwrap();
    assert_ne!(denied.exit_code, Some(0));
    assert_eq!(
        denied.sandbox,
        facts(SandboxMode::WorkspaceWrite, true, enforcement)
    );
    assert!(!outside.path().join("denied.txt").exists());
    bash.close().await;
}

#[tokio::test]
async fn classifies_a_background_denial_once_the_task_settles() {
    let Some((launcher, enforcement)) = landlock() else {
        return;
    };
    let workdir = temp_dir(&home());
    let root = text(workdir.path());
    let bash = sandboxed_bash(launcher, &root, "read-only").await;
    let task = bash
        .start(bash.resolve(request(&format!("echo hi > {root}/bg-denied.txt"))))
        .await
        .unwrap();
    task.done().await;
    assert_eq!(
        task.sandbox(),
        facts(SandboxMode::ReadOnly, true, enforcement)
    );
    assert!(!workdir.path().join("bg-denied.txt").exists());
    bash.close().await;
}

#[tokio::test]
async fn an_approved_escalated_retry_the_spec_level_workspace_write_override_lands_the_exact_write_read_only_denied()
 {
    let Some((launcher, enforcement)) = landlock() else {
        return;
    };
    let workdir = temp_dir(&home());
    let root = text(workdir.path());
    let bash = sandboxed_bash(launcher, &root, "read-only").await;
    let command = format!("printf escalated > {root}/escalated.txt");
    let strict = bash.run(bash.resolve(request(&command))).await.unwrap();
    assert_ne!(strict.exit_code, Some(0));
    assert_eq!(
        strict.sandbox,
        facts(SandboxMode::ReadOnly, true, enforcement)
    );
    assert!(!workdir.path().join("escalated.txt").exists());
    let retried = bash
        .run(bash.resolve(ShellExecRequest {
            sandbox_policy: Some(SandboxExecutionPolicy {
                mode: SandboxMode::WorkspaceWrite,
                workspace_root: root.clone(),
                additional_writable_roots: None,
                session_id: None,
                protected_roots: None,
            }),
            ..request(&command)
        }))
        .await
        .unwrap();
    assert_eq!(retried.exit_code, Some(0));
    assert_eq!(
        retried.sandbox,
        facts(SandboxMode::WorkspaceWrite, false, enforcement)
    );
    assert_eq!(
        fs::read_to_string(workdir.path().join("escalated.txt")).unwrap(),
        "escalated"
    );
    bash.close().await;
}
