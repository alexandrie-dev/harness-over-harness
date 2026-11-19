//! Deterministic real-process proofs for runner classification: the real local provider and the
//! sandbox bash executor exercise direct runner-spawn failures and a POSIX fake Landlock launcher
//! that prints its notice before exec.

mod support;

use std::{fs, os::unix::fs::PermissionsExt, path::Path, sync::Arc};

use hoh_error::BoxError;
use hoh_node_addon_system::LAUNCHER_FAILURE_EXIT;
use hoh_sandbox::{SandboxEnforcement, SandboxMode};
use hoh_sandbox_local::{Config, LocalSandboxProvider, ProbeVerdict, SandboxInternals};
use hoh_shell::{ShellExecRequest, ShellProcessStatus, ShellSandboxInfo};
use serde_json::json;
use support::*;

const NOTICE: &str = "landlock-run: partial enforcement (older Landlock ABI)";
const FATAL_PREFIX: &str = "landlock-run: ";
const FATAL: &str = "landlock-run: landlock ruleset error: Invalid argument";
const FAILURE_EXIT: i64 = LAUNCHER_FAILURE_EXIT as i64;

fn write_executable(path: &Path, content: &str, mode: u32) {
    fs::write(path, content).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn cwd() -> String {
    std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

/// Write a fake native launcher that reports partial enforcement, then execs or fails.
fn fake_launcher(dir: &Path, fatal_exit: Option<i64>) -> std::path::PathBuf {
    let launcher = dir.join("landlock-run");
    let fatal_branch = match fatal_exit {
        None => String::new(),
        Some(exit) => format!("printf '%s\\n' '{FATAL}' >&2\nexit {exit}\n"),
    };
    write_executable(
        &launcher,
        &format!(
            "#!/bin/sh
while [ \"$#\" -gt 0 ]; do
  case \"$1\" in
    --ro|--rw) shift 2 ;;
    --) shift; break ;;
    *) printf '%s\\n' '{FATAL_PREFIX}usage error: unexpected fake argument' >&2; exit {FAILURE_EXIT} ;;
  esac
done
printf '%s\\n' '{NOTICE}' >&2
{fatal_branch}exec \"$@\"
"
        ),
        0o755,
    );
    launcher
}

struct Partial {
    real: Real,
    _launcher_dir: tempfile::TempDir,
}

async fn setup_partial(fatal_exit: Option<i64>) -> Partial {
    let launcher_dir = tempfile::tempdir().unwrap();
    let launcher = fake_launcher(launcher_dir.path(), fatal_exit);
    let provider = LocalSandboxProvider::new_with_internals(
        Config::default(),
        SandboxInternals {
            platform: Some("linux".into()),
            probe_bwrap: Some(Arc::new(|| false)),
            probe_landlock: Some(Arc::new(|_| ProbeVerdict::Partial)),
            landlock_launcher: Some(launcher),
            ..SandboxInternals::default()
        },
    )
    .unwrap();
    Partial {
        real: setup_real(
            Arc::new(provider),
            json!({"cwd": cwd(), "timeoutMs": 5_000}),
            "read-only",
            &cwd(),
        )
        .await,
        _launcher_dir: launcher_dir,
    }
}

async fn setup_configured_runner(runner: &str) -> Real {
    let provider = LocalSandboxProvider::new(Config {
        runner_command: vec![runner.to_owned()],
        runner_failure_signatures: vec!["configured-runner: fatal".into()],
        ..Config::default()
    })
    .unwrap();
    setup_real(
        Arc::new(provider),
        json!({"cwd": cwd(), "timeoutMs": 5_000}),
        "read-only",
        &cwd(),
    )
    .await
}

fn partial(denied: bool) -> Option<ShellSandboxInfo> {
    Some(ShellSandboxInfo {
        mode: SandboxMode::ReadOnly,
        denied,
        enforcement: Some(SandboxEnforcement::Partial),
        runner_failed: None,
    })
}

fn full_runner_failed() -> Option<ShellSandboxInfo> {
    Some(ShellSandboxInfo {
        mode: SandboxMode::ReadOnly,
        denied: false,
        enforcement: Some(SandboxEnforcement::Full),
        runner_failed: Some(true),
    })
}

fn message(error: &BoxError) -> String {
    error.to_string()
}

mod partial_landlock_runner_failure_classification {
    use super::*;

