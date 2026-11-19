use hoh_error::{HarnessError, SharedError};
use hoh_kernel::Kernel;
use hoh_sandbox::{
    ConfinedSandboxMode, Sandbox, SandboxEnforcement, SandboxPolicy, SandboxProvider,
};
use hoh_sandbox_local::{
    Config, LocalRunner, LocalSandboxPlugin, LocalSandboxProvider, ProbeVerdict, SandboxInternals,
    profiles::{bwrap_profile_args, landlock_profile_args},
};
use hoh_timeout::AbortController;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tempfile::TempDir;

fn policy(mode: ConfinedSandboxMode, root: impl AsRef<Path>) -> SandboxPolicy {
    SandboxPolicy {
        mode,
        workspace_root: root.as_ref().to_string_lossy().into_owned(),
        additional_writable_roots: None,
        session_id: None,
    }
}

fn ro() -> SandboxPolicy {
    policy(ConfinedSandboxMode::ReadOnly, "/ws")
}

fn ww() -> SandboxPolicy {
    policy(ConfinedSandboxMode::WorkspaceWrite, "/ws")
}

fn internals(
    bwrap: impl Fn() -> bool + Send + Sync + 'static,
    landlock: impl Fn(&Path) -> ProbeVerdict + Send + Sync + 'static,
) -> SandboxInternals {
    SandboxInternals {
        platform: Some("linux".into()),
        probe_bwrap: Some(Arc::new(bwrap)),
        probe_landlock: Some(Arc::new(landlock)),
        landlock_launcher: Some(PathBuf::from("/fake/landlock-run")),
        ..Default::default()
    }
}

fn provider_with(internals: SandboxInternals) -> LocalSandboxProvider {
    LocalSandboxProvider::new_with_internals(Config::default(), internals).unwrap()
}

#[tokio::test]
async fn plugin_registers_the_sandbox_service() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fiber = ctx
        .plugin::<LocalSandboxPlugin>(serde_json::json!({
            "runnerCommand": ["fake-runner"],
            "runnerFailureSignatures": ["fake-runner: failed"]
        }))
        .unwrap();
    fiber.settled().await.unwrap();
    let confined = ctx
        .get::<Sandbox>()
        .unwrap()
        .confine(&["true".into()], ro(), None)
        .await
        .unwrap();
    assert_eq!(confined.argv[0], "fake-runner");
    kernel.shutdown().await;
}

