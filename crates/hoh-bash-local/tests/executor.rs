mod common;

use common::*;
use futures_util::FutureExt;
use hoh_error::{BoxError, SharedError};
use hoh_shell::ShellProcessStatus;
use hoh_subprocess::*;
use hoh_timeout::{AbortController, AbortSignal, MAX_TIMER_DELAY_MS};
use indexmap::IndexMap;
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[tokio::test]
async fn resolves_with_output_and_the_effective_timeout() {
    let setup = setup(json!({"timeoutMs": 5_000})).await;
    let spec = setup.shell.resolve(&request("echo hi"));
    let result = run(&setup.shell, spec).await.unwrap();
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.stdout.text, "hi\n");
    assert_eq!(result.timeout_ms, 5_000.0);
}

#[tokio::test]
async fn uses_config_cwd_overridable_per_call() {
    let setup = setup(json!({"cwd": "/tmp"})).await;
    let spec = setup.shell.resolve(&request("pwd"));
    let from_config = run(&setup.shell, spec).await.unwrap();
    assert!(from_config.stdout.text.trim().ends_with("/tmp"));
    let mut request = request("pwd");
    request.workdir = Some("/".into());
    let spec = setup.shell.resolve(&request);
    let from_call = run(&setup.shell, spec).await.unwrap();
    assert_eq!(from_call.stdout.text.trim(), "/");
}

#[tokio::test]
async fn defaults_cwd_to_process_cwd() {
    let setup = setup(json!({})).await;
    let spec = setup.shell.resolve(&request("pwd"));
    let result = run(&setup.shell, spec).await.unwrap();
    assert_eq!(
        result.stdout.text.trim(),
        std::env::current_dir().unwrap().to_string_lossy()
    );
}

#[tokio::test]
async fn caps_per_call_timeouts_at_max_timeout_ms() {
    let setup = setup(json!({"timeoutMs": 1_000, "maxTimeoutMs": 2_000})).await;
    let mut request = request("true");
    request.timeout_ms = Some(99_999.0);
    let spec = setup.shell.resolve(&request);
    let result = run(&setup.shell, spec).await.unwrap();
    assert_eq!(result.timeout_ms, 2_000.0);
}

#[tokio::test]
async fn rejects_unusable_numeric_config_at_the_next_command_and_invalid_timeout_overrides_at_resolve()
 {
    // `timeoutMs: NaN` of the source is not representable in a JSON profile; it is covered by
    // `direct_config_validation_preserves_the_nan_case` in settings.rs.
    let grace = format!("graceMs must be no greater than {MAX_TIMER_DELAY_MS}");
    let cases = [
        (json!({"maxTimeoutMs": 0}), "maxTimeoutMs".to_owned()),
        (json!({"maxOutputBytes": -1}), "maxOutputBytes".to_owned()),
        (json!({"maxSpillBytes": 0}), "maxSpillBytes".to_owned()),
        (json!({"graceMs": 0}), "graceMs".to_owned()),
        (json!({"graceMs": MAX_TIMER_DELAY_MS + 1}), grace),
    ];
    for (config, field) in cases {
        let setup = setup(config).await;
        let error = resolve_error(&setup.shell, request("true"));
        assert!(error.contains(&field), "{error}");
    }

    let setup = setup(json!({})).await;
    for timeout in [f64::NAN, -1.0] {
        let mut request = request("true");
        request.timeout_ms = Some(timeout);
        assert!(resolve_error(&setup.shell, request).contains("request.timeoutMs"));
    }
    // `stdoutMaxBytes: NaN` and `-1` are not representable in the u64 seam type; zero is the
    // remaining non-positive value.
    let mut request = request("true");
    request.stdout_max_bytes = Some(0);
    assert!(resolve_error(&setup.shell, request).contains("request.stdoutMaxBytes"));
}

