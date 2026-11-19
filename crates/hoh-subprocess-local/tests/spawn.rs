mod common;
use common::*;
use hoh_subprocess::*;
use hoh_subprocess_local::spawn::{
    SpawnInternals, kill_group, spawn_subprocess, validate_subprocess_spec,
};
use hoh_timeout::AbortController;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

macro_rules! batch_case {
    ($name:ident,$command:expr,$stdout:expr,$stderr:expr,$code:expr) => {
        #[tokio::test]
        async fn $name() {
            let (kernel, ctx, _) = setup();
            let handle = spawn_subprocess(&ctx, spec($command), SpawnInternals::default()).unwrap();
            let outcome = done(&handle).await;
            assert_eq!(outcome.exit_code, Some($code));
            assert_eq!(outcome.signal, None);
            assert_eq!(out(&handle).text, $stdout);
            assert!(!out(&handle).lossy);
            assert_eq!(
                handle.collected.stderr.as_ref().unwrap().read_from(0).text,
                $stderr
            );
            shutdown(kernel).await;
        }
    };
}
batch_case!(captures_stdout_on_success, "echo hello", "hello\n", "", 0);
batch_case!(captures_stderr_separately, "echo oops >&2", "", "oops\n", 0);
batch_case!(
    captures_both_streams,
    "echo out; echo err >&2",
    "out\n",
    "err\n",
    0
);
batch_case!(reports_non_zero_exit_codes, "exit 42", "", "", 42);
batch_case!(runs_in_the_requested_cwd, "pwd", "/tmp\n", "", 0);
batch_case!(
    a_command_that_reads_stdin_sees_eof_when_none_is_supplied,
    "cat",
    "",
    "",
    0
);

#[tokio::test]
async fn does_not_inherit_the_bootstrap_exec_acknowledgement_into_the_target() {
    let (kernel, ctx, _) = setup();
    let handle = spawn_subprocess(
        &ctx,
        spec("for fd in /proc/self/fd/*; do readlink \"$fd\"; done; true"),
        SpawnInternals::default(),
    )
    .unwrap();
    assert_eq!(done(&handle).await.exit_code, Some(0));
    let output = out(&handle).text;
    assert!(output.contains("socket:"), "{output}");
    assert!(!output.contains("exec-status.fifo"), "{output}");
    assert!(handle.lifecycle.wait_for_exit(None).await.unwrap());
    shutdown(kernel).await;
}

