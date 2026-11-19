//! Consumer-side `SandboxBashExecutor` tests. A fake `sandbox` service makes wrapping, policy
//! hand-off, fail-closed propagation, classification and fact stamping deterministic;
//! real-provider integration lives in `tests/landlock_e2e.rs` and `tests/bwrap_e2e.rs`. A
//! mode-0555 directory supplies the Unix denial signature used by the classifier without
//! requiring a real sandbox runner.

mod support;

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use hoh_error::{BoxError, SharedError};
use hoh_sandbox::{
    ConfinedArgv, ConfinedSandboxMode, SandboxEnforcement, SandboxExecutionPolicy, SandboxMode,
    SandboxPolicy, SandboxUnavailableError,
};
use hoh_shell::{
    ShellExecRequest, ShellProcessStatus, ShellRunResult, ShellSandboxInfo, types::CollectedOutput,
};
use hoh_subprocess::{
    SubprocessCollectedOutputs, SubprocessForeignError, SubprocessFuture, SubprocessHandle,
    SubprocessLifecycle, SubprocessOutcome, SubprocessOutputRead, SubprocessOutputReader,
    SubprocessSpawnRejection, subprocess_done,
};
use hoh_timeout::{AbortController, AbortSignal};
use serde_json::json;
use support::*;
use tokio::sync::oneshot;

fn canonical(path: impl AsRef<Path>) -> String {
    path.as_ref().to_string_lossy().into_owned()
}

fn cwd() -> String {
    std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn info(
    mode: SandboxMode,
    denied: bool,
    enforcement: Option<SandboxEnforcement>,
) -> ShellSandboxInfo {
    ShellSandboxInfo {
        mode,
        denied,
        enforcement,
        runner_failed: None,
    }
}

fn full(mode: SandboxMode, denied: bool) -> ShellSandboxInfo {
    info(mode, denied, Some(SandboxEnforcement::Full))
}

fn execution_policy(mode: SandboxMode, workspace_root: &str) -> SandboxExecutionPolicy {
    SandboxExecutionPolicy {
        mode,
        workspace_root: workspace_root.to_owned(),
        additional_writable_roots: None,
        session_id: None,
    }
}

fn with_policy(command: &str, policy: SandboxExecutionPolicy) -> ShellExecRequest {
    ShellExecRequest {
        sandbox_policy: Some(policy),
        ..request(command)
    }
}

/// A whole policy, as the source's `toEqual` sees it: absent optional fields are not serialized.
fn policy_json(policy: &SandboxPolicy) -> serde_json::Value {
    serde_json::to_value(policy).unwrap()
}

fn exec_policy_json(policy: &SandboxExecutionPolicy) -> serde_json::Value {
    serde_json::to_value(policy).unwrap()
}

fn reason(text: &str) -> BoxError {
    Box::new(std::io::Error::other(text.to_owned()))
}

/// The abort `reason` the failed call must rethrow: same message, shared identity of the signal's.
fn assert_reason(error: &BoxError, signal: &AbortSignal, text: &str) {
    assert_eq!(error.to_string(), text);
    if let (Some(thrown), Some(expected)) = (error.downcast_ref::<SharedError>(), signal.reason()) {
        assert!(
            thrown.ptr_eq(&expected),
            "the caller's own reason is rethrown"
        );
    }
}

struct EmptyReader;
impl SubprocessOutputReader for EmptyReader {
    fn read_from(&self, from_byte: u64) -> SubprocessOutputRead {
        SubprocessOutputRead {
            text: String::new(),
            next_offset: from_byte,
            lossy: false,
            spill_path: None,
        }
    }
}

struct QuietLifecycle;
impl SubprocessLifecycle for QuietLifecycle {
    fn terminate(&self) {}
    fn wait_for_exit(&self, _signal: Option<AbortSignal>) -> SubprocessFuture<'_, bool> {
        Box::pin(async { Ok(true) })
    }
}

/// A handle whose `done` is settled by the returned sender.
fn pending_handle() -> (
    SubprocessHandle,
    oneshot::Sender<Result<SubprocessOutcome, SharedError>>,
) {
    let (settle, settled) = oneshot::channel();
    let reader: Arc<dyn SubprocessOutputReader> = Arc::new(EmptyReader);
    let handle = SubprocessHandle {
        stdin: None,
        stdout: None,
        stderr: None,
        control: None,
        collected: SubprocessCollectedOutputs {
            stdout: Some(reader.clone()),
            stderr: Some(reader),
        },
        done: subprocess_done(async move {
            settled
                .await
                .unwrap_or_else(|_| Err(SharedError::new(std::io::Error::other("settle dropped"))))
        }),
        lifecycle: Arc::new(QuietLifecycle),
    };
    (handle, settle)
}

/// A handle whose `done` already rejected with `error`.
fn failed_handle(error: SharedError) -> SubprocessHandle {
    let reader: Arc<dyn SubprocessOutputReader> = Arc::new(EmptyReader);
    SubprocessHandle {
        stdin: None,
        stdout: None,
        stderr: None,
        control: None,
        collected: SubprocessCollectedOutputs {
            stdout: Some(reader.clone()),
            stderr: Some(reader),
        },
        done: subprocess_done(async move { Err(error) }),
        lifecycle: Arc::new(QuietLifecycle),
    }
}

/// A behavior that parks until the test answers; the signal it was given is reported.
struct Parked {
    behavior: Behavior,
    entered: Mutex<Option<oneshot::Receiver<Option<AbortSignal>>>>,
    respond: Mutex<Option<oneshot::Sender<ConfinedArgv>>>,
}

