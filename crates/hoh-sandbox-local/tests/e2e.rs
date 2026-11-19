use hoh_error::HarnessError;
use hoh_node_addon_system::{LandlockEnforcement, launcher_path, probe};
use hoh_sandbox::{ConfinedSandboxMode, SandboxEnforcement, SandboxPolicy, SandboxProvider};
use hoh_sandbox_local::{
    Config, LocalSandboxProvider, SandboxInternals, profiles::bwrap_profile_args,
};
use std::{fs, path::Path, process::Stdio, sync::Arc, time::Duration};
use tempfile::TempDir;
use tokio::process::Command;

fn policy(mode: ConfinedSandboxMode, root: &Path) -> SandboxPolicy {
    SandboxPolicy {
        mode,
        workspace_root: root.to_string_lossy().into_owned(),
        additional_writable_roots: None,
        session_id: None,
    }
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

fn native_unavailable(reason: &str) {
    if matches!(
        std::env::var("HOH_REQUIRE_NATIVE_SANDBOX").as_deref(),
        Ok("1")
    ) {
        panic!("HOH_REQUIRE_NATIVE_SANDBOX=1: {reason}");
    }
    eprintln!("SKIP: {reason}");
}

fn bwrap_provider() -> LocalSandboxProvider {
    LocalSandboxProvider::new_with_internals(
        Config::default(),
        SandboxInternals {
            platform: Some("linux".into()),
            ..Default::default()
        },
    )
    .unwrap()
}

fn landlock_provider() -> Option<(LocalSandboxProvider, SandboxEnforcement)> {
    let launcher = match launcher_path() {
        Ok(launcher) => launcher,
        Err(cause) => {
            native_unavailable(&format!("Landlock launcher unavailable: {cause}"));
            return None;
        }
    };
    let expected = match probe(Some(&launcher), Some(Duration::from_secs(5))) {
        LandlockEnforcement::Full => SandboxEnforcement::Full,
        LandlockEnforcement::Partial => SandboxEnforcement::Partial,
        LandlockEnforcement::Unusable => {
            native_unavailable("Landlock functional probe reported unusable");
            return None;
        }
    };
    Some((
        LocalSandboxProvider::new_with_internals(
            Config::default(),
            SandboxInternals {
                platform: Some("linux".into()),
                probe_bwrap: Some(Arc::new(|| false)),
                landlock_launcher: Some(launcher),
                ..Default::default()
            },
        )
        .unwrap(),
        expected,
    ))
}

async fn run(
    provider: &LocalSandboxProvider,
    command: &str,
    policy: SandboxPolicy,
) -> (std::process::Output, Vec<String>) {
    let confined = provider
        .confine(&["bash".into(), "-c".into(), command.into()], policy, None)
        .await
        .unwrap();
    let mut process = Command::new(&confined.argv[0]);
    process.args(&confined.argv[1..]).kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(30), process.output())
        .await
        .expect("confined command timed out")
        .unwrap();
    (output, confined.denial_signatures)
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

fn workspace_temp() -> TempDir {
    TempDir::new_in(std::env::current_dir().unwrap()).unwrap()
}

#[tokio::test]
async fn real_bwrap_read_only_denies_writes_and_keeps_dev_null_usable_when_available() {
    if !bwrap_usable().await {
        return;
    }
    let workspace = workspace_temp();
    let denied = workspace.path().join("denied.txt");
    let provider = bwrap_provider();
    let facts = provider
        .confine(
            &["true".into()],
            policy(ConfinedSandboxMode::ReadOnly, workspace.path()),
            None,
        )
        .await
        .unwrap();
    assert_eq!(facts.argv[0], "bwrap");
    assert_eq!(facts.enforcement, hoh_sandbox::SandboxEnforcement::Full);
    assert_eq!(facts.denial_signatures, ["read-only file system"]);
    let (output, signatures) = run(
        &provider,
        &format!("echo denied > {}", quote(&denied)),
        policy(ConfinedSandboxMode::ReadOnly, workspace.path()),
    )
    .await;
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
    assert!(
        signatures
            .iter()
            .any(|signature| stderr.contains(signature))
    );
    assert!(!denied.exists());

    let (output, _) = run(
        &provider,
        "ls / > /dev/null && echo dev-ok",
        policy(ConfinedSandboxMode::ReadOnly, workspace.path()),
    )
    .await;
    assert!(output.status.success());
    assert_eq!(output.stdout, b"dev-ok\n");
}

#[tokio::test]
async fn real_bwrap_workspace_write_is_scoped_and_tmp_is_ephemeral_when_available() {
    if !bwrap_usable().await {
        return;
    }
    let workspace = workspace_temp();
    let outside = workspace_temp();
    let allowed = workspace.path().join("allowed.txt");
    let denied = outside.path().join("denied.txt");
    let host_tmp =
        std::env::temp_dir().join(format!("hoh-sandbox-local-bwrap-{}", std::process::id()));
    let _ = std::fs::remove_file(&host_tmp);
    let provider = bwrap_provider();
    let command = format!(
        "printf allowed > {}; printf tmp-ok > {}; cat {}",
        quote(&allowed),
        quote(&host_tmp),
        quote(&host_tmp)
    );
    let (output, _) = run(
        &provider,
        &command,
        policy(ConfinedSandboxMode::WorkspaceWrite, workspace.path()),
    )
    .await;
    assert!(output.status.success());
    assert_eq!(std::fs::read_to_string(&allowed).unwrap(), "allowed");
    assert_eq!(output.stdout, b"tmp-ok");
    assert!(!host_tmp.exists());

    let (output, _) = run(
        &provider,
        &format!("printf denied > {}", quote(&denied)),
        policy(ConfinedSandboxMode::WorkspaceWrite, workspace.path()),
    )
    .await;
    assert!(!output.status.success());
    assert!(!denied.exists());
}

#[tokio::test]
async fn real_landlock_read_only_denies_host_effects_when_available() {
    let Some((provider, expected_enforcement)) = landlock_provider() else {
        return;
    };
    let workspace = workspace_temp();
    let confined = provider
        .confine(
            &["true".into()],
            policy(ConfinedSandboxMode::ReadOnly, workspace.path()),
            None,
        )
        .await
        .unwrap();
    assert_eq!(confined.enforcement, expected_enforcement);
    let denied = workspace.path().join("denied.txt");
    let shm =
        Path::new("/dev/shm").join(format!("hoh-sandbox-local-landlock-{}", std::process::id()));
    let _ = std::fs::remove_file(&shm);
    let (output, signatures) = run(
        &provider,
        &format!(
            "printf denied > {}; printf denied > {}",
            quote(&denied),
            quote(&shm)
        ),
        policy(ConfinedSandboxMode::ReadOnly, workspace.path()),
    )
    .await;
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
    assert!(
        signatures
            .iter()
            .any(|signature| stderr.contains(signature))
    );
    assert!(!denied.exists());
    assert!(!shm.exists());

    let (output, _) = run(
        &provider,
        "ls / > /dev/null && echo dev-ok",
        policy(ConfinedSandboxMode::ReadOnly, workspace.path()),
    )
    .await;
    assert!(output.status.success());
    assert_eq!(output.stdout, b"dev-ok\n");
}

#[tokio::test]
async fn real_landlock_workspace_write_grants_workspace_and_host_tmp_when_available() {
    let Some((provider, _)) = landlock_provider() else {
        return;
    };
    let workspace = workspace_temp();
    let outside = workspace_temp();
    let scratch = TempDir::new().unwrap();
    let allowed = workspace.path().join("allowed.txt");
    let denied = outside.path().join("denied.txt");
    let temp_allowed = scratch.path().join("allowed.txt");
    let command = format!(
        "printf workspace > {}; printf tmp > {}",
        quote(&allowed),
        quote(&temp_allowed)
    );
    let (output, _) = run(
        &provider,
        &command,
        policy(ConfinedSandboxMode::WorkspaceWrite, workspace.path()),
    )
    .await;
    assert!(output.status.success());
    assert_eq!(std::fs::read_to_string(allowed).unwrap(), "workspace");
    assert_eq!(std::fs::read_to_string(temp_allowed).unwrap(), "tmp");

    let (output, _) = run(
        &provider,
        &format!("printf denied > {}", quote(&denied)),
        policy(ConfinedSandboxMode::WorkspaceWrite, workspace.path()),
    )
    .await;
    assert!(!output.status.success());
    assert!(!denied.exists());
}

async fn check_extra_roots_and_redirect(provider: &LocalSandboxProvider) {
    let workspace = workspace_temp();
    let extra = workspace_temp();
    let outside = workspace_temp();
    let allowed = extra.path().join("allowed.txt");
    let denied = outside.path().join("denied.txt");
    let mut write_policy = policy(ConfinedSandboxMode::WorkspaceWrite, workspace.path());
    write_policy.additional_writable_roots = Some(vec![
        fs::canonicalize(extra.path())
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    ]);
    let (output, _) = run(
        provider,
        &format!("printf allowed > {}", quote(&allowed)),
        write_policy.clone(),
    )
    .await;
    assert!(output.status.success());
    assert_eq!(fs::read_to_string(&allowed).unwrap(), "allowed");
    let (output, signatures) = run(
        provider,
        &format!("printf denied > {}", quote(&denied)),
        write_policy.clone(),
    )
    .await;
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
    assert!(
        signatures
            .iter()
            .any(|signature| stderr.contains(signature))
    );
    assert!(!denied.exists());

    let mut read_policy = policy(ConfinedSandboxMode::ReadOnly, workspace.path());
    read_policy.additional_writable_roots = write_policy.additional_writable_roots.clone();
    let read_only = extra.path().join("read-only.txt");
    let (output, _) = run(
        provider,
        &format!("printf denied > {}", quote(&read_only)),
        read_policy,
    )
    .await;
    assert!(!output.status.success());
    assert!(!read_only.exists());

    let redirected = workspace.path().join("redirected");
    fs::create_dir(&redirected).unwrap();
    let mut redirect_policy = policy(ConfinedSandboxMode::WorkspaceWrite, workspace.path());
    redirect_policy.additional_writable_roots = Some(vec![
        fs::canonicalize(&redirected)
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    ]);
    let (replacement, _) = run(
        provider,
        &format!(
            "rmdir {} && ln -s {} {}",
            quote(&redirected),
            quote(outside.path()),
            quote(&redirected)
        ),
        redirect_policy.clone(),
    )
    .await;
    let escaped = outside.path().join("escaped.txt");
    if replacement.status.success() {
        assert!(
            fs::symlink_metadata(&redirected)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let cause = provider
            .confine(
                &[
                    "bash".into(),
                    "-c".into(),
                    format!("touch {}", quote(&escaped)),
                ],
                redirect_policy,
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(
            HarnessError::of(cause.as_ref()).unwrap().code(),
            "SANDBOX_INVALID_ROOT"
        );
    } else {
        assert!(redirected.is_dir());
        assert!(!redirected.is_symlink());
        let still_granted = redirected.join("still-granted.txt");
        let (output, _) = run(
            provider,
            &format!("printf granted > {}", quote(&still_granted)),
            redirect_policy.clone(),
        )
        .await;
        assert!(output.status.success());
        assert_eq!(fs::read_to_string(still_granted).unwrap(), "granted");

        let (output, signatures) = run(
            provider,
            &format!("printf escaped > {}", quote(&escaped)),
            redirect_policy,
        )
        .await;
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
        assert!(
            signatures
                .iter()
                .any(|signature| stderr.contains(signature))
        );
    }
    assert!(!escaped.exists());
}

#[tokio::test]
async fn real_bwrap_explicit_roots_and_redirect_are_scoped_when_available() {
    if !bwrap_usable().await {
        return;
    }
    check_extra_roots_and_redirect(&bwrap_provider()).await;
}

#[tokio::test]
async fn real_landlock_explicit_roots_and_redirect_are_scoped_when_available() {
    let Some((provider, _)) = landlock_provider() else {
        return;
    };
    check_extra_roots_and_redirect(&provider).await;
}

#[tokio::test]
async fn real_bwrap_private_pid_namespace_blocks_procfs_escape_when_available() {
    if !bwrap_usable().await {
        return;
    }
    let provider = bwrap_provider();
    let workspace = workspace_temp();
    let outside = workspace_temp();
    let target = outside.path().join("escaped.txt");
    let host_namespace = fs::read_link("/proc/self/ns/pid").unwrap();
    for mode in [
        ConfinedSandboxMode::ReadOnly,
        ConfinedSandboxMode::WorkspaceWrite,
    ] {
        let (output, _) = run(
            &provider,
            "readlink /proc/self/ns/pid",
            policy(mode, workspace.path()),
        )
        .await;
        assert!(output.status.success());
        assert_ne!(
            String::from_utf8_lossy(&output.stdout).trim(),
            host_namespace.to_string_lossy()
        );
        let (output, _) = run(
            &provider,
            &format!("printf escaped > /proc/1/root{}", target.display()),
            policy(mode, workspace.path()),
        )
        .await;
        assert!(!output.status.success());
        assert!(!target.exists());
    }
    let (output, _) = run(
        &provider,
        "sleep 30 & child=$!; kill -0 \"$child\" && kill \"$child\"; wait \"$child\"; status=$?; test \"$status\" -ge 128",
        policy(ConfinedSandboxMode::ReadOnly, workspace.path()),
    )
    .await;
    assert!(output.status.success());
}