#[tokio::test]
async fn bwrap_read_only_profile_is_exact() {
    assert_eq!(
        bwrap_profile_args(&ro()).await.unwrap(),
        [
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
    );
}

#[tokio::test]
async fn bwrap_workspace_write_adds_ephemeral_tmp_and_workspace_bind() {
    assert_eq!(
        bwrap_profile_args(&ww()).await.unwrap(),
        [
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--unshare-pid",
            "--proc",
            "/proc",
            "--die-with-parent",
            "--tmpfs",
            "/tmp",
            "--bind",
            "/ws",
            "/ws",
        ]
    );
}

#[tokio::test]
async fn landlock_profiles_keep_grant_order_and_dev_null_scope() {
    assert_eq!(
        landlock_profile_args(&ro()).await.unwrap(),
        ["--ro", "/", "--rw", "/dev/null"]
    );
    assert_eq!(
        landlock_profile_args(&ww()).await.unwrap(),
        [
            "--ro",
            "/",
            "--rw",
            "/dev/null",
            "--rw",
            "/tmp",
            "--rw",
            "/ws",
        ]
    );
}

#[tokio::test]
async fn explicit_roots_follow_workspace_and_are_deduplicated() {
    let workspace = TempDir::new().unwrap();
    let extra = TempDir::new().unwrap();
    let root = fs::canonicalize(workspace.path()).unwrap();
    let extra = fs::canonicalize(extra.path()).unwrap();
    let mut policy = policy(ConfinedSandboxMode::WorkspaceWrite, &root);
    policy.additional_writable_roots = Some(vec![
        extra.to_string_lossy().into_owned(),
        root.to_string_lossy().into_owned(),
        extra.to_string_lossy().into_owned(),
    ]);
    let bwrap = bwrap_profile_args(&policy).await.unwrap();
    assert_eq!(
        bwrap[bwrap.len() - 6..],
        [
            "--bind",
            root.to_str().unwrap(),
            root.to_str().unwrap(),
            "--bind",
            extra.to_str().unwrap(),
            extra.to_str().unwrap(),
        ]
    );
    let landlock = landlock_profile_args(&policy).await.unwrap();
    assert_eq!(
        landlock[landlock.len() - 6..],
        [
            "--rw",
            "/tmp",
            "--rw",
            root.to_str().unwrap(),
            "--rw",
            extra.to_str().unwrap(),
        ]
    );
}

#[tokio::test]
async fn read_only_ignores_even_invalid_additional_roots() {
    let mut policy = ro();
    policy.additional_writable_roots = Some(vec!["/definitely/missing/root".into()]);
    assert!(
        !bwrap_profile_args(&policy)
            .await
            .unwrap()
            .contains(&"--bind".into())
    );
    assert_eq!(
        landlock_profile_args(&policy).await.unwrap(),
        ["--ro", "/", "--rw", "/dev/null"]
    );
}

#[tokio::test]
async fn redirected_additional_root_is_rejected_before_wrap() {
    let workspace = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let link = workspace.path().join("extra");
    std::os::unix::fs::symlink(outside.path(), &link).unwrap();
    let mut policy = policy(ConfinedSandboxMode::WorkspaceWrite, workspace.path());
    policy.additional_writable_roots = Some(vec![link.to_string_lossy().into_owned()]);
    let cause = bwrap_profile_args(&policy).await.unwrap_err();
    let cause = HarnessError::of(cause.as_ref()).unwrap();
    assert_eq!(cause.code(), "SANDBOX_INVALID_ROOT");
}

#[tokio::test]
async fn configured_runner_skips_selection_and_uses_bwrap_profile() {
    let provider = LocalSandboxProvider::new_with_internals(
        Config {
            runner_command: vec!["fake-runner".into(), "--flag".into()],
            runner_failure_signatures: vec!["fake-runner: profile rejected".into()],
            ..Default::default()
        },
        internals(
            || panic!("bwrap probe must be skipped"),
            |_| panic!("Landlock probe must be skipped"),
        ),
    )
    .unwrap();
    let confined = provider
        .confine(&["bash".into(), "-c".into(), "echo hi".into()], ww(), None)
        .await
        .unwrap();
    assert_eq!(confined.enforcement, SandboxEnforcement::Full);
    assert_eq!(
        confined.denial_signatures,
        ["read-only file system", "permission denied"]
    );
    assert_eq!(
        confined.runner_failure_rules[0].fatal_signatures,
        ["fake-runner: profile rejected"]
    );
    assert_eq!(&confined.argv[..2], ["fake-runner", "--flag"]);
    assert_eq!(
        &confined.argv[confined.argv.len() - 4..],
        ["--", "bash", "-c", "echo hi"]
    );
}

#[test]
fn config_defaults_and_validation_match_source() {
    let parsed: Config = serde_json::from_str("{}").unwrap();
    assert_eq!(parsed.probe_timeout_ms, 5_000.0);
    assert!(parsed.runner_command.is_empty());
    assert!(
        LocalSandboxProvider::new(Config {
            runner_command: vec!["runner".into()],
            ..Default::default()
        })
        .err()
        .unwrap()
        .to_string()
        .contains("runnerCommand requires at least one")
    );
    assert!(
        LocalSandboxProvider::new(Config {
            runner_failure_signatures: vec!["failure".into()],
            ..Default::default()
        })
        .err()
        .unwrap()
        .to_string()
        .contains("runnerFailureSignatures requires runnerCommand")
    );
    for signature in ["  ", "\u{feff}", "fatal\ncontinued", "fatal\rcontinued"] {
        assert!(
            LocalSandboxProvider::new(Config {
                runner_command: vec!["runner".into()],
                runner_failure_signatures: vec![signature.into()],
                ..Default::default()
            })
            .err()
            .unwrap()
            .to_string()
            .contains("entries must be non-empty single-line strings")
        );
    }
    for timeout in [0.0, -1.0, 1.5, f64::INFINITY] {
        assert!(
            LocalSandboxProvider::new(Config {
                probe_timeout_ms: timeout,
                ..Default::default()
            })
            .err()
            .unwrap()
            .to_string()
            .contains("probeTimeoutMs must be a positive finite number")
        );
    }
}

#[tokio::test]
async fn empty_runner_command_still_uses_and_gates_the_linux_chain() {
    let provider = LocalSandboxProvider::new_with_internals(
        Config {
            runner_command: vec![],
            ..Default::default()
        },
        internals(|| false, |_| ProbeVerdict::Unusable),
    )
    .unwrap();
    let cause = provider
        .confine(&["true".into()], ro(), None)
        .await
        .unwrap_err();
    assert_eq!(
        HarnessError::of(cause.as_ref()).unwrap().code(),
        "SANDBOX_UNAVAILABLE"
    );
}

#[tokio::test]
async fn linux_prefers_bwrap_and_does_not_probe_landlock() {
    let provider = provider_with(internals(
        || true,
        |_| panic!("Landlock must not be probed after bwrap passes"),
    ));
    let confined = provider
        .confine(&["true".into()], ro(), None)
        .await
        .unwrap();
    assert_eq!(confined.argv[0], "bwrap");
    assert_eq!(confined.enforcement, SandboxEnforcement::Full);
    assert_eq!(confined.denial_signatures, ["read-only file system"]);
    assert_eq!(
        confined.runner_failure_rules[0].fatal_signatures,
        ["bwrap: "]
    );
}

#[tokio::test]
async fn linux_falls_back_to_landlock_and_keeps_full_or_partial_probe_fact() {
    for (verdict, enforcement) in [
        (ProbeVerdict::Full, SandboxEnforcement::Full),
        (ProbeVerdict::Partial, SandboxEnforcement::Partial),
    ] {
        let seen_launcher = Arc::new(std::sync::Mutex::new(None));
        let captured_launcher = seen_launcher.clone();
        let provider = provider_with(internals(
            || false,
            move |launcher| {
                *captured_launcher.lock().unwrap() = Some(launcher.to_path_buf());
                verdict
            },
        ));
        let confined = provider
            .confine(&["bash".into(), "-c".into(), "echo hi".into()], ww(), None)
            .await
            .unwrap();
        assert_eq!(
            seen_launcher.lock().unwrap().as_deref(),
            Some(Path::new("/fake/landlock-run"))
        );
        assert_eq!(confined.argv[0], "/fake/landlock-run");
        assert_eq!(confined.enforcement, enforcement);
        assert_eq!(confined.denial_signatures, ["permission denied"]);
        let rule = &confined.runner_failure_rules[0];
        assert_eq!(rule.allowed_exit_codes.as_deref(), Some(&[125][..]));
        assert_eq!(rule.fatal_signatures, ["landlock-run: "]);
        assert_eq!(
            rule.informational_lines.as_deref(),
            Some(&["landlock-run: partial enforcement (older Landlock ABI)".into()][..])
        );
    }
}

#[tokio::test]
async fn unsupported_platform_fails_closed_without_probing() {
    let bwrap_probes = Arc::new(AtomicUsize::new(0));
    let landlock_probes = Arc::new(AtomicUsize::new(0));
    let seen_bwrap = bwrap_probes.clone();
    let seen_landlock = landlock_probes.clone();
    let provider = provider_with(SandboxInternals {
        platform: Some("freebsd".into()),
        probe_bwrap: Some(Arc::new(move || {
            seen_bwrap.fetch_add(1, Ordering::SeqCst);
            true
        })),
        probe_landlock: Some(Arc::new(move |_| {
            seen_landlock.fetch_add(1, Ordering::SeqCst);
            ProbeVerdict::Full
        })),
        ..Default::default()
    });
    let cause = provider
        .confine(&["true".into()], ro(), None)
        .await
        .unwrap_err();
    assert_eq!(
        HarnessError::of(cause.as_ref()).unwrap().code(),
        "SANDBOX_UNAVAILABLE"
    );
    assert_eq!(bwrap_probes.load(Ordering::SeqCst), 0);
    assert_eq!(landlock_probes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn sole_candidate_is_selected_without_a_probe() {
    let provider = provider_with(SandboxInternals {
        chain: Some(vec![LocalRunner::Landlock]),
        probe_landlock: Some(Arc::new(|_| panic!("sole candidate must not be probed"))),
        landlock_launcher: Some("/fake/landlock-run".into()),
        ..Default::default()
    });
    let confined = provider
        .confine(&["true".into()], ro(), None)
        .await
        .unwrap();
    assert_eq!(confined.argv[0], "/fake/landlock-run");
    assert_eq!(confined.enforcement, SandboxEnforcement::Full);
}

#[tokio::test]
async fn selected_and_unavailable_verdicts_are_cached() {
    for usable in [true, false] {
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        let provider = provider_with(internals(
            move || {
                seen.fetch_add(1, Ordering::SeqCst);
                usable
            },
            |_| ProbeVerdict::Unusable,
        ));
        for _ in 0..2 {
            let result = provider.confine(&["true".into()], ro(), None).await;
            assert_eq!(result.is_ok(), usable);
        }
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_calls_share_exactly_one_chain_walk() {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let provider = Arc::new(provider_with(internals(
        move || {
            seen.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(20));
            true
        },
        |_| ProbeVerdict::Unusable,
    )));
    let mut calls = Vec::new();
    for _ in 0..8 {
        let provider = provider.clone();
        calls.push(tokio::spawn(async move {
            provider
                .confine(&["true".into()], ro(), None)
                .await
                .unwrap()
        }));
    }
    for call in calls {
        assert_eq!(call.await.unwrap().argv[0], "bwrap");
    }
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_first_confine_does_not_cancel_or_repeat_selection() {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let provider = Arc::new(provider_with(internals(
        move || {
            seen.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(40));
            true
        },
        |_| ProbeVerdict::Unusable,
    )));
    let first = {
        let provider = provider.clone();
        tokio::spawn(async move { provider.confine(&["true".into()], ro(), None).await })
    };
    tokio::task::yield_now().await;
    first.abort();
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(
        provider
            .confine(&["true".into()], ro(), None)
            .await
            .unwrap()
            .argv[0],
        "bwrap"
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_provider_during_bwrap_probe_still_kills_and_reaps_child() {
    let dir = TempDir::new().unwrap();
    let executable = dir.path().join("fake-bwrap");
    let pid_file = dir.path().join("pid");
    fs::write(
        &executable,
        format!(
            "#!/bin/sh\necho $$ > '{}'\nwhile :; do :; done\n",
            pid_file.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    let provider = Arc::new(
        LocalSandboxProvider::new_with_internals(
            Config {
                probe_timeout_ms: 15_000.0,
                ..Default::default()
            },
            SandboxInternals {
                platform: Some("linux".into()),
                bwrap_executable: Some(executable),
                probe_landlock: Some(Arc::new(|_| ProbeVerdict::Unusable)),
                landlock_launcher: Some("/fake/landlock-run".into()),
                ..Default::default()
            },
        )
        .unwrap(),
    );
    let call = {
        let provider = provider.clone();
        tokio::spawn(async move { provider.confine(&["true".into()], ro(), None).await })
    };
    drop(provider);
    for _ in 0..100 {
        if pid_file.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let pid = fs::read_to_string(&pid_file).unwrap();
    let process = PathBuf::from(format!("/proc/{}", pid.trim()));
    assert!(
        process.exists(),
        "fake bwrap did not remain active for the test"
    );
    call.abort();
    for _ in 0..500 {
        if !process.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    panic!("fake bwrap survived provider disposal");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abort_reason_identity_is_preserved_before_and_after_selection() {
    let pre = AbortController::new();
    pre.abort_with(std::io::Error::other("pre-abort"));
    let reason = pre.signal().reason().unwrap();
    let provider = provider_with(internals(|| true, |_| ProbeVerdict::Unusable));
    let cause = provider
        .confine(&["true".into()], ro(), Some(pre.signal()))
        .await
        .unwrap_err();
    assert!(cause.downcast_ref::<SharedError>().unwrap().ptr_eq(&reason));

    let controller = AbortController::new();
    let signal = controller.signal();
    let provider = Arc::new(provider_with(internals(
        || {
            std::thread::sleep(Duration::from_millis(30));
            true
        },
        |_| ProbeVerdict::Unusable,
    )));
    let call = {
        let provider = provider.clone();
        tokio::spawn(async move { provider.confine(&["true".into()], ro(), Some(signal)).await })
    };
    tokio::task::yield_now().await;
    controller.abort_with(std::io::Error::other("late-abort"));
    let reason = controller.signal().reason().unwrap();
    let cause = call.await.unwrap().unwrap_err();
    assert!(cause.downcast_ref::<SharedError>().unwrap().ptr_eq(&reason));
}

fn fake_launcher(script: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("landlock-run");
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    (dir, path)
}

#[tokio::test]
async fn default_landlock_probe_parses_full_partial_and_failure() {
    for (script, expected) in [
        (
            "#!/bin/sh\necho 'landlock: fully enforced'\nexit 0\n",
            Some(SandboxEnforcement::Full),
        ),
        (
            "#!/bin/sh\necho 'landlock: partially enforced (older ABI)'\nexit 0\n",
            Some(SandboxEnforcement::Partial),
        ),
        ("#!/bin/sh\nexit 125\n", None),
    ] {
        let (_dir, launcher) = fake_launcher(script);
        let provider = provider_with(SandboxInternals {
            platform: Some("linux".into()),
            probe_bwrap: Some(Arc::new(|| false)),
            landlock_launcher: Some(launcher),
            ..Default::default()
        });
        let result = provider.confine(&["true".into()], ro(), None).await;
        match expected {
            Some(enforcement) => assert_eq!(result.unwrap().enforcement, enforcement),
            None => assert_eq!(
                HarnessError::of(result.unwrap_err().as_ref())
                    .unwrap()
                    .code(),
                "SANDBOX_UNAVAILABLE"
            ),
        }
    }
}

#[tokio::test]
async fn real_default_linux_probe_chain_is_usable_or_fails_closed() {
    for provider in [
        LocalSandboxProvider::new_with_internals(
            Config::default(),
            SandboxInternals {
                platform: Some("linux".into()),
                ..Default::default()
            },
        )
        .unwrap(),
        LocalSandboxProvider::new(Config::default()).unwrap(),
    ] {
        match provider.confine(&["true".into()], ro(), None).await {
            Ok(confined) => {
                assert!(confined.argv[0] == "bwrap" || confined.argv[0].ends_with("landlock-run"))
            }
            Err(cause) => assert_eq!(
                HarnessError::of(cause.as_ref()).unwrap().code(),
                "SANDBOX_UNAVAILABLE"
            ),
        }
    }
}

#[tokio::test]
async fn probe_timeout_accepts_a_slow_success_then_bounds_and_fails_closed() {
    let (_dir, launcher) =
        fake_launcher("#!/bin/sh\nsleep 1\necho 'landlock: fully enforced'\nexit 0\n");
    let provider = LocalSandboxProvider::new_with_internals(
        Config {
            probe_timeout_ms: 15_000.0,
            ..Default::default()
        },
        SandboxInternals {
            platform: Some("linux".into()),
            probe_bwrap: Some(Arc::new(|| false)),
            landlock_launcher: Some(launcher.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        provider
            .confine(&["true".into()], ro(), None)
            .await
            .unwrap()
            .enforcement,
        SandboxEnforcement::Full
    );

    let provider = LocalSandboxProvider::new_with_internals(
        Config {
            probe_timeout_ms: 20.0,
            ..Default::default()
        },
        SandboxInternals {
            platform: Some("linux".into()),
            probe_bwrap: Some(Arc::new(|| false)),
            landlock_launcher: Some(launcher),
            ..Default::default()
        },
    )
    .unwrap();
    let cause = provider
        .confine(&["true".into()], ro(), None)
        .await
        .unwrap_err();
    assert_eq!(
        HarnessError::of(cause.as_ref()).unwrap().code(),
        "SANDBOX_UNAVAILABLE"
    );
}