fn parked() -> Parked {
    let (entered_tx, entered_rx) = oneshot::channel();
    let (respond_tx, respond_rx) = oneshot::channel::<ConfinedArgv>();
    let entered_tx = Mutex::new(Some(entered_tx));
    let respond_rx = Mutex::new(Some(respond_rx));
    let behavior: Behavior = Arc::new(move |_argv, _policy, signal| {
        if let Some(entered) = lock(&entered_tx).take() {
            let _ = entered.send(signal);
        }
        let response = lock(&respond_rx).take();
        Box::pin(async move {
            match response {
                Some(response) => response.await.map_err(|_| {
                    Box::new(std::io::Error::other("confinement dropped")) as BoxError
                }),
                None => Err("confinement already answered".into()),
            }
        })
    });
    Parked {
        behavior,
        entered: Mutex::new(Some(entered_rx)),
        respond: Mutex::new(Some(respond_tx)),
    }
}

impl Parked {
    async fn entered(&self) -> Option<AbortSignal> {
        let entered = lock(&self.entered).take().expect("entered once");
        entered.await.expect("the provider was consulted")
    }

    fn respond(&self, wrap: ConfinedArgv) {
        if let Some(respond) = lock(&self.respond).take() {
            let _ = respond.send(wrap);
        }
    }
}

async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

mod the_provider_hand_off {
    use super::*;

    #[tokio::test]
    async fn reports_caller_cancellation_when_a_failed_launch_result_is_read_after_abort() {
        let s = setup(Options {
            behavior: immediate(|_, _| Ok(wrap(strings(&["node"])))),
            ..Options::default()
        })
        .await;
        let controller = AbortController::new();
        s.subprocess
            .set(SpawnMode::Throw(Arc::new(|| spawn_failure(2, "node"))));
        let execution = s
            .shell
            .execute(s.resolve(ShellExecRequest {
                signal: Some(controller.signal()),
                ..request("true")
            }))
            .await
            .unwrap();
        execution.done().await;
        controller.abort_with(reason("caller abandoned the failed launch"));
        let error = execution.result().await.unwrap_err();
        assert_reason(
            &error,
            &controller.signal(),
            "caller abandoned the failed launch",
        );
        s.close().await;
    }

    #[tokio::test]
    async fn preserves_cancellation_when_a_pending_subprocess_launch_later_rejects() {
        let s = setup(Options {
            behavior: immediate(|_, _| Ok(wrap(strings(&["node"])))),
            ..Options::default()
        })
        .await;
        let (handle, settle_done) = pending_handle();
        let handle = Mutex::new(Some(handle));
        s.subprocess.set(SpawnMode::Handle(Arc::new(move |_| {
            lock(&handle).take().expect("one spawn")
        })));
        let controller = AbortController::new();
        let spec = s.resolve(ShellExecRequest {
            signal: Some(controller.signal()),
            ..request("true")
        });
        let shell = s.shell.clone();
        let pending = tokio::spawn(async move { shell.execute(spec).await?.result().await });
        while s.subprocess.spawn_count() == 0 {
            tokio::task::yield_now().await;
        }
        controller.abort_with(reason("caller cancelled pending launch"));
        let _ = settle_done.send(Err(SharedError::new(spawn_failure(2, "node"))));
        let result = pending.await.unwrap().unwrap();
        assert!(result.aborted);
        assert!(!result.timed_out);
        s.close().await;
    }

    async fn cancels_while_confinement_is_pending_without_spawning(expiry: &str) {
        let parked = parked();
        let s = setup(Options {
            behavior: parked.behavior.clone(),
            ..Options::default()
        })
        .await;
        let controller = AbortController::new();
        let spec = s.resolve(ShellExecRequest {
            on_expiry: Some(match expiry {
                "none" => hoh_shell::ShellExpiryPolicy::None,
                _ => hoh_shell::ShellExpiryPolicy::Kill,
            }),
            signal: Some(controller.signal()),
            ..request("true")
        });
        let shell = s.shell.clone();
        let pending = tokio::spawn(async move { shell.execute(spec).await.map(|_| ()) });
        let signal = parked
            .entered()
            .await
            .expect("preparation carries a signal");
        assert_eq!(s.subprocess.spawn_count(), 0);
        controller.abort_with(reason("cancel pending confinement"));
        assert!(signal.aborted());
        parked.respond(passthrough(&strings(&["bash", "-c", "true"])));
        let error = pending.await.unwrap().unwrap_err();
        assert_reason(&error, &controller.signal(), "cancel pending confinement");
        assert_eq!(s.subprocess.spawn_count(), 0);
        s.close().await;
    }

    #[tokio::test]
    async fn cancels_kill_while_confinement_is_pending_without_spawning() {
        cancels_while_confinement_is_pending_without_spawning("kill").await;
    }

    #[tokio::test]
    async fn cancels_none_while_confinement_is_pending_without_spawning() {
        cancels_while_confinement_is_pending_without_spawning("none").await;
    }

    #[tokio::test]
    async fn publishes_a_background_process_only_after_confinement_completes() {
        let parked = parked();
        let s = setup(Options {
            behavior: parked.behavior.clone(),
            ..Options::default()
        })
        .await;
        let spec = s.resolve(request("printf ready"));
        let shell = s.shell.clone();
        let pending = tokio::spawn(async move {
            shell
                .execute(hoh_shell::ShellExecSpec {
                    on_expiry: hoh_shell::ShellExpiryPolicy::None,
                    ..spec
                })
                .await
        });
        parked.entered().await;
        settle().await;
        assert!(
            !pending.is_finished(),
            "published before confinement completed"
        );
        assert_eq!(s.subprocess.spawn_count(), 0);
        parked.respond(passthrough(&strings(&["bash", "-c", "printf ready"])));
        let process = pending.await.unwrap().unwrap();
        process.done().await;
        assert_eq!(process.read_output().delta, "ready");
        assert_eq!(s.subprocess.spawn_count(), 1);
        s.close().await;
    }