#[tokio::test]
async fn defaults_stdout_cap_and_lets_foreground_callers_raise_stdout_only() {
    let setup = setup(json!({"maxOutputBytes": 100})).await;
    assert_eq!(setup.shell.resolve(&request("true")).stdout_max_bytes, 100);
    let mut request = request("printf \"%.0sx\" $(seq 1 500); printf \"%.0se\" $(seq 1 500) >&2");
    request.stdout_max_bytes = Some(500);
    let spec = setup.shell.resolve(&request);
    let result = run(&setup.shell, spec).await.unwrap();
    assert!(!result.stdout.truncated);
    assert_eq!(result.stdout.text, "x".repeat(500));
    assert!(result.stderr.truncated);
    assert!(result.stderr.text.len() <= 100);
}

#[tokio::test]
async fn per_call_timeout_takes_precedence_under_cap_and_kills_on_expiry() {
    let setup = setup(json!({"timeoutMs": 60_000})).await;
    let mut request = request("sleep 60");
    request.timeout_ms = Some(100.0);
    let spec = setup.shell.resolve(&request);
    let result = run(&setup.shell, spec).await.unwrap();
    assert!(result.timed_out);
    assert!(!result.aborted);
    assert_eq!(result.timeout_ms, 100.0);
}

#[tokio::test]
async fn propagates_abort_signals() {
    let setup = setup(json!({})).await;
    let controller = AbortController::new();
    let mut request = request("sleep 60");
    request.signal = Some(controller.signal());
    let spec = setup.shell.resolve(&request);
    let pending = run(&setup.shell, spec);
    tokio::pin!(pending);
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_millis(50)) => controller.abort(),
        result = &mut pending => panic!("finished before abort: {result:?}"),
    }
    let result = pending.await.unwrap();
    assert!(result.aborted);
    assert!(!result.timed_out);
}

#[tokio::test]
async fn classifies_self_killed_command_as_neither_timed_out_nor_aborted() {
    let setup = setup(json!({"timeoutMs": 60_000})).await;
    let spec = setup.shell.resolve(&request("kill -TERM $$"));
    let result = run(&setup.shell, spec).await.unwrap();
    assert_eq!(result.signal.as_deref(), Some("SIGTERM"));
    assert!(!result.timed_out);
    assert!(!result.aborted);
}

#[tokio::test]
async fn rejects_on_spawn_failure_bad_workdir() {
    let setup = setup(json!({})).await;
    let mut request = request("true");
    request.workdir = Some("/nonexistent-dsh".into());
    let spec = setup.shell.resolve(&request);
    let error = run(&setup.shell, spec).await.unwrap_err();
    assert!(error.to_string().contains("ENOENT"), "{error}");
}

#[tokio::test]
async fn resolve_carries_stdin_env_and_dsh_env_and_run_threads_them() {
    let setup = setup(json!({})).await;
    let mut request = request("cat; echo \"[$SEAM_VAR][$DSH_SEAM_VAR]\"");
    request.stdin = Some("piped\n".into());
    request.env = Some(IndexMap::from([("SEAM_VAR".into(), "env-ok".into())]));
    request.dsh_env = Some(IndexMap::from([("DSH_SEAM_VAR".into(), "dsh-ok".into())]));
    let spec = setup.shell.resolve(&request);
    assert_eq!(spec.stdin.as_deref(), Some("piped\n"));
    assert_eq!(spec.env.clone().unwrap()["SEAM_VAR"], "env-ok");
    assert_eq!(spec.dsh_env.clone().unwrap()["DSH_SEAM_VAR"], "dsh-ok");
    let result = run(&setup.shell, spec).await.unwrap();
    assert_eq!(result.stdout.text, "piped\n[env-ok][dsh-ok]\n");
}

#[tokio::test]
async fn resolve_omits_stdin_env_and_dsh_env_when_absent() {
    let setup = setup(json!({})).await;
    let spec = setup.shell.resolve(&request("true"));
    assert!(spec.stdin.is_none());
    assert!(spec.env.is_none());
    assert!(spec.dsh_env.is_none());
}

#[tokio::test]
async fn start_returns_running_handle_before_child_finishes() {
    let setup = setup(json!({})).await;
    let directory = tempfile::tempdir().unwrap();
    let mut request = request("echo ready; while [ ! -f release ]; do sleep 0.02; done; echo done");
    request.workdir = Some(directory.path().to_string_lossy().into_owned());
    let spec = setup.shell.resolve(&request);
    let process = start(&setup.shell, spec).await;
    read_until(&*process, "ready\n").await;
    assert_eq!(process.status(), ShellProcessStatus::Running);
    std::fs::write(directory.path().join("release"), "").unwrap();
    process.done().await;
    assert_eq!(process.status(), ShellProcessStatus::Completed);
    assert_eq!(process.exit_code(), Some(0));
    assert_eq!(process.read_output().delta, "done\n");
}