#[test]
fn rejects_an_invalid_grace_before_spawning() {
    for grace_ms in [
        0.0,
        -1.0,
        f64::NAN,
        f64::INFINITY,
        hoh_timeout::MAX_TIMER_DELAY_MS as f64 + 1.0,
    ] {
        let mut spec = spec("true");
        spec.grace_ms = grace_ms;
        assert_eq!(
            validate_subprocess_spec(&spec).unwrap_err().to_string(),
            format!(
                "subprocess graceMs must be a positive finite number no greater than {}",
                hoh_timeout::MAX_TIMER_DELAY_MS
            )
        );
    }
}
#[test]
fn rejects_an_empty_argv_before_spawning() {
    let mut spec = spec("true");
    spec.argv.clear();
    let error = validate_subprocess_spec(&spec).unwrap_err();
    assert_eq!(
        hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
        "SUBPROCESS_INVALID_ARGV"
    );
}
#[test]
fn rejects_an_empty_program_name_before_spawning() {
    let mut spec = spec("true");
    spec.argv[0].clear();
    assert!(
        validate_subprocess_spec(&spec)
            .unwrap_err()
            .to_string()
            .contains("invalid argv")
    );
}
#[test]
fn throws_a_stable_abort_error_when_already_aborted_before_spawn() {
    use hoh_subprocess_local::SubprocessAbortReason;
    for (reason, expected) in [
        (
            SubprocessAbortReason::Value(serde_json::json!("too late")),
            "too late",
        ),
        (
            SubprocessAbortReason::Value(serde_json::Value::Null),
            "aborted",
        ),
    ] {
        let controller = AbortController::new();
        controller.abort_with(reason);
        let mut request = spec("echo hi");
        request.signal = Some(controller.signal());
        let error = validate_subprocess_spec(&request).unwrap_err();
        assert!(hoh_timeout::is_abort_error(error.as_ref()));
        assert_eq!(
            error.to_string(),
            format!("aborted before spawn: {expected}")
        );
    }
}
#[test]
fn uses_a_stable_fallback_for_a_reason_less_pre_aborted_signal() {
    let controller = AbortController::new();
    controller.abort_with(hoh_subprocess_local::SubprocessAbortReason::Undefined);
    let mut request = spec("echo hi");
    request.signal = Some(controller.signal());
    assert_eq!(
        validate_subprocess_spec(&request).unwrap_err().to_string(),
        "aborted before spawn: aborted"
    );
}
#[test]
fn preserves_the_abort_error_name_of_a_default_reason() {
    let controller = AbortController::new();
    controller.abort();
    let mut request = spec("true");
    request.signal = Some(controller.signal());
    assert_eq!(
        validate_subprocess_spec(&request).unwrap_err().to_string(),
        "aborted before spawn: AbortError: This operation was aborted"
    );
}
#[test]
fn contains_an_abort_reason_whose_string_conversion_throws() {
    #[derive(Debug)]
    struct Reason;
    impl std::fmt::Display for Reason {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            panic!("reason formatting must not escape");
        }
    }
    impl std::error::Error for Reason {}
    let controller = AbortController::new();
    controller.abort_with(Reason);
    let mut request = spec("echo hi");
    request.signal = Some(controller.signal());
    assert_eq!(
        validate_subprocess_spec(&request).unwrap_err().to_string(),
        "aborted before spawn: aborted"
    );
}

#[tokio::test]
async fn bare_spill_failure_fixture() {
    if std::env::var_os("HOH_SPILL_FIXTURE").is_none() {
        return;
    }
    let (kernel, ctx, _) = setup();
    let dir = tempfile::tempdir().unwrap();
    let mut request = spec("for i in {1..200}; do printf 'line-%04d\\n' $i; done");
    request.stdio.stdout = collect(500);
    let handle = spawn_subprocess(
        &ctx,
        request,
        SpawnInternals {
            spill_dir: Some(dir.path().join("removed")),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(done(&handle).await.exit_code, Some(0));
    let read = out(&handle);
    assert!(read.lossy);
    assert!(read.text.contains("line-0200"));
    assert!(read.spill_path.is_none());
    shutdown(kernel).await;
}
#[tokio::test]
async fn writes_one_stderr_line_for_a_bare_spawn_whose_spill_directory_is_gone() {
    let output = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "bare_spill_failure_fixture", "--nocapture"])
        .env("HOH_SPILL_FIXTURE", "1")
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let lines = stderr
        .lines()
        .filter(|line| line.starts_with("dsh-subprocess-local:"))
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("stdout spill failed; only the in-memory tail is retained"));
    assert!(lines[0].contains("ENOENT"));
}
#[test]
fn kill_group_ignores_an_unpublished_pid() {
    kill_group(None, rustix::process::Signal::TERM);
}
#[test]
fn kill_group_swallows_esrch_for_vanished_groups() {
    kill_group(Some(1 << 30), rustix::process::Signal::TERM);
}