    #[tokio::test]
    async fn hands_the_provider_the_exact_bash_argv_and_the_per_call_policy_and_runs_the_returned_argv()
     {
        let s = setup(Options::default()).await;
        let command = "echo 'a b' \"c'd\"";
        let result = run(&s, s.resolve(request(command))).await.unwrap();
        assert_eq!(result.stdout.text, "a b c'd\n");
        assert_eq!(result.sandbox, Some(full(SandboxMode::ReadOnly, false)));
        let calls = s.sandbox.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].argv, strings(&["bash", "-c", command]));
        assert_eq!(
            policy_json(&calls[0].policy),
            json!({"mode": "read-only", "workspaceRoot": cwd()})
        );
        s.close().await;
    }

    #[tokio::test]
    async fn hands_the_providers_returned_argv_directly_to_subprocess_spawn() {
        let returned = strings(&[
            "env",
            "DSH_WRAP=1",
            "bash",
            "-c",
            "printf \"%s\" \"$DSH_WRAP\"",
        ]);
        let s = setup(Options {
            behavior: {
                let returned = returned.clone();
                immediate(move |_, _| Ok(wrap(returned.clone())))
            },
            ..Options::default()
        })
        .await;
        let result = run(&s, s.resolve(request("printf \"%s\" \"$DSH_WRAP\"")))
            .await
            .unwrap();
        assert_eq!(result.stdout.text, "1");
        let spawns = s.subprocess.spawns();
        assert_eq!(spawns.len(), 1);
        assert_eq!(spawns[0].argv, returned);
        assert_eq!(result.sandbox, Some(full(SandboxMode::ReadOnly, false)));
        s.close().await;
    }

    #[tokio::test]
    async fn starts_a_non_bash_runner_before_the_confined_inner_bash_evaluates_bash_env() {
        let dir = tempfile::tempdir().unwrap();
        let hook = dir.path().join("hook.sh");
        let order = dir.path().join("order.txt");
        fs::write(&hook, "printf \"hook\\n\" >> \"$DSH_ORDER_FILE\"\n").unwrap();
        // The runner is `sh`, not bash: it records itself, then hands over to the inner argv.
        let runner = "printf \"runner\\n\" >> \"$DSH_ORDER_FILE\"; exec \"$@\"";
        let s = setup(Options {
            behavior: immediate(move |argv, _| {
                let mut wrapped = strings(&["sh", "-c", runner, "sh"]);
                wrapped.extend(argv.iter().cloned());
                Ok(wrap(wrapped))
            }),
            ..Options::default()
        })
        .await;
        let result = run(
            &s,
            s.resolve(ShellExecRequest {
                env: Some(
                    [("BASH_ENV".to_owned(), canonical(&hook))]
                        .into_iter()
                        .collect(),
                ),
                dsh_env: Some(
                    [("DSH_ORDER_FILE".to_owned(), canonical(&order))]
                        .into_iter()
                        .collect(),
                ),
                ..request("true")
            }),
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(fs::read_to_string(&order).unwrap(), "runner\nhook\n");
        s.close().await;
    }

    #[tokio::test]
    async fn workspace_write_rides_the_policy_workspace_root_falling_back_to_process_cwd_when_not_configured()
     {
        let s = setup(Options {
            mode: Some("workspace-write"),
            ..Options::default()
        })
        .await;
        let result = run(&s, s.resolve(request("true"))).await.unwrap();
        assert_eq!(
            result.sandbox,
            Some(full(SandboxMode::WorkspaceWrite, false))
        );
        let calls = s.sandbox.calls();
        assert_eq!(
            policy_json(&calls[0].policy),
            json!({"mode": "workspace-write", "workspaceRoot": cwd()})
        );
        s.close().await;
    }

    #[tokio::test]
    async fn an_explicit_workspace_root_on_the_policy_wins() {
        let s = setup(Options {
            mode: Some("workspace-write"),
            workspace_root: Some("/ws".into()),
            executor: json!({"cwd": canonical(std::env::temp_dir())}),
            ..Options::default()
        })
        .await;
        run(&s, s.resolve(request("true"))).await.unwrap();
        assert_eq!(
            policy_json(&s.sandbox.calls()[0].policy),
            json!({"mode": "workspace-write", "workspaceRoot": "/ws"})
        );
        s.close().await;
    }

    #[tokio::test]
    async fn the_provider_is_consulted_per_wrap_no_caching_in_the_consumer_run_and_start_each_hand_off()
     {
        let s = setup(Options::default()).await;
        run(&s, s.resolve(request("true"))).await.unwrap();
        let task = start(&s, s.resolve(request("true"))).await.unwrap();
        task.done().await;
        assert_eq!(s.sandbox.calls().len(), 2);
        s.close().await;
    }
}

mod fail_closed {
    use super::*;