#[tokio::test]
async fn threads_stdin_and_extra_env_into_background_process() {
    let setup = setup(json!({})).await;
    let mut request = request("cat; echo \"[$BG_VAR][$DSH_BG_VAR]\"");
    request.stdin = Some("bg-stdin\n".into());
    request.env = Some(IndexMap::from([("BG_VAR".into(), "bg-env".into())]));
    request.dsh_env = Some(IndexMap::from([("DSH_BG_VAR".into(), "bg-dsh-env".into())]));
    let spec = setup.shell.resolve(&request);
    let process = start(&setup.shell, spec).await;
    let output = read_until(&*process, "[bg-env][bg-dsh-env]").await;
    assert!(output.contains("bg-stdin"));
    process.done().await;
    assert_eq!(process.exit_code(), Some(0));
}

#[tokio::test]
async fn read_output_is_consuming_and_stays_valid_after_exit() {
    let setup = setup(json!({})).await;
    let spec = setup
        .shell
        .resolve(&request("echo first; sleep 1; echo second"));
    let process = start(&setup.shell, spec).await;
    assert_eq!(read_until(&*process, "first\n").await, "first\n");
    process.done().await;
    let second = process.read_output();
    assert_eq!(second.delta, "second\n");
    assert!(!second.lossy);
    assert_eq!(process.read_output().delta, "");
}

#[tokio::test]
async fn read_output_marks_stderr_sections() {
    let setup = setup(json!({})).await;
    let spec = setup.shell.resolve(&request("echo out; echo err >&2"));
    let process = start(&setup.shell, spec).await;
    process.done().await;
    assert_eq!(process.read_output().delta, "out\n[stderr]\nerr\n");
}

#[tokio::test]
async fn read_output_reports_stderr_only_without_leading_newline() {
    let setup = setup(json!({})).await;
    let spec = setup.shell.resolve(&request("echo err >&2"));
    let process = start(&setup.shell, spec).await;
    process.done().await;
    assert_eq!(process.read_output().delta, "[stderr]\nerr\n");
}

#[tokio::test]
async fn read_output_adds_separator_only_when_stdout_lacks_newline() {
    let setup = setup(json!({})).await;
    let spec = setup.shell.resolve(&request("printf out; echo err >&2"));
    let process = start(&setup.shell, spec).await;
    process.done().await;
    assert_eq!(process.read_output().delta, "out\n[stderr]\nerr\n");
}

#[tokio::test]
async fn read_output_flags_lossy_reads_and_reports_stdout_spill() {
    let setup = setup(json!({"maxOutputBytes": 100})).await;
    let spec = setup.shell.resolve(&request(
        "for i in $(seq 1 100); do printf \"line-%04d\\n\" $i; done",
    ));
    let process = start(&setup.shell, spec).await;
    process.done().await;
    let read = process.read_output();
    assert!(read.lossy);
    assert!(read.stdout_spill_path.is_some());
}

#[tokio::test]
async fn read_output_reports_stderr_spill_paths() {
    let setup = setup(json!({"maxOutputBytes": 100})).await;
    let spec = setup.shell.resolve(&request(
        "for i in $(seq 1 100); do printf \"line-%04d\\n\" $i >&2; done",
    ));
    let process = start(&setup.shell, spec).await;
    process.done().await;
    let read = process.read_output();
    assert!(read.lossy);
    assert!(read.stderr_spill_path.is_some());
    assert!(read.delta.contains("[stderr]"));
}

#[tokio::test]
async fn kill_requests_managed_range_termination_true_once_false_after_settlement() {
    let setup = setup(json!({})).await;
    let spec = setup.shell.resolve(&request("sleep 60"));
    let process = start(&setup.shell, spec).await;
    assert!(process.kill());
    process.done().await;
    assert_eq!(process.status(), ShellProcessStatus::Killed);
    assert_eq!(process.signal().as_deref(), Some("SIGTERM"));
    assert!(!process.kill());
}