    async fn classifies_a_configured_runner_through_the_direct_spawn_error_channel(kind: &str) {
        let dir = tempfile::tempdir().unwrap();
        let runner = canonical(dir.path().join(format!("{kind}-runner")));
        match kind {
            "unexecutable" => write_executable(Path::new(&runner), "#!/bin/sh\nexit 0\n", 0o644),
            "missing-interpreter" => write_executable(
                Path::new(&runner),
                "#!/dsh-definitely-missing-sandbox-interpreter\nexit 0\n",
                0o755,
            ),
            _ => {}
        }
        let bash = setup_configured_runner(&runner).await;

        let error = bash.run(bash.resolve(request("true"))).await.unwrap_err();
        assert!(is_sandbox_unavailable(&error), "{error}");
        assert!(message(&error).contains(&runner), "{error}");

        let task = bash.start(bash.resolve(request("true"))).await.unwrap();
        task.done().await;
        assert_eq!(task.status(), ShellProcessStatus::Killed);
        assert!(
            task.read_output().delta.contains(&format!(
                "subprocess failed before reporting an outcome: Error: spawn {runner}"
            )),
            "{}",
            task.read_output().delta
        );
        assert_eq!(task.sandbox(), full_runner_failed());
        bash.close().await;
    }

    fn canonical(path: impl AsRef<Path>) -> String {
        path.as_ref().to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn classifies_a_missing_configured_runner_through_the_direct_spawn_error_channel() {
        classifies_a_configured_runner_through_the_direct_spawn_error_channel("missing").await;
    }

    #[tokio::test]
    async fn classifies_a_unexecutable_configured_runner_through_the_direct_spawn_error_channel() {
        classifies_a_configured_runner_through_the_direct_spawn_error_channel("unexecutable").await;
    }

    #[tokio::test]
    async fn classifies_a_missing_interpreter_configured_runner_through_the_direct_spawn_error_channel()
     {
        classifies_a_configured_runner_through_the_direct_spawn_error_channel(
            "missing-interpreter",
        )
        .await;
    }

    async fn classifies_a_runner_whose_shebang_interpreter_is_missing(form: &str) {
        let dir = tempfile::tempdir().unwrap();
        let filename = "missing-interpreter-runner";
        let runner = if form == "bare-name" {
            filename.to_owned()
        } else {
            format!("./{filename}")
        };
        write_executable(
            &dir.path().join(filename),
            "#!/dsh-definitely-missing-sandbox-interpreter\nexit 0\n",
            0o755,
        );
        let bash = setup_configured_runner(&runner).await;
        let dir_text = canonical(dir.path());
        let make = || {
            if form == "bare-name" {
                ShellExecRequest {
                    env: Some(
                        [("PATH".to_owned(), dir_text.clone())]
                            .into_iter()
                            .collect(),
                    ),
                    ..request("true")
                }
            } else {
                ShellExecRequest {
                    workdir: Some(dir_text.clone()),
                    ..request("true")
                }
            }
        };

        let error = bash.run(bash.resolve(make())).await.unwrap_err();
        assert!(is_sandbox_unavailable(&error), "{form}: {error}");
        // The passed bare/relative argv[0] is preserved in the spawn error, not resolved.
        assert!(
            message(&error).contains(&format!("spawn {runner} ENOENT")),
            "{form}: {error}"
        );

        let task = bash.start(bash.resolve(make())).await.unwrap();
        task.done().await;
        assert_eq!(task.status(), ShellProcessStatus::Killed, "{form}");
        assert!(
            task.read_output().delta.contains(&format!(
                "subprocess failed before reporting an outcome: Error: spawn {runner} ENOENT"
            )),
            "{form}: {}",
            task.read_output().delta
        );
        assert_eq!(task.sandbox(), full_runner_failed(), "{form}");
        bash.close().await;
    }

    #[tokio::test]
    async fn classifies_a_bare_name_runner_whose_shebang_interpreter_is_missing() {
        classifies_a_runner_whose_shebang_interpreter_is_missing("bare-name").await;
    }

    #[tokio::test]
    async fn classifies_a_relative_runner_whose_shebang_interpreter_is_missing() {
        classifies_a_runner_whose_shebang_interpreter_is_missing("relative").await;
    }

    #[tokio::test]
    async fn keeps_a_real_malformed_executable_ordinary_across_no_shebang_spawn_behavior() {
        let dir = tempfile::tempdir().unwrap();
        let runner = dir.path().join("malformed-runner");
        write_executable(
            &runner,
            "not a native executable or shebang script\n",
            0o755,
        );
        let bash = setup_configured_runner(&runner.to_string_lossy()).await;

        // The OS may expose execve's ENOEXEC directly or retry a no-shebang executable through
        // /bin/sh. Neither supplies the ENOENT/EACCES with the exact failed executable path
        // required for runner attribution.
        match bash.run(bash.resolve(request("true"))).await {
            Err(foreground) => {
                assert!(!is_sandbox_unavailable(&foreground), "{foreground}");
                assert_eq!(error_code(&foreground).as_deref(), Some("ENOEXEC"));
                // Containment: the handle settles killed instead of the spawn throwing.
                let background = bash.start(bash.resolve(request("true"))).await.unwrap();
                background.done().await;
                assert_eq!(background.status(), ShellProcessStatus::Killed);
                let error = background.result().await.unwrap_err();
                assert_eq!(error_code(&error).as_deref(), Some("ENOEXEC"));
                assert!(!is_sandbox_unavailable(&error));
            }
            Ok(foreground) => {
                assert_eq!(foreground.exit_code, Some(127));
                assert_eq!(foreground.signal, None);
                assert_eq!(
                    foreground.sandbox,
                    Some(ShellSandboxInfo {
                        mode: SandboxMode::ReadOnly,
                        denied: false,
                        enforcement: Some(SandboxEnforcement::Full),
                        runner_failed: None,
                    })
                );
                assert!(!foreground.stderr.text.is_empty());

                let background = bash.start(bash.resolve(request("true"))).await.unwrap();
                background.done().await;
                assert_eq!(background.status(), ShellProcessStatus::Completed);
                assert_eq!(background.exit_code(), Some(127));
                assert_eq!(background.signal(), None);
                assert_eq!(
                    background.sandbox(),
                    Some(ShellSandboxInfo {
                        mode: SandboxMode::ReadOnly,
                        denied: false,
                        enforcement: Some(SandboxEnforcement::Full),
                        runner_failed: None,
                    })
                );
                let output = background.read_output().delta;
                assert!(output.starts_with("[stderr]\n"), "{output}");
                assert!(output.len() > "[stderr]\n".len());
                assert!(!output.contains("subprocess failed before reporting an outcome:"));
            }
        }
        bash.close().await;
    }

    async fn keeps_child_exit_ordinary_when_the_notice_is_the_only_runner_line(exit_code: i64) {
        let test = setup_partial(None).await;
        let bash = &test.real;
        let result = bash
            .run(bash.resolve(request(&format!("exit {exit_code}"))))
            .await
            .unwrap();
        assert_eq!(result.exit_code, Some(exit_code));
        assert_eq!(result.stderr.text, format!("{NOTICE}\n"));
        assert_eq!(result.sandbox, partial(false));
    }

    #[tokio::test]
    async fn keeps_child_exit_0_ordinary_when_the_partial_enforcement_notice_is_the_only_runner_line()
     {
        keeps_child_exit_ordinary_when_the_notice_is_the_only_runner_line(0).await;
    }

    #[tokio::test]
    async fn keeps_child_exit_1_ordinary_when_the_partial_enforcement_notice_is_the_only_runner_line()
     {
        keeps_child_exit_ordinary_when_the_notice_is_the_only_runner_line(1).await;
    }

    #[tokio::test]
    async fn keeps_child_exit_2_ordinary_when_the_partial_enforcement_notice_is_the_only_runner_line()
     {
        keeps_child_exit_ordinary_when_the_notice_is_the_only_runner_line(2).await;
    }

    #[tokio::test]
    async fn keeps_launcher_failure_exit_ordinary_when_the_partial_enforcement_notice_is_the_only_runner_line()
     {
        keeps_child_exit_ordinary_when_the_notice_is_the_only_runner_line(FAILURE_EXIT).await;
    }

    async fn keeps_a_successfully_launched_child_exit_ordinary(exit_code: i64) {
        let test = setup_partial(None).await;
        let bash = &test.real;
        let result = bash
            .run(bash.resolve(request(&format!("exit {exit_code}"))))
            .await
            .unwrap();
        assert_eq!(result.exit_code, Some(exit_code));
        assert_eq!(result.stderr.text, format!("{NOTICE}\n"));
        assert_eq!(result.sandbox, partial(false));
    }

    #[tokio::test]
    async fn keeps_a_successfully_launched_landlock_child_exit_126_as_an_ordinary_outcome() {
        keeps_a_successfully_launched_child_exit_ordinary(126).await;
    }

    #[tokio::test]
    async fn keeps_a_successfully_launched_landlock_child_exit_127_as_an_ordinary_outcome() {
        keeps_a_successfully_launched_child_exit_ordinary(127).await;
    }

    async fn keeps_a_fatal_line_at_a_non_launcher_exit_as_insufficient_evidence(exit_code: i64) {
        let test = setup_partial(Some(exit_code)).await;
        let bash = &test.real;
        let result = bash.run(bash.resolve(request("true"))).await.unwrap();
        assert_eq!(result.exit_code, Some(exit_code));
        assert_eq!(result.stderr.text, format!("{NOTICE}\n{FATAL}\n"));
        assert_eq!(result.sandbox, partial(false));
    }

    #[tokio::test]
    async fn keeps_a_landlock_fatal_line_at_exit_1_as_insufficient_runner_failure_evidence() {
        keeps_a_fatal_line_at_a_non_launcher_exit_as_insufficient_evidence(1).await;
    }

    #[tokio::test]
    async fn keeps_a_landlock_fatal_line_at_exit_2_as_insufficient_runner_failure_evidence() {
        keeps_a_fatal_line_at_a_non_launcher_exit_as_insufficient_evidence(2).await;
    }

    #[tokio::test]
    async fn reports_the_fatal_line_after_the_notice_as_sandbox_unavailable_detail() {
        let test = setup_partial(Some(FAILURE_EXIT)).await;
        let bash = &test.real;
        let error = bash.run(bash.resolve(request("true"))).await.unwrap_err();
        assert!(is_sandbox_unavailable(&error), "{error}");
        assert!(
            message(&error).contains(&format!("Runner failure: {FATAL}")),
            "{error}"
        );
        assert!(!message(&error).contains(NOTICE), "{error}");
    }

    #[tokio::test]
    async fn classifies_a_notice_plus_child_permission_denied_as_a_denial_not_runner_failure() {
        let test = setup_partial(None).await;
        let bash = &test.real;
        let result = bash
            .run(bash.resolve(request(
                "printf \"%s\\n\" \"child: Permission denied\" >&2; exit 1",
            )))
            .await
            .unwrap();
        assert_eq!(
            result.stderr.text,
            format!("{NOTICE}\nchild: Permission denied\n")
        );
        assert_eq!(result.sandbox, partial(true));
    }

    #[tokio::test]
    async fn applies_the_same_evidence_rule_to_notice_only_background_exits() {
        let test = setup_partial(None).await;
        let bash = &test.real;
        for command in [
            "exit 1".to_owned(),
            "exit 2".to_owned(),
            format!("exit {FAILURE_EXIT}"),
        ] {
            let task = bash.start(bash.resolve(request(&command))).await.unwrap();
            task.done().await;
            assert_eq!(task.sandbox(), partial(false), "{command}");
            assert!(task.read_output().delta.contains(NOTICE), "{command}");
        }
    }

    #[tokio::test]
    async fn classifies_a_background_notice_plus_child_permission_denied_as_denial() {
        let test = setup_partial(None).await;
        let bash = &test.real;
        let task = bash
            .start(bash.resolve(request(
                "printf \"%s\\n\" \"child: Permission denied\" >&2; exit 1",
            )))
            .await
            .unwrap();
        task.done().await;
        assert_eq!(task.sandbox(), partial(true));
        assert!(task.read_output().delta.contains(NOTICE));
    }

    #[tokio::test]
    async fn makes_a_background_fatal_line_outrank_denial_text_after_the_notice() {
        let test = setup_partial(Some(FAILURE_EXIT)).await;
        let bash = &test.real;
        let task = bash.start(bash.resolve(request("true"))).await.unwrap();
        task.done().await;
        assert_eq!(
            task.sandbox(),
            Some(ShellSandboxInfo {
                mode: SandboxMode::ReadOnly,
                denied: false,
                enforcement: Some(SandboxEnforcement::Partial),
                runner_failed: Some(true),
            })
        );
        let output = task.read_output().delta;
        assert!(output.contains(NOTICE));
        assert!(output.contains(FATAL));
    }
}