    fn runner_forms() -> [(&'static str, String); 3] {
        [
            ("absolute", "/usr/bin/env".to_owned()),
            ("bare", "env".to_owned()),
            ("relative", "./sandbox-runner".to_owned()),
        ]
    }

    #[tokio::test]
    async fn propagates_the_providers_structured_sandbox_unavailable_on_run_and_start() {
        let s = setup(Options {
            behavior: immediate(|_, _| {
                Err(Box::new(SandboxUnavailableError::new(
                    ConfinedSandboxMode::ReadOnly,
                    None,
                )) as BoxError)
            }),
            ..Options::default()
        })
        .await;
        let spec = s.resolve(request("echo hi"));
        let error = run(&s, spec.clone()).await.unwrap_err();
        assert!(is_sandbox_unavailable(&error), "{error}");
        let error = start(&s, spec).await.err().expect("start rejects");
        assert!(is_sandbox_unavailable(&error), "{error}");
        s.close().await;
    }

    #[tokio::test]
    async fn preserves_an_already_aborted_foreground_call_as_cancellation() {
        let s = setup(Options::default()).await;
        let controller = AbortController::new();
        controller.abort_with(reason("caller cancelled before spawn"));
        let error = run(
            &s,
            s.resolve(ShellExecRequest {
                signal: Some(controller.signal()),
                ..request("true")
            }),
        )
        .await
        .unwrap_err();
        assert_reason(
            &error,
            &controller.signal(),
            "caller cancelled before spawn",
        );
        s.close().await;
    }

    #[tokio::test]
    async fn keeps_an_invalid_workdir_ordinary_with_every_provider_runner_form() {
        for (form, runner) in runner_forms() {
            let s = setup(Options {
                behavior: {
                    let runner = runner.clone();
                    immediate(move |argv, _| {
                        let mut wrapped = vec![runner.clone()];
                        wrapped.extend(argv.iter().cloned());
                        Ok(wrap(wrapped))
                    })
                },
                ..Options::default()
            })
            .await;
            let parent = tempfile::tempdir().unwrap();
            let missing = canonical(parent.path().join("missing"));
            let failure = run(
                &s,
                s.resolve(ShellExecRequest {
                    workdir: Some(missing),
                    ..request("true")
                }),
            )
            .await
            .unwrap_err();
            assert_eq!(
                error_code(&failure).as_deref(),
                Some("ENOENT"),
                "{form}: {failure}"
            );
            assert!(!is_sandbox_unavailable(&failure), "{form}");
            s.close().await;
        }
    }

    #[tokio::test]
    async fn keeps_an_invalid_workdir_ordinary_when_danger_full_access_bypasses_the_provider() {
        let s = setup(Options {
            mode: Some("danger-full-access"),
            ..Options::default()
        })
        .await;
        let parent = tempfile::tempdir().unwrap();
        let failure = run(
            &s,
            s.resolve(ShellExecRequest {
                workdir: Some(canonical(parent.path().join("missing"))),
                ..request("true")
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(error_code(&failure).as_deref(), Some("ENOENT"), "{failure}");
        assert!(!is_sandbox_unavailable(&failure));
        s.close().await;
    }

    #[test]
    fn structured_errno_reader_accepts_the_spawn_helpers_coded_form() {
        let helper = SubprocessForeignError::Coded {
            message: "spawn /usr/bin/env ENOENT".into(),
            code: json!("ENOENT"),
        };
        let error: BoxError = Box::new(SharedError::new(Box::new(helper) as BoxError));
        assert_eq!(error_code(&error).as_deref(), Some("ENOENT"));
    }

    #[tokio::test]
    async fn keeps_node_shaped_synchronous_enoexec_ordinary_in_run_and_start() {
        let runner = canonical(std::env::temp_dir().join("malformed-runner"));
        let s = setup(Options {
            behavior: {
                let runner = runner.clone();
                immediate(move |argv, _| {
                    let mut wrapped = vec![runner.clone()];
                    wrapped.extend(argv.iter().cloned());
                    Ok(wrap(wrapped))
                })
            },
            ..Options::default()
        })
        .await;
        s.subprocess.set(SpawnMode::Throw(Arc::new(|| {
            spawn_failure(libc_enoexec(), "")
        })));

        let foreground = run(&s, s.resolve(request("true"))).await.unwrap_err();
        assert_eq!(error_code(&foreground).as_deref(), Some("ENOEXEC"));
        assert!(!is_sandbox_unavailable(&foreground));

        // Containment: the handle settles killed instead of the spawn throwing, and the projection
        // carries the original error, never the fail-closed one.
        let background = start(&s, s.resolve(request("true"))).await.unwrap();
        background.done().await;
        assert_eq!(background.status(), ShellProcessStatus::Killed);
        let error = background.result().await.unwrap_err();
        assert_eq!(error_code(&error).as_deref(), Some("ENOEXEC"));
        assert!(!is_sandbox_unavailable(&error));
        assert!(
            background
                .read_output()
                .delta
                .contains("subprocess failed before reporting an outcome:")
        );
        s.close().await;
    }

    fn libc_enoexec() -> i32 {
        8
    }

    #[tokio::test]
    async fn classifies_a_synchronous_subprocess_runtime_eacces_with_the_exact_runner_path() {
        let runner = canonical(std::env::temp_dir().join("unexecutable-runner"));
        let s = setup(Options {
            behavior: {
                let runner = runner.clone();
                immediate(move |argv, _| {
                    let mut wrapped = vec![runner.clone()];
                    wrapped.extend(argv.iter().cloned());
                    Ok(wrap(wrapped))
                })
            },
            ..Options::default()
        })
        .await;
        // This pins an alternative SubprocessRuntime's synchronous seam, not the shipped local
        // behavior.
        let thrown = runner.clone();
        s.subprocess.set(SpawnMode::Throw(Arc::new(move || {
            spawn_failure(13, &thrown)
        })));

        let error = run(&s, s.resolve(request("true"))).await.unwrap_err();
        assert!(is_sandbox_unavailable(&error), "{error}");
        // The background view of the same failure: a killed handle whose facts attribute the
        // launch failure to the runner.
        let background = start(&s, s.resolve(request("true"))).await.unwrap();
        background.done().await;
        assert_eq!(background.status(), ShellProcessStatus::Killed);
        assert_eq!(
            background
                .sandbox()
                .and_then(|sandbox| sandbox.runner_failed),
            Some(true)
        );
        s.close().await;
    }

    #[tokio::test]
    async fn keeps_a_synchronous_cwd_owned_enoent_as_the_original_start_error() {
        let runner = "./sandbox-runner";
        let s = setup(Options {
            behavior: immediate(move |argv, _| {
                let mut wrapped = vec![runner.to_owned()];
                wrapped.extend(argv.iter().cloned());
                Ok(wrap(wrapped))
            }),
            ..Options::default()
        })
        .await;
        let parent = tempfile::tempdir().unwrap();
        let workdir = canonical(parent.path().join("missing"));
        // The Node error of the source: `{code:'ENOENT', syscall:'spawn ./sandbox-runner', path}`.
        s.subprocess.set(SpawnMode::Throw(Arc::new(|| {
            spawn_failure(2, "./sandbox-runner")
        })));
        let background = start(
            &s,
            s.resolve(ShellExecRequest {
                workdir: Some(workdir),
                ..request("true")
            }),
        )
        .await
        .unwrap();
        background.done().await;
        assert_eq!(background.status(), ShellProcessStatus::Killed);
        // A cwd-owned failure keeps the original error: not the runner's fault.
        let thrown = background.result().await.unwrap_err();
        let shared = thrown.downcast_ref::<SharedError>().expect("shared error");
        assert!(
            shared
                .get_ref()
                .downcast_ref::<SubprocessSpawnRejection>()
                .is_some(),
            "{thrown}"
        );
        assert!(!is_sandbox_unavailable(&thrown));
        assert_eq!(
            background
                .sandbox()
                .and_then(|sandbox| sandbox.runner_failed),
            None
        );

        // The same rejection with a usable cwd IS the runner's: only the cwd check kept it ordinary.
        let usable = start(&s, s.resolve(request("true"))).await.unwrap();
        usable.done().await;
        assert_eq!(
            usable.sandbox().and_then(|sandbox| sandbox.runner_failed),
            Some(true)
        );
        s.close().await;
    }

    /// A subprocess helper reports a spawn failure as `Coded` (the code and the Node-shaped
    /// message only: no separate `syscall`/`path`). `message` is the helper's text.
    async fn helper_reports(
        message: impl Fn(&str) -> String + Send + Sync + 'static,
        workdir_missing: bool,
    ) -> (Result<ShellRunResult, BoxError>, Option<ShellSandboxInfo>) {
        let runner = canonical(std::env::temp_dir().join("dsh-missing-helper-runner"));
        let s = setup(Options {
            behavior: {
                let runner = runner.clone();
                immediate(move |argv, _| {
                    let mut wrapped = vec![runner.clone()];
                    wrapped.extend(argv.iter().cloned());
                    Ok(wrap(wrapped))
                })
            },
            ..Options::default()
        })
        .await;
        let text = message(&runner);
        s.subprocess.set(SpawnMode::Handle(Arc::new(move |_| {
            let error = SubprocessForeignError::Coded {
                message: text.clone(),
                code: json!("ENOENT"),
            };
            failed_handle(SharedError::new(error))
        })));
        let parent = tempfile::tempdir().unwrap();
        let workdir = workdir_missing.then(|| canonical(parent.path().join("missing")));
        let spec = || {
            s.resolve(ShellExecRequest {
                workdir: workdir.clone(),
                ..request("true")
            })
        };
        let foreground = run(&s, spec()).await;
        let background = start(&s, spec()).await.unwrap();
        background.done().await;
        let facts = background.sandbox();
        s.close().await;
        (foreground, facts)
    }

    #[tokio::test]
    async fn attributes_a_helper_reported_runner_enoent_to_the_runner() {
        let (foreground, facts) =
            helper_reports(|runner| format!("spawn {runner} ENOENT"), false).await;
        let error = foreground.unwrap_err();
        assert!(is_sandbox_unavailable(&error), "{error}");
        assert!(
            error
                .to_string()
                .contains("dsh-missing-helper-runner ENOENT"),
            "{error}"
        );
        assert_eq!(facts.and_then(|facts| facts.runner_failed), Some(true));
    }

    #[tokio::test]
    async fn keeps_a_helper_reported_enoent_ordinary_when_the_workdir_is_missing() {
        let (foreground, facts) =
            helper_reports(|runner| format!("spawn {runner} ENOENT"), true).await;
        let error = foreground.unwrap_err();
        assert!(!is_sandbox_unavailable(&error), "{error}");
        assert_eq!(facts.and_then(|facts| facts.runner_failed), None);
    }

    #[tokio::test]
    async fn keeps_a_helper_reported_execve_enoent_ordinary() {
        let (foreground, facts) = helper_reports(
            |runner| format!("ENOENT: no such file or directory, execve '{runner}'"),
            false,
        )
        .await;
        let error = foreground.unwrap_err();
        assert!(!is_sandbox_unavailable(&error), "{error}");
        assert_eq!(facts.and_then(|facts| facts.runner_failed), None);
    }
}

mod danger_full_access {
    use super::*;

    #[tokio::test]
    async fn runs_unwrapped_the_provider_is_never_consulted_facts_carry_no_enforcement() {
        let s = setup(Options {
            mode: Some("danger-full-access"),
            ..Options::default()
        })
        .await;
        let result = run(&s, s.resolve(request("echo free"))).await.unwrap();
        assert_eq!(result.stdout.text, "free\n");
        assert_eq!(
            result.sandbox,
            Some(info(SandboxMode::DangerFullAccess, false, None))
        );
        assert!(s.sandbox.calls().is_empty());
        s.close().await;
    }

    #[tokio::test]
    async fn start_passes_through_unwrapped_and_stamps_nothing_at_settle() {
        let s = setup(Options {
            mode: Some("danger-full-access"),
            ..Options::default()
        })
        .await;
        let task = start(&s, s.resolve(request("echo free-bg"))).await.unwrap();
        task.done().await;
        assert_eq!(task.sandbox(), None);
        assert!(task.read_output().delta.contains("free-bg"));
        assert!(s.sandbox.calls().is_empty());
        s.close().await;
    }
}

mod per_call_sandbox_policy_the_session_and_escalation_carrier {
    use super::*;

    #[tokio::test]
    async fn exposes_the_configured_default_as_the_capability_fact_and_resolve_stamps_it() {
        let s = setup(Options::default()).await;
        assert_eq!(s.shell.sandbox_mode(), Some(SandboxMode::ReadOnly));
        assert_eq!(
            s.resolve(request("true"))
                .sandbox_policy
                .as_ref()
                .map(exec_policy_json),
            Some(json!({"mode": "read-only", "workspaceRoot": cwd()}))
        );
        s.close().await;
    }

    #[tokio::test]
    async fn an_explicit_policy_outranks_the_default_at_resolve_and_the_wrap_follows_its_mode_and_root()
     {
        let s = setup(Options::default()).await;
        let explicit = execution_policy(SandboxMode::WorkspaceWrite, "/session/project");
        let stamped = s
            .resolve(with_policy("true", explicit.clone()))
            .sandbox_policy
            .unwrap();
        assert_eq!(exec_policy_json(&stamped), exec_policy_json(&explicit));
        run(&s, s.resolve(with_policy("true", explicit)))
            .await
            .unwrap();
        run(&s, s.resolve(request("true"))).await.unwrap();
        let calls = s.sandbox.calls();
        assert_eq!(
            calls
                .iter()
                .map(|call| policy_json(&call.policy))
                .collect::<Vec<_>>(),
            [
                json!({"mode": "workspace-write", "workspaceRoot": "/session/project"}),
                json!({"mode": "read-only", "workspaceRoot": cwd()})
            ]
        );
        s.close().await;
    }

    #[tokio::test]
    async fn an_escalated_run_reports_the_mode_it_actually_ran_under() {
        let s = setup(Options::default()).await;
        let result = run(
            &s,
            s.resolve(with_policy(
                "true",
                execution_policy(SandboxMode::WorkspaceWrite, &cwd()),
            )),
        )
        .await
        .unwrap();
        assert_eq!(
            result.sandbox,
            Some(full(SandboxMode::WorkspaceWrite, false))
        );
        s.close().await;
    }

    #[tokio::test]
    async fn escalating_to_danger_full_access_bypasses_the_provider_entirely_the_grant_not_a_probe_is_the_authority()
     {
        let s = setup(Options::default()).await;
        let result = run(
            &s,
            s.resolve(with_policy(
                "echo free",
                execution_policy(SandboxMode::DangerFullAccess, &cwd()),
            )),
        )
        .await
        .unwrap();
        assert_eq!(result.stdout.text, "free\n");
        assert_eq!(
            result.sandbox,
            Some(info(SandboxMode::DangerFullAccess, false, None))
        );
        assert!(s.sandbox.calls().is_empty());
        s.close().await;
    }

    #[tokio::test]
    async fn overlapping_background_jobs_settle_with_their_own_modes_an_escalated_task_next_to_a_default_one()
     {
        // With per-call policy, tasks under different modes are in flight at once — anything keyed
        // off the configured default would misreport the escalated one at its settle stamp.
        let s = setup(Options::default()).await;
        let escalated = start(
            &s,
            s.resolve(with_policy(
                "sleep 0.3; echo \"x: Permission denied\" >&2; exit 1",
                execution_policy(SandboxMode::WorkspaceWrite, &cwd()),
            )),
        )
        .await
        .unwrap();
        let plain = start(&s, s.resolve(request("true"))).await.unwrap();
        plain.done().await;
        escalated.done().await;
        assert_eq!(
            escalated.sandbox(),
            Some(full(SandboxMode::WorkspaceWrite, true))
        );
        assert_eq!(plain.sandbox(), Some(full(SandboxMode::ReadOnly, false)));
        s.close().await;
    }

    #[tokio::test]
    async fn an_escalated_danger_full_access_background_job_carries_no_facts_nothing_confined_it() {
        let s = setup(Options::default()).await;
        let task = start(
            &s,
            s.resolve(with_policy(
                "echo bg-free",
                execution_policy(SandboxMode::DangerFullAccess, &cwd()),
            )),
        )
        .await
        .unwrap();
        task.done().await;
        assert_eq!(task.sandbox(), None);
        assert!(task.read_output().delta.contains("bg-free"));
        assert!(s.sandbox.calls().is_empty());
        s.close().await;
    }
}

mod classify_denial {
    use super::*;
    use hoh_bash_sandbox::helpers::classify_denial;

    fn run_result(exit_code: Option<i64>, stderr: &str) -> ShellRunResult {
        ShellRunResult {
            exit_code,
            signal: None,
            timed_out: false,
            aborted: false,
            timeout_ms: 1000.0,
            stdout: CollectedOutput {
                text: String::new(),
                truncated: false,
                spill_path: None,
            },
            stderr: CollectedOutput {
                text: stderr.to_owned(),
                truncated: false,
                spill_path: None,
            },
            sandbox: None,
        }
    }

    #[test]
    fn never_classifies_a_clean_exit_or_a_signal_kill_as_a_denial() {
        assert!(!classify_denial(
            &run_result(Some(0), "Permission denied"),
            &unix_signatures()
        ));
        assert!(!classify_denial(
            &run_result(None, "Permission denied"),
            &unix_signatures()
        ));
    }

    #[test]
    fn classifies_failed_runs_by_the_wraps_own_dialect_conservatively() {
        let signatures = unix_signatures();
        assert!(classify_denial(
            &run_result(Some(1), "touch: cannot touch /x: Read-only file system"),
            &signatures
        ));
        assert!(classify_denial(
            &run_result(Some(1), "sh: /x: Permission denied"),
            &signatures
        ));
        // Bare EPERM is not a Linux runner's dialect: mount/kill/ptrace fail with it unsandboxed
        // too, and the mode vocabulary governs file effects only — claiming a file denial here
        // would tell the model the sandbox blocked something it never governed.
        assert!(!classify_denial(
            &run_result(Some(1), "mount: Operation not permitted"),
            &signatures
        ));
        assert!(!classify_denial(
            &run_result(Some(1), "No such file or directory"),
            &signatures
        ));
    }

    #[test]
    fn matches_exactly_the_active_backends_dialect_eperm_classifies_under_seatbelt_eacces_does_not_under_bwrap()
     {
        // The same stderr flips meaning with the backend: under Seatbelt, EPERM text IS how the
        // kernel refuses a governed file write; under bwrap's EROFS-only dialect, `Permission
        // denied` is ordinary DAC, not the sandbox — per-wrap signatures are what keep both
        // classifications honest.
        assert!(classify_denial(
            &run_result(Some(1), "bash: /etc/x: Operation not permitted"),
            &strings(&["operation not permitted"])
        ));
        assert!(!classify_denial(
            &run_result(Some(1), "sh: /x: Permission denied"),
            &strings(&["read-only file system"])
        ));
    }
}

mod result_facts {
    use super::*;

    async fn exit_code_stays_ordinary(exit_code: i64) {
        let s = setup(Options {
            behavior: immediate(|argv, _| {
                let mut wrapped = strings(&["env"]);
                wrapped.extend(argv.iter().cloned());
                Ok(wrap(wrapped))
            }),
            ..Options::default()
        })
        .await;
        let result = run(&s, s.resolve(request(&format!("exit {exit_code}"))))
            .await
            .unwrap();
        assert_eq!(result.exit_code, Some(exit_code));
        assert_eq!(result.sandbox, Some(full(SandboxMode::ReadOnly, false)));
        s.close().await;
    }

    #[tokio::test]
    async fn keeps_a_successfully_launched_wrapped_child_exit_126_as_an_ordinary_outcome() {
        exit_code_stays_ordinary(126).await;
    }

    #[tokio::test]
    async fn keeps_a_successfully_launched_wrapped_child_exit_127_as_an_ordinary_outcome() {
        exit_code_stays_ordinary(127).await;
    }

    #[tokio::test]
    async fn reports_a_real_permission_failure_as_a_sandbox_denial_with_the_mode_it_ran_under() {
        let s = setup(Options::default()).await;
        let denied_root = tempfile::tempdir().unwrap();
        let locked = denied_root.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        let result = run(
            &s,
            s.resolve(request(&format!("echo x > {}/f", canonical(&locked)))),
        )
        .await
        .unwrap();
        assert_ne!(result.exit_code, Some(0));
        assert_eq!(result.sandbox, Some(full(SandboxMode::ReadOnly, true)));
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        s.close().await;
    }

    #[tokio::test]
    async fn carries_the_providers_partial_enforcement_fact_through_unchanged() {
        let s = setup(Options {
            behavior: immediate(|argv, _| {
                Ok(ConfinedArgv {
                    enforcement: SandboxEnforcement::Partial,
                    ..passthrough(argv)
                })
            }),
            ..Options::default()
        })
        .await;
        let result = run(&s, s.resolve(request("true"))).await.unwrap();
        assert_eq!(
            result.sandbox,
            Some(info(
                SandboxMode::ReadOnly,
                false,
                Some(SandboxEnforcement::Partial)
            ))
        );
        s.close().await;
    }
}

mod background_sandbox_facts {
    use super::*;

    #[tokio::test]
    async fn keeps_an_invalid_workdir_rejection_ordinary_for_every_provider_runner_form() {
        for (form, runner) in [
            ("absolute", "/usr/bin/env"),
            ("bare", "env"),
            ("relative", "./sandbox-runner"),
        ] {
            let s = setup(Options {
                behavior: immediate(move |argv, _| {
                    let mut wrapped = vec![runner.to_owned()];
                    wrapped.extend(argv.iter().cloned());
                    Ok(wrap(wrapped))
                }),
                ..Options::default()
            })
            .await;
            let parent = tempfile::tempdir().unwrap();
            let task = start(
                &s,
                s.resolve(ShellExecRequest {
                    workdir: Some(canonical(parent.path().join("missing"))),
                    ..request("true")
                }),
            )
            .await
            .unwrap();
            task.done().await;
            assert_eq!(task.status(), ShellProcessStatus::Killed, "{form}");
            assert!(
                task.read_output()
                    .delta
                    .contains("subprocess failed before reporting an outcome:"),
                "{form}"
            );
            assert_eq!(
                task.sandbox(),
                Some(full(SandboxMode::ReadOnly, false)),
                "{form}"
            );
            s.close().await;
        }
    }

    #[tokio::test]
    async fn does_not_invent_runner_evidence_when_a_provider_rejection_has_no_structured_reason() {
        let s = setup(Options::default()).await;
        // Arbitrary subprocess providers can reject without a code or public stage. (The source
        // rejects with `undefined`; a rejection here is always an error value.)
        let (handle, settle_done) = pending_handle();
        let handle = Mutex::new(Some(handle));
        s.subprocess.set(SpawnMode::Handle(Arc::new(move |_| {
            lock(&handle).take().expect("one spawn")
        })));
        let task = start(&s, s.resolve(request("true"))).await.unwrap();
        let _ = settle_done.send(Err(SharedError::new(Marked)));
        task.done().await;
        assert!(task.read_output().delta.contains(
            "subprocess failed before reporting an outcome: Error: no structured reason"
        ));
        assert_eq!(task.sandbox(), Some(full(SandboxMode::ReadOnly, false)));
        s.close().await;
    }

    #[derive(Debug)]
    struct Marked;
    impl std::fmt::Display for Marked {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("no structured reason")
        }
    }
    impl std::error::Error for Marked {}

    #[tokio::test]
    async fn stamps_a_settled_denial_nonzero_exit_plus_permission_stderr_under_a_confined_mode() {
        let s = setup(Options::default()).await;
        let task = start(
            &s,
            s.resolve(request("echo \"x: Permission denied\" >&2; exit 1")),
        )
        .await
        .unwrap();
        task.done().await;
        assert_eq!(task.sandbox(), Some(full(SandboxMode::ReadOnly, true)));
        s.close().await;
    }

    #[tokio::test]
    async fn a_foreground_runner_failure_throws_the_fail_closed_error_never_a_task_result() {
        // The wrap's runner prefix on a failed run means the SANDBOX broke and the command never
        // ran — the late twin of the confine-time throw, with the matched fatal stderr line
        // carried as the cause.
        let s = setup(Options::default()).await;
        let spec = s.resolve(request(
            "echo \"fake-runner: ruleset rejected\" >&2; exit 125",
        ));
        let execution = s.shell.execute(spec).await.unwrap();
        let first = execution.result().await.unwrap_err();
        assert!(is_sandbox_unavailable(&first), "{first}");
        let second = execution.result().await.unwrap_err();
        assert!(
            second.to_string().contains("fake-runner: ruleset rejected"),
            "{second}"
        );
        s.close().await;
    }

    #[tokio::test]
    async fn a_foreground_runner_failure_outranks_denial_runner_error_text_may_contain_denial_words()
     {
        let s = setup(Options::default()).await;
        let error = run(
            &s,
            s.resolve(request(
                "echo \"fake-runner: cannot open rule path: /x: Permission denied\" >&2; exit 125",
            )),
        )
        .await
        .unwrap_err();
        assert!(is_sandbox_unavailable(&error), "{error}");
        s.close().await;
    }

    #[tokio::test]
    async fn a_settled_background_runner_failure_stamps_runner_failed_no_error_channel_remains_not_denied()
     {
        let s = setup(Options::default()).await;
        let task = start(
            &s,
            s.resolve(request(
                "echo \"fake-runner: cannot open rule path: /x: Permission denied\" >&2; exit 125",
            )),
        )
        .await
        .unwrap();
        task.done().await;
        assert_eq!(
            task.sandbox(),
            Some(ShellSandboxInfo {
                mode: SandboxMode::ReadOnly,
                denied: false,
                enforcement: Some(SandboxEnforcement::Full),
                runner_failed: Some(true),
            })
        );
        s.close().await;
    }

    #[tokio::test]
    async fn overlapping_background_jobs_keep_their_own_wrap_facts_per_task_not_latest_wrap() {
        // Facts belong to each wrap and may vary between calls. The slow task settles after the
        // quick task starts; a shared latest-wrap field would classify and stamp it with the wrong
        // task's dialect and enforcement.
        let wraps = [
            (SandboxEnforcement::Partial, strings(&["permission denied"])),
            (
                SandboxEnforcement::Full,
                strings(&["read-only file system"]),
            ),
        ];
        let call = Arc::new(Mutex::new(0usize));
        let s = setup(Options {
            behavior: immediate(move |argv, _| {
                let mut call = lock(&call);
                let (enforcement, signatures) = wraps[(*call).min(wraps.len() - 1)].clone();
                *call += 1;
                Ok(ConfinedArgv {
                    argv: argv.to_vec(),
                    enforcement,
                    denial_signatures: signatures,
                    runner_failure_rules: runner_failure(),
                })
            }),
            ..Options::default()
        })
        .await;
        let slow = start(
            &s,
            s.resolve(request(
                "sleep 0.4; echo \"x: Permission denied\" >&2; exit 1",
            )),
        )
        .await
        .unwrap();
        let quick = start(&s, s.resolve(request("true"))).await.unwrap();
        quick.done().await;
        slow.done().await;
        assert_eq!(
            slow.sandbox(),
            Some(info(
                SandboxMode::ReadOnly,
                true,
                Some(SandboxEnforcement::Partial)
            ))
        );
        assert_eq!(quick.sandbox(), Some(full(SandboxMode::ReadOnly, false)));
        s.close().await;
    }

    #[tokio::test]
    async fn a_signal_killed_task_is_never_a_denial_null_exit_code() {
        let s = setup(Options::default()).await;
        let task = start(
            &s,
            s.resolve(request("echo \"Permission denied\" >&2; sleep 30")),
        )
        .await
        .unwrap();
        // Let the stderr land before the kill so the classifier sees the signature and must still
        // refuse it on the null exit code alone.
        let mut seen = String::new();
        for _ in 0..500 {
            seen.push_str(&task.read_output().delta);
            if seen.contains("Permission denied") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(seen.contains("Permission denied"));
        task.kill();
        task.done().await;
        assert_eq!(task.sandbox(), Some(full(SandboxMode::ReadOnly, false)));
        s.close().await;
    }

    #[tokio::test]
    async fn disposal_kills_wrapped_background_jobs_inherited_hmr_safety() {
        let s = setup(Options::default()).await;
        let task = start(&s, s.resolve(request("sleep 30"))).await.unwrap();
        s.close().await;
        assert_eq!(task.status(), ShellProcessStatus::Killed);
    }
}

#[allow(dead_code)]
fn unused(_: &SandboxPolicy) {}