#[tokio::test]
async fn kill_returns_false_for_naturally_completed_process() {
    let setup = setup(json!({})).await;
    let spec = setup.shell.resolve(&request("true"));
    let process = start(&setup.shell, spec).await;
    process.done().await;
    assert_eq!(process.status(), ShellProcessStatus::Completed);
    assert!(!process.kill());
}

#[tokio::test]
async fn kill_escalation_uses_configured_grace_ms() {
    let setup = setup(json!({})).await;
    let spec = setup
        .shell
        .resolve(&request("trap '' TERM; echo armed; sleep 60"));
    let process = start(&setup.shell, spec).await;
    read_until(&*process, "armed").await;
    process.kill();
    process.done().await;
    assert_eq!(process.status(), ShellProcessStatus::Killed);
    assert_eq!(process.signal().as_deref(), Some("SIGKILL"));
}

#[tokio::test]
async fn spec_signal_abort_settles_handle_as_killed_not_completed() {
    let setup = setup(json!({})).await;
    let controller = AbortController::new();
    let mut request = request("sleep 60");
    request.signal = Some(controller.signal());
    let spec = setup.shell.resolve(&request);
    let process = start(&setup.shell, spec).await;
    controller.abort();
    process.done().await;
    assert_eq!(process.status(), ShellProcessStatus::Killed);
    assert_eq!(process.signal().as_deref(), Some("SIGTERM"));
}

#[tokio::test]
async fn self_signal_exit_settles_handle_as_killed_not_completed() {
    let setup = setup(json!({})).await;
    let spec = setup.shell.resolve(&request("kill -TERM $$"));
    let process = start(&setup.shell, spec).await;
    process.done().await;
    assert_eq!(process.status(), ShellProcessStatus::Killed);
    assert_eq!(process.exit_code(), None);
    assert_eq!(process.signal().as_deref(), Some("SIGTERM"));
}

#[tokio::test]
async fn observed_readers_delegate_to_collectors_once_spawned() {
    let setup = setup(json!({})).await;
    let spec = setup.shell.resolve(&request("echo err 1>&2"));
    let process = start(&setup.shell, spec).await;
    process.done().await;
    assert_eq!(process.observed().stderr.read_from(0).text, "err\n");
    assert_eq!(process.observed().stdout.read_from(0).text, "");
}

struct StaticReader(&'static str);
impl SubprocessOutputReader for StaticReader {
    fn read_from(&self, from_byte: u64) -> SubprocessOutputRead {
        let offset = (from_byte as usize).min(self.0.len());
        SubprocessOutputRead {
            text: self.0[offset..].to_owned(),
            next_offset: self.0.len() as u64,
            lossy: false,
            spill_path: None,
        }
    }
}

struct NoopLifecycle;
impl SubprocessLifecycle for NoopLifecycle {
    fn terminate(&self) {}
    fn wait_for_exit(&self, _signal: Option<AbortSignal>) -> SubprocessFuture<'_, bool> {
        futures_util::future::ok(true).boxed()
    }
}

struct FakeRuntime {
    done: SubprocessDone,
    stderr: &'static str,
}
impl FakeRuntime {
    fn new(done: SubprocessDone, stderr: &'static str) -> Arc<Self> {
        Arc::new(Self { done, stderr })
    }
}
impl SubprocessRuntime for FakeRuntime {
    fn resolve_executable<'a>(
        &'a self,
        command: &'a str,
        _env: Option<&'a IndexMap<String, String>>,
        _signal: Option<AbortSignal>,
    ) -> SubprocessFuture<'a, String> {
        futures_util::future::ok(command.to_owned()).boxed()
    }
    fn terminal_environment(
        &self,
        _signal: Option<AbortSignal>,
    ) -> SubprocessFuture<'_, SubprocessTerminalEnvironment> {
        futures_util::future::err(Box::new(FixtureError) as BoxError).boxed()
    }
    fn spawn(&self, _spec: SubprocessSpawnSpec) -> Result<SubprocessHandle, BoxError> {
        Ok(SubprocessHandle {
            stdin: None,
            stdout: None,
            stderr: None,
            control: None,
            collected: SubprocessCollectedOutputs {
                stdout: Some(Arc::new(StaticReader(""))),
                stderr: Some(Arc::new(StaticReader(self.stderr))),
            },
            done: self.done.clone(),
            lifecycle: Arc::new(NoopLifecycle),
        })
    }
    fn spawn_terminal(
        &self,
        _spec: SubprocessTerminalSpawnSpec,
    ) -> SubprocessFuture<'_, SubprocessTerminalHandle> {
        futures_util::future::err(Box::new(FixtureError) as BoxError).boxed()
    }
}