#[tokio::test]
async fn writes_stdin_to_the_command_and_closes_it() {
    let (kernel, ctx, _) = setup();
    let mut spec = spec("cat");
    spec.stdio.stdin = SubprocessStdinMode::Data {
        data: "hello from stdin\n".into(),
    };
    let handle = spawn_subprocess(&ctx, spec, SpawnInternals::default()).unwrap();
    assert!(handle.stdin.is_none());
    assert_eq!(done(&handle).await.exit_code, Some(0));
    assert_eq!(out(&handle).text, "hello from stdin\n");
    shutdown(kernel).await;
}
#[tokio::test]
async fn gives_fd_0_the_exact_pre_seam_type_dev_null_when_no_stdin_a_socket_when_supplied() {
    let (kernel, ctx, _) = setup();
    let none = spawn_subprocess(
        &ctx,
        spec("test -c /dev/stdin && echo char || echo other"),
        SpawnInternals::default(),
    )
    .unwrap();
    done(&none).await;
    assert_eq!(out(&none).text, "char\n");
    let mut spec = spec("test -S /dev/stdin && echo socket || echo other");
    spec.stdio.stdin = SubprocessStdinMode::Data { data: "x".into() };
    let piped = spawn_subprocess(&ctx, spec, SpawnInternals::default()).unwrap();
    done(&piped).await;
    assert_eq!(out(&piped).text, "socket\n");
    shutdown(kernel).await;
}
#[tokio::test]
async fn merges_ordinary_extra_env_entries_onto_the_scrubbed_environment() {
    let (kernel, ctx, _) = setup();
    let mut spec = spec("echo \"$EXTRA_ONE/$EXTRA_TWO\"");
    spec.env = Some(
        [
            ("EXTRA_ONE".into(), Some("alpha".into())),
            ("EXTRA_TWO".into(), Some("beta".into())),
        ]
        .into(),
    );
    let handle = spawn_subprocess(&ctx, spec, SpawnInternals::default()).unwrap();
    done(&handle).await;
    assert_eq!(out(&handle).text, "alpha/beta\n");
    shutdown(kernel).await;
}
#[tokio::test]
async fn an_explicit_extra_env_entry_overrides_the_credential_scrub() {
    let (kernel, ctx, _) = setup();
    let mut spec = spec("echo \"$EXPLICIT_OVERRIDE_PASSWORD\"");
    spec.env = Some(
        [(
            "EXPLICIT_OVERRIDE_PASSWORD".into(),
            Some("explicit-wins".into()),
        )]
        .into(),
    );
    let handle = spawn_subprocess(&ctx, spec, SpawnInternals::default()).unwrap();
    done(&handle).await;
    assert_eq!(out(&handle).text, "explicit-wins\n");
    shutdown(kernel).await;
}
#[tokio::test]
async fn passes_the_ambient_term_through_untouched_terminal_policy_is_the_callers() {
    let (kernel, ctx, _) = setup();
    let mut spec = spec("echo \"${TERM:-unset}\"");
    spec.env = Some([("TERM".into(), Some("callers-choice".into()))].into());
    let handle = spawn_subprocess(&ctx, spec, SpawnInternals::default()).unwrap();
    done(&handle).await;
    assert_eq!(out(&handle).text, "callers-choice\n");
    shutdown(kernel).await;
}
#[tokio::test]
async fn forwards_explicit_dsh_env_entries_while_scrubbing_ambient_ones() {
    let (kernel, ctx, _) = setup();
    let mut spec = spec("echo \"[$DSH_SHELL|$DSH_SESSION_ID]\"");
    spec.env = Some(
        [
            ("DSH_SHELL".into(), Some("bash".into())),
            ("DSH_SESSION_ID".into(), Some("session".into())),
        ]
        .into(),
    );
    let handle = spawn_subprocess(&ctx, spec, SpawnInternals::default()).unwrap();
    done(&handle).await;
    assert_eq!(out(&handle).text, "[bash|session]\n");
    shutdown(kernel).await;
}
#[tokio::test]
async fn does_not_crash_or_reject_when_the_child_ignores_a_large_stdin_epipe() {
    let (kernel, ctx, _) = setup();
    let mut spec = spec("exit 7");
    spec.stdio.stdin = SubprocessStdinMode::Data {
        data: "x".repeat(1024 * 1024),
    };
    let handle = spawn_subprocess(&ctx, spec, SpawnInternals::default()).unwrap();
    assert_eq!(done(&handle).await.exit_code, Some(7));
    shutdown(kernel).await;
}
#[tokio::test]
async fn pipe_exposes_raw_streams_for_caller_owned_protocol_decoding() {
    let (kernel, ctx, _) = setup();
    let mut spec = spec("cat");
    spec.stdio.stdin = SubprocessStdinMode::Pipe;
    spec.stdio.stdout = SubprocessOutputMode::Pipe;
    let mut handle = spawn_subprocess(&ctx, spec, SpawnInternals::default())
        .unwrap()
        .handle;
    assert!(handle.collected.stdout.is_none());
    assert!(handle.collected.stderr.is_some());
    assert!(handle.stderr.is_none());
    let mut input = handle.stdin.take().unwrap();
    input.write_all(b"through the pipe\n").await.unwrap();
    input.shutdown().await.unwrap();
    drop(input);
    let mut echoed = String::new();
    handle
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut echoed)
        .await
        .unwrap();
    assert_eq!(done(&handle).await.exit_code, Some(0));
    assert_eq!(echoed, "through the pipe\n");
    shutdown(kernel).await;
}
#[tokio::test]
async fn stderr_pipe_exposes_the_raw_stream() {
    let (kernel, ctx, _) = setup();
    let mut spec = spec("echo err >&2");
    spec.stdio.stderr = SubprocessOutputMode::Pipe;
    let mut handle = spawn_subprocess(&ctx, spec, SpawnInternals::default())
        .unwrap()
        .handle;
    let mut text = String::new();
    handle
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut text)
        .await
        .unwrap();
    done(&handle).await;
    assert_eq!(text, "err\n");
    assert!(handle.collected.stderr.is_none());
    shutdown(kernel).await;
}
#[tokio::test]
async fn kills_the_process_group_with_sigterm_when_the_signal_fires() {
    let (kernel, ctx, _) = setup();
    let controller = AbortController::new();
    let mut spec = spec("echo ready; exec sleep 60");
    spec.signal = Some(controller.signal());
    let handle = spawn_subprocess(&ctx, spec, SpawnInternals::default()).unwrap();
    wait_output(&handle, "ready\n").await;
    controller.abort_with(hoh_subprocess_local::LocalError::Error("deadline".into()));
    let result = done(&handle).await;
    assert_eq!(result.signal.as_deref(), Some("SIGTERM"));
    assert_eq!(result.exit_code, None);
    shutdown(kernel).await;
}
#[tokio::test]
async fn terminate_escalates_to_sigkill_when_sigterm_is_trapped() {
    let (kernel, ctx, _) = setup();
    let handle = spawn_subprocess(
        &ctx,
        spec("trap '' TERM; echo ready; while :; do sleep 60 & wait $!; done"),
        SpawnInternals::default(),
    )
    .unwrap();
    wait_output(&handle, "ready\n").await;
    handle.lifecycle.terminate();
    assert_eq!(done(&handle).await.signal.as_deref(), Some("SIGKILL"));
    shutdown(kernel).await;
}
#[tokio::test]
async fn terminate_is_idempotent_second_call_does_not_restart_escalation() {
    let (kernel, ctx, _) = setup();
    let handle = spawn_subprocess(
        &ctx,
        spec("echo ready; exec sleep 60"),
        SpawnInternals::default(),
    )
    .unwrap();
    wait_output(&handle, "ready\n").await;
    handle.lifecycle.terminate();
    handle.lifecycle.terminate();
    assert_eq!(done(&handle).await.signal.as_deref(), Some("SIGTERM"));
    assert!(handle.lifecycle.wait_for_exit(None).await.unwrap());
    shutdown(kernel).await;
}
#[tokio::test]
async fn wait_for_exit_on_a_failed_spawn_reports_exited_immediately() {
    let (kernel, ctx, _) = setup();
    let mut spec = spec("true");
    spec.argv = vec!["/missing-hoh-command".into()];
    let handle = spawn_subprocess(&ctx, spec, SpawnInternals::default()).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), handle.done.clone())
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("ENOENT")
    );
    assert!(handle.lifecycle.wait_for_exit(None).await.unwrap());
    shutdown(kernel).await;
}

