//! Keyless integration of the real provider and executor through public run/start paths. With no
//! rung forced, a passing bwrap probe selects the ladder's first rung. The tests check world
//! effects and stamped facts, including EROFS classification through the wrap-carried dialect;
//! backend-only confinement is covered by `hoh-sandbox-local`.
//!
//! Skips when bwrap or unprivileged user namespaces are unavailable (a failure instead under
//! `HOH_REQUIRE_NATIVE_SANDBOX=1`). HOME-based paths are intentional because bwrap replaces
//! `/tmp`, which cannot prove the workspace-root boundary.

mod support;

use std::{fs, path::Path, process::Stdio, sync::Arc, time::Duration};

use hoh_sandbox::SandboxExecutionPolicy;
use hoh_sandbox::{ConfinedSandboxMode, SandboxEnforcement, SandboxMode, SandboxPolicy};
use hoh_sandbox_local::{
    Config, LocalSandboxProvider, SandboxInternals, profiles::bwrap_profile_args,
};
use hoh_shell::{ShellExecRequest, ShellSandboxInfo};
use serde_json::json;
use support::*;
use tokio::process::Command;

fn native_unavailable(reason: &str) {
    if matches!(
        std::env::var("HOH_REQUIRE_NATIVE_SANDBOX").as_deref(),
        Ok("1")
    ) {
        panic!("HOH_REQUIRE_NATIVE_SANDBOX=1: {reason}");
    }
    eprintln!("SKIP: {reason}");
}

async fn bwrap_usable() -> bool {
    let profile = bwrap_profile_args(&SandboxPolicy {
        mode: ConfinedSandboxMode::ReadOnly,
        workspace_root: "/".into(),
        additional_writable_roots: None,
        session_id: None,
    })
    .await
    .unwrap();
    let mut command = Command::new("bwrap");
    command
        .args(profile)
        .arg("--")
        .arg("true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let usable = tokio::time::timeout(Duration::from_secs(5), command.status())
        .await
        .is_ok_and(|result| result.is_ok_and(|status| status.success()));
    if !usable {
        native_unavailable("bwrap functional probe failed");
    }
    usable
}

fn home() -> String {
    std::env::var("HOH_NATIVE_SANDBOX_TEST_ROOT")
        .or_else(|_| std::env::var("HOME"))
        .expect("HOH_NATIVE_SANDBOX_TEST_ROOT or HOME is set")
}

fn temp_dir(base: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("dsh-bwrap-e2e-")
        .tempdir_in(base)
        .unwrap()
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

async fn sandboxed_bash(workspace: &str, mode: &str) -> Real {
    let provider = LocalSandboxProvider::new_with_internals(
        Config::default(),
        SandboxInternals {
            platform: Some("linux".into()),
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

fn facts(mode: SandboxMode, denied: bool) -> Option<ShellSandboxInfo> {
    Some(ShellSandboxInfo {
        mode,
        denied,
        enforcement: Some(SandboxEnforcement::Full),
        runner_failed: None,
    })
}

#[tokio::test]
async fn read_only_denies_a_write_the_file_must_not_exist_and_erofs_text_classifies_as_a_denial() {
    if !bwrap_usable().await {
        return;
    }
    let workdir = temp_dir(&home());
    let bash = sandboxed_bash(&text(workdir.path()), "read-only").await;
    let result = bash
        .run(bash.resolve(request(&format!(
            "echo hi > {}/denied.txt",
            text(workdir.path())
        ))))
        .await
        .unwrap();
    assert_ne!(result.exit_code, Some(0));
    assert_eq!(result.sandbox, facts(SandboxMode::ReadOnly, true));
    assert!(!workdir.path().join("denied.txt").exists());
    bash.close().await;
}

#[tokio::test]
async fn workspace_write_lands_a_write_inside_the_workspace_root_and_still_denies_one_beside_it() {
    if !bwrap_usable().await {
        return;
    }
    let workdir = temp_dir(&home());
    let outside = temp_dir(&home());
    let bash = sandboxed_bash(&text(workdir.path()), "workspace-write").await;

    let inside = bash
        .run(bash.resolve(request(&format!(
            "printf bwrap-ok > {}/allowed.txt",
            text(workdir.path())
        ))))
        .await
        .unwrap();
    assert_eq!(inside.exit_code, Some(0));
    assert_eq!(inside.sandbox, facts(SandboxMode::WorkspaceWrite, false));
    assert_eq!(
        fs::read_to_string(workdir.path().join("allowed.txt")).unwrap(),
        "bwrap-ok"
    );

    let denied = bash
        .run(bash.resolve(request(&format!(
            "echo hi > {}/denied.txt",
            text(outside.path())
        ))))
        .await
        .unwrap();
    assert_ne!(denied.exit_code, Some(0));
    assert_eq!(denied.sandbox, facts(SandboxMode::WorkspaceWrite, true));
    assert!(!outside.path().join("denied.txt").exists());
    bash.close().await;
}

#[tokio::test]
async fn classifies_a_background_denial_once_the_task_settles() {
    if !bwrap_usable().await {
        return;
    }
    let workdir = temp_dir(&home());
    let bash = sandboxed_bash(&text(workdir.path()), "read-only").await;
    let task = bash
        .start(bash.resolve(request(&format!(
            "echo hi > {}/bg-denied.txt",
            text(workdir.path())
        ))))
        .await
        .unwrap();
    task.done().await;
    assert_eq!(task.sandbox(), facts(SandboxMode::ReadOnly, true));
    assert!(!workdir.path().join("bg-denied.txt").exists());
    bash.close().await;
}

#[tokio::test]
async fn an_approved_escalated_retry_the_spec_level_workspace_write_override_lands_the_exact_write_read_only_denied()
 {
    if !bwrap_usable().await {
        return;
    }
    let workdir = temp_dir(&home());
    let root = text(workdir.path());
    let bash = sandboxed_bash(&root, "read-only").await;
    let command = format!("printf escalated > {root}/escalated.txt");
    let strict = bash.run(bash.resolve(request(&command))).await.unwrap();
    assert_ne!(strict.exit_code, Some(0));
    assert_eq!(strict.sandbox, facts(SandboxMode::ReadOnly, true));
    assert!(!workdir.path().join("escalated.txt").exists());
    let retried = bash
        .run(bash.resolve(ShellExecRequest {
            sandbox_policy: Some(SandboxExecutionPolicy {
                mode: SandboxMode::WorkspaceWrite,
                workspace_root: root.clone(),
                additional_writable_roots: None,
                session_id: None,
            }),
            ..request(&command)
        }))
        .await
        .unwrap();
    assert_eq!(retried.exit_code, Some(0));
    assert_eq!(retried.sandbox, facts(SandboxMode::WorkspaceWrite, false));
    assert_eq!(
        fs::read_to_string(workdir.path().join("escalated.txt")).unwrap(),
        "escalated"
    );
    bash.close().await;
}