fn ordinary_error(message: &'static str) -> SharedError {
    SharedError::new(std::io::Error::other(message))
}

#[tokio::test]
async fn reports_unread_stderr_and_async_provider_rejection_exactly_once() {
    let done = subprocess_done(async { Err(ordinary_error("provider lost the direct outcome")) });
    let (_ctx, shell) = setup_fake(FakeRuntime::new(done, "target stderr"), json!({})).await;
    let spec = shell.resolve(&request("true"));
    let process = start(&shell, spec).await;
    process.done().await;
    assert_eq!(process.status(), ShellProcessStatus::Killed);
    let output = process.read_output().delta;
    assert!(output.contains("target stderr"));
    assert!(output.contains("subprocess failed before reporting an outcome:"));
    assert!(!output.contains("spawn failed:"));
    assert_eq!(process.read_output().delta, "");
}

#[tokio::test]
async fn rejection_after_own_abort_is_aborted_outcome_not_provider_failure() {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let done = subprocess_done(async { receiver.await.unwrap() });
    let (_ctx, shell) = setup_fake(FakeRuntime::new(done, ""), json!({})).await;
    let controller = AbortController::new();
    let mut request = request("true");
    request.signal = Some(controller.signal());
    let spec = shell.resolve(&request);
    let process = start(&shell, spec).await;
    controller.abort();
    sender
        .send(Err(controller.signal().reason().unwrap()))
        .unwrap();
    process.done().await;
    assert_eq!(process.status(), ShellProcessStatus::Killed);
    let result = process.result().await.unwrap();
    assert!(result.aborted);
    assert!(!result.timed_out);
    assert_eq!(result.exit_code, None);
    assert_eq!(process.read_output().delta, "");
}

#[tokio::test]
async fn rejection_after_kill_is_killed_outcome_not_provider_failure() {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let done = subprocess_done(async { receiver.await.unwrap() });
    let (_ctx, shell) = setup_fake(FakeRuntime::new(done, ""), json!({})).await;
    let spec = shell.resolve(&request("true"));
    let process = start(&shell, spec).await;
    assert!(process.kill());
    sender
        .send(Err(ordinary_error(
            "subprocess terminated before target start",
        )))
        .unwrap();
    process.done().await;
    assert_eq!(process.status(), ShellProcessStatus::Killed);
    let result = process.result().await.unwrap();
    assert!(!result.aborted);
    assert!(!result.timed_out);
    assert_eq!(result.exit_code, None);
    assert_eq!(process.read_output().delta, "");
}

#[derive(Debug)]
struct UnprintableError;
impl std::fmt::Display for UnprintableError {
    fn fmt(&self, _formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Err(std::fmt::Error)
    }
}
impl std::error::Error for UnprintableError {}

#[tokio::test]
async fn settles_unprintable_provider_rejection_instead_of_rejecting_done() {
    let done = subprocess_done(async { Err(SharedError::new(UnprintableError)) });
    let (_ctx, shell) = setup_fake(FakeRuntime::new(done, ""), json!({})).await;
    let spec = shell.resolve(&request("true"));
    let process = start(&shell, spec).await;
    process.done().await;
    assert_eq!(process.status(), ShellProcessStatus::Killed);
    assert!(
        process
            .read_output()
            .delta
            .contains("unprintable provider failure")
    );
    assert_eq!(process.read_output().delta, "");
}