#[tokio::test]
async fn fallback_spawn_failure_retains_empty_collected_outputs_without_a_helper() {
    let (kernel, ctx, _) = setup();
    let mut request = spec("unused");
    request.argv = vec!["/definitely/missing-hoh-target".into()];
    let handle = spawn_subprocess(
        &ctx,
        request,
        SpawnInternals {
            runner_invocation: Some(vec!["/definitely/missing-hoh-runner".into()]),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        handle
            .done
            .clone()
            .await
            .unwrap_err()
            .to_string()
            .contains("ENOENT")
    );
    for output in [
        handle.collected.stdout.as_ref().unwrap(),
        handle.collected.stderr.as_ref().unwrap(),
    ] {
        let read = output.read_from(0);
        assert_eq!(read.text, "");
        assert!(!read.lossy);
        assert!(read.spill_path.is_none());
    }
    assert!(handle.lifecycle.wait_for_exit(None).await.unwrap());
    shutdown(kernel).await;
}

#[tokio::test(flavor = "current_thread")]
async fn delayed_exec_ack_does_not_block_the_executor_and_abort_still_terminates() {
    let (kernel, ctx, _) = setup();
    let helper = hoh_subprocess_local::runner_launch::spawn_runner_invocation()
        .into_iter()
        .next()
        .unwrap();
    let delayed_runner = vec![
        "/bin/sh".into(),
        "-c".into(),
        "sleep 3; helper=$1; shift; exec \"$helper\" \"$@\"".into(),
        "hoh-delayed-runner".into(),
        helper,
    ];
    let controller = AbortController::new();
    let mut request = spec("exec sleep 60");
    request.signal = Some(controller.signal());
    let started = std::time::Instant::now();
    let handle = spawn_subprocess(
        &ctx,
        request,
        SpawnInternals {
            runner_invocation: Some(delayed_runner),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "spawn blocked the current-thread executor for {:?}",
        started.elapsed()
    );
    controller.abort();
    let outcome = tokio::time::timeout(Duration::from_secs(2), handle.done.clone())
        .await
        .expect("abort did not interrupt exec acknowledgement")
        .unwrap();
    assert_eq!(outcome.signal.as_deref(), Some("SIGTERM"));
    assert!(handle.lifecycle.wait_for_exit(None).await.unwrap());
    shutdown(kernel).await;
}
#[tokio::test]
async fn rejects_with_a_spawn_error_for_a_nonexistent_cwd() {
    let (kernel, ctx, _) = setup();
    let mut spec = spec("echo hi");
    spec.cwd = "/nonexistent-dir-dsh-test".into();
    let handle = spawn_subprocess(&ctx, spec, SpawnInternals::default()).unwrap();
    assert!(
        handle
            .done
            .clone()
            .await
            .unwrap_err()
            .to_string()
            .contains("ENOENT")
    );
    assert!(handle.lifecycle.wait_for_exit(None).await.unwrap());
    shutdown(kernel).await;
}
#[tokio::test]
async fn applies_stdout_and_stderr_caps_independently() {
    let (kernel, ctx, _) = setup();
    let dir = tempfile::tempdir().unwrap();
    let mut spec = spec("printf '%.0sx' {1..500}; printf '%.0se' {1..500} >&2");
    spec.stdio.stdout = collect(500);
    spec.stdio.stderr = collect(100);
    let handle = spawn_subprocess(
        &ctx,
        spec,
        SpawnInternals {
            spill_dir: Some(dir.path().into()),
            ..Default::default()
        },
    )
    .unwrap();
    done(&handle).await;
    assert_eq!(out(&handle).text, "x".repeat(500));
    assert!(!out(&handle).lossy);
    let errors = handle.collected.stderr.as_ref().unwrap().read_from(0);
    assert!(errors.lossy);
    assert_eq!(errors.text, "e".repeat(100));
    shutdown(kernel).await;
}
#[tokio::test]
async fn keeps_the_tail_and_spills_the_full_stream_to_disk() {
    let (kernel, ctx, _) = setup();
    let dir = tempfile::tempdir().unwrap();
    let mut spec = spec("for i in {1..200}; do printf 'line-%04d\\n' $i; done");
    spec.stdio.stdout = collect(500);
    let handle = spawn_subprocess(
        &ctx,
        spec,
        SpawnInternals {
            spill_dir: Some(dir.path().into()),
            ..Default::default()
        },
    )
    .unwrap();
    done(&handle).await;
    let read = out(&handle);
    assert!(read.lossy);
    assert!(read.text.len() <= 500);
    assert!(read.text.contains("line-0200"));
    assert!(!read.text.contains("line-0001"));
    let full = std::fs::read_to_string(read.spill_path.unwrap()).unwrap();
    assert!(full.contains("line-0001"));
    assert!(full.contains("line-0200"));
    shutdown(kernel).await;
}
#[tokio::test]
async fn a_collect_mode_without_spill_keeps_only_the_in_memory_tail_no_file() {
    let (kernel, ctx, _) = setup();
    let mut spec = spec("for i in {1..200}; do printf 'line-%04d\\n' $i; done");
    spec.stdio.stdout = SubprocessOutputMode::Collect(SubprocessCollect {
        max_bytes: 100,
        spill: None,
    });
    let handle = spawn_subprocess(&ctx, spec, SpawnInternals::default()).unwrap();
    done(&handle).await;
    let read = out(&handle);
    assert!(read.lossy);
    assert!(read.text.contains("line-0200"));
    assert!(read.spill_path.is_none());
    shutdown(kernel).await;
}

#[tokio::test]
async fn reports_the_terminating_signal_of_an_externally_self_killed_command() {
    let (kernel, ctx, _) = setup();
    let handle = spawn_subprocess(&ctx, spec("kill -KILL $$"), SpawnInternals::default()).unwrap();
    let outcome = done(&handle).await;
    assert_eq!(outcome.exit_code, None);
    assert_eq!(outcome.signal.as_deref(), Some("SIGKILL"));
    shutdown(kernel).await;
}
#[tokio::test]
async fn spawns_argv_verbatim_without_shell_interpretation() {
    let (kernel, ctx, _) = setup();
    let mut request = spec("unused");
    request.argv = vec![
        "/bin/printf".into(),
        "%s".into(),
        "a b; $HOME `true` $(false)".into(),
    ];
    let handle = spawn_subprocess(&ctx, request, SpawnInternals::default()).unwrap();
    assert_eq!(done(&handle).await.exit_code, Some(0));
    assert_eq!(out(&handle).text, "a b; $HOME `true` $(false)");
    shutdown(kernel).await;
}
#[test]
fn validates_every_node_baseline_nul_location_before_launch() {
    use hoh_subprocess_local::{LocalError, runner_launch::target_environment};
    for location in 0..5 {
        let mut request = spec("true");
        match location {
            0 => request.argv[0] = "node\0".into(),
            1 => request.argv.push("a\0".into()),
            2 => request.cwd = "bad\0cwd".into(),
            3 => request.env = Some([("BAD\0KEY".into(), Some("x".into()))].into()),
            4 => request.env = Some([("BAD".into(), Some("x\0".into()))].into()),
            _ => unreachable!(),
        }
        let error = target_environment(&request).unwrap_err();
        let error = error.downcast_ref::<LocalError>().unwrap();
        assert_eq!(error.name(), "TypeError");
        assert_eq!(error.code(), Some("ERR_INVALID_ARG_VALUE"));
        assert!(
            error
                .to_string()
                .contains("must be a string without null bytes")
        );
    }
}
#[test]
fn an_explicit_tombstone_removes_an_ordinary_ambient_env_entry() {
    // Use an existing entry and an explicit map: never mutate the test process's env.
    let Some((key, _)) = std::env::vars_os().find(|(key, _)| key == "PATH") else {
        return;
    };
    let env = hoh_subprocess_local::spawn::child_env(Some(
        &[(key.to_string_lossy().into_owned(), None)].into(),
    ));
    assert!(!env.contains_key("PATH"));
}

struct ChildPid(u32);
impl Drop for ChildPid {
    fn drop(&mut self) {
        if let Some(pid) = rustix::process::Pid::from_raw(self.0 as i32) {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
        }
    }
}
async fn pid_from_output(handle: &SubprocessHandle) -> ChildPid {
    wait_output(handle, "CHILD:").await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(pid) = out(handle)
                .text
                .lines()
                .find_map(|line| line.strip_prefix("CHILD:").and_then(|pid| pid.parse().ok()))
            {
                return ChildPid(pid);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
async fn wait_gone(pid: u32) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"));
            if stat
                .ok()
                .and_then(|text| hoh_subprocess_local::process_inspector::parse_proc_stat(&text))
                .is_none_or(|stat| hoh_subprocess_local::process_inspector::quiescent(&stat.state))
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("descendant remains live");
}
#[tokio::test]
async fn terminates_the_whole_process_group_grandchildren_die_too() {
    let (kernel, ctx, _) = setup();
    let handle = spawn_subprocess(
        &ctx,
        spec("sleep 60 & echo CHILD:$!; wait"),
        SpawnInternals::default(),
    )
    .unwrap();
    let pid = pid_from_output(&handle).await;
    handle.lifecycle.terminate();
    assert_eq!(done(&handle).await.signal.as_deref(), Some("SIGTERM"));
    wait_gone(pid.0).await;
    assert!(handle.lifecycle.wait_for_exit(None).await.unwrap());
    shutdown(kernel).await;
}
#[tokio::test]
async fn bounds_inherited_pipe_draining_after_the_shell_exits() {
    let (kernel, ctx, _) = setup();
    let handle = spawn_subprocess(
        &ctx,
        spec("sleep 60 & echo CHILD:$!; echo shell-done"),
        SpawnInternals::default(),
    )
    .unwrap();
    let pid = pid_from_output(&handle).await;
    assert_eq!(done(&handle).await.exit_code, Some(0));
    assert!(out(&handle).text.ends_with("shell-done\n"));
    let controller = AbortController::new();
    controller.abort();
    assert!(
        !handle
            .lifecycle
            .wait_for_exit(Some(controller.signal()))
            .await
            .unwrap()
    );
    handle.lifecycle.terminate();
    assert!(handle.lifecycle.wait_for_exit(None).await.unwrap());
    wait_gone(pid.0).await;
    shutdown(kernel).await;
}
#[tokio::test]
async fn an_aborted_wait_reports_false_while_the_tree_lives() {
    let (kernel, ctx, _) = setup();
    let handle = spawn_subprocess(
        &ctx,
        spec("echo ready; exec sleep 60"),
        SpawnInternals::default(),
    )
    .unwrap();
    wait_output(&handle, "ready\n").await;
    let controller = AbortController::new();
    let observation = handle.lifecycle.wait_for_exit(Some(controller.signal()));
    controller.abort();
    assert!(!observation.await.unwrap());
    handle.lifecycle.terminate();
    assert!(handle.lifecycle.wait_for_exit(None).await.unwrap());
    shutdown(kernel).await;
}
#[tokio::test]
async fn force_kills_the_current_process_tree_without_waiting_for_the_normal_grace() {
    let (kernel, ctx, _) = setup();
    let mut request = spec("trap '' TERM; sleep 60 & echo CHILD:$!; wait");
    request.grace_ms = 60_000.0;
    let handle = spawn_subprocess(&ctx, request, SpawnInternals::default()).unwrap();
    let pid = pid_from_output(&handle).await;
    handle.lifecycle.terminate_for_host_exit();
    assert_eq!(done(&handle).await.signal.as_deref(), Some("SIGKILL"));
    wait_gone(pid.0).await;
    shutdown(kernel).await;
}
#[tokio::test]
async fn terminate_sigkills_a_term_trapping_descendant_after_the_direct_child_settles() {
    let (kernel, ctx, _) = setup();
    let handle = spawn_subprocess(
        &ctx,
        spec("bash -c 'trap \"\" TERM; echo CHILD:$$; while :; do sleep 60; done' & wait"),
        SpawnInternals::default(),
    )
    .unwrap();
    let pid = pid_from_output(&handle).await;
    handle.lifecycle.terminate();
    assert_eq!(done(&handle).await.signal.as_deref(), Some("SIGTERM"));
    assert!(handle.lifecycle.wait_for_exit(None).await.unwrap());
    wait_gone(pid.0).await;
    shutdown(kernel).await;
}
#[tokio::test]
async fn repeated_terminate_after_exit_never_signals_a_reused_process_group() {
    let (kernel, ctx, _) = setup();
    let handle = spawn_subprocess(&ctx, spec("true"), SpawnInternals::default()).unwrap();
    done(&handle).await;
    assert!(handle.lifecycle.wait_for_exit(None).await.unwrap());
    handle.lifecycle.terminate();
    handle.lifecycle.terminate();
    handle.lifecycle.terminate_for_host_exit();
    assert!(handle.lifecycle.wait_for_exit(None).await.unwrap());
    shutdown(kernel).await;
}
#[tokio::test]
async fn inherit_with_a_collected_other_stream_wires_only_the_requested_collector() {
    for inherited_stdout in [true, false] {
        let (kernel, ctx, _) = setup();
        let mut request = spec(if inherited_stdout {
            "echo err >&2"
        } else {
            "echo out"
        });
        if inherited_stdout {
            request.stdio.stdout = SubprocessOutputMode::Inherit;
        } else {
            request.stdio.stderr = SubprocessOutputMode::Inherit;
        }
        let handle = spawn_subprocess(&ctx, request, SpawnInternals::default()).unwrap();
        assert_eq!(done(&handle).await.exit_code, Some(0));
        assert!(handle.stdout.is_none());
        assert!(handle.stderr.is_none());
        if inherited_stdout {
            assert!(handle.collected.stdout.is_none());
            assert_eq!(
                handle.collected.stderr.as_ref().unwrap().read_from(0).text,
                "err\n"
            );
        } else {
            assert!(handle.collected.stderr.is_none());
            assert_eq!(out(&handle).text, "out\n");
        }
        shutdown(kernel).await;
    }
}
#[tokio::test]
async fn creates_spill_files_with_owner_only_permissions_and_random_names_in_a_private_directory() {
    use std::os::unix::fs::PermissionsExt;
    let (kernel, ctx, _) = setup();
    let storage = std::sync::Arc::new(hoh_subprocess_local::output::SpillStorage::default());
    let mut paths = vec![];
    for _ in 0..2 {
        let mut request = spec("printf 123456789");
        request.stdio.stdout = collect(1);
        let handle = spawn_subprocess(
            &ctx,
            request,
            SpawnInternals {
                spill_storage: Some(storage.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        done(&handle).await;
        let path = std::path::PathBuf::from(out(&handle).spill_path.unwrap());
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"123456789");
        paths.push(path);
    }
    assert_ne!(paths[0], paths[1]);
    assert_eq!(paths[0].parent(), paths[1].parent());
    shutdown(kernel).await;
    for path in paths {
        std::fs::remove_file(path).unwrap();
    }
}