#[tokio::test]
async fn asynchronous_creation_failure_settles_killed_with_stage_neutral_note() {
    let setup = setup(json!({})).await;
    let mut request = request("true");
    request.workdir = Some("/nonexistent-dsh".into());
    let spec = setup.shell.resolve(&request);
    let process = start(&setup.shell, spec).await;
    process.done().await;
    assert_eq!(process.status(), ShellProcessStatus::Killed);
    let first = process.observed().stderr.read_from(0);
    assert!(
        first
            .text
            .starts_with("subprocess failed before reporting an outcome: ")
    );
    assert_eq!(first.next_offset, first.text.len() as u64);
    assert!(!first.lossy);
    assert_eq!(
        process.observed().stderr.read_from(first.next_offset).text,
        ""
    );
    assert_eq!(process.observed().stdout.read_from(0).text, "");
    assert_eq!(
        process.read_output().delta,
        format!("[stderr]\n{}", first.text)
    );
    assert_eq!(process.read_output().delta, "");
}

#[tokio::test]
async fn background_process_survives_executor_disposal_and_dies_with_subprocess_service() {
    let setup = setup(json!({})).await;
    let spec = setup.shell.resolve(&request("echo $$; sleep 60"));
    let process = start(&setup.shell, spec).await;
    let pid = read_until(&*process, "\n")
        .await
        .trim()
        .parse::<u32>()
        .unwrap();
    assert!(pid > 0);
    setup.executor_fiber.dispose().await;
    assert_eq!(process.status(), ShellProcessStatus::Running);
    assert!(std::path::Path::new(&format!("/proc/{pid}")).exists());
    setup.manager_disposer.dispose().await;
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    process.done().await;
    assert_eq!(process.status(), ShellProcessStatus::Killed);
}

#[tokio::test]
async fn service_disposal_escalates_for_trapping_children_and_settles_handles() {
    let setup = setup(json!({})).await;
    let spec = setup.shell.resolve(&request("echo done"));
    let finished = start(&setup.shell, spec).await;
    finished.done().await;
    assert_eq!(finished.status(), ShellProcessStatus::Completed);
    let spec = setup
        .shell
        .resolve(&request("trap '' TERM; echo armed; sleep 60"));
    let trapping = start(&setup.shell, spec).await;
    read_until(&*trapping, "armed").await;
    setup.manager_disposer.dispose().await;
    assert_eq!(finished.status(), ShellProcessStatus::Completed);
    trapping.done().await;
    assert_eq!(trapping.status(), ShellProcessStatus::Killed);
    assert_eq!(trapping.signal().as_deref(), Some("SIGKILL"));
}

#[tokio::test]
async fn caller_abort_before_deadline_stays_first_caused_when_process_outlives_deadline() {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let done = subprocess_done(async { receiver.await.unwrap() });
    let (_ctx, shell) = setup_fake(FakeRuntime::new(done, ""), json!({})).await;
    let controller = AbortController::new();
    let mut request = request("sleep 30");
    request.timeout_ms = Some(20.0);
    request.signal = Some(controller.signal());
    let spec = shell.resolve(&request);
    let process = shell.execute(spec).await.unwrap();
    controller.abort();
    tokio::time::sleep(Duration::from_millis(40)).await;
    sender
        .send(Ok(SubprocessOutcome {
            exit_code: None,
            signal: Some("SIGTERM".into()),
        }))
        .unwrap();
    let result = process.result().await.unwrap();
    assert!(result.aborted);
    assert!(!result.timed_out);
}

#[tokio::test]
async fn none_policy_classifies_clean_settle_then_aborted_execution() {
    let setup = setup(json!({})).await;
    let spec = setup.shell.resolve(&request("echo bg"));
    let clean = start(&setup.shell, spec).await;
    clean.done().await;
    let result = clean.result().await.unwrap();
    assert_eq!(result.exit_code, Some(0));
    assert!(!result.timed_out);
    assert!(!result.aborted);

    let controller = AbortController::new();
    let mut request = request("sleep 30");
    request.signal = Some(controller.signal());
    let spec = setup.shell.resolve(&request);
    let killed = start(&setup.shell, spec).await;
    controller.abort();
    killed.done().await;
    let result = killed.result().await.unwrap();
    assert!(!result.timed_out);
    assert!(result.aborted);
}
