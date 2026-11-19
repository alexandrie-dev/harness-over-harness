mod common;
use common::*;
use hoh_subprocess::*;
use hoh_subprocess_local::LocalSubprocessRuntime;
use hoh_timeout::AbortController;
use std::sync::Arc;
#[tokio::test]
async fn registers_as_ctx_subprocess_and_spawns_managed_handles() {
    let (kernel, ctx, runtime) = setup();
    provide_subprocess(&ctx, runtime).await.unwrap();
    let runtime = ctx.get::<Subprocess>().unwrap();
    let handle = runtime.spawn(spec("echo hello")).unwrap();
    assert_eq!(done(&handle).await.exit_code, Some(0));
    assert_eq!(out(&handle).text, "hello\n");
    shutdown(kernel).await;
}
#[tokio::test]
async fn resolves_absolute_and_path_executables_and_honors_lookup_cancellation() {
    let (kernel, _, runtime) = setup();
    assert_eq!(
        runtime
            .resolve_executable("/bin/sh", None, None)
            .await
            .unwrap(),
        "/bin/sh"
    );
    let env = [("PATH".into(), "/definitely/missing:/bin".into())].into();
    assert_eq!(
        runtime
            .resolve_executable("sh", Some(&env), None)
            .await
            .unwrap(),
        "/bin/sh"
    );
    assert_eq!(
        runtime
            .resolve_executable("./sh", None, None)
            .await
            .unwrap_err()
            .to_string(),
        "subprocess-local: command \"./sh\" is a relative path; use an absolute path or a bare PATH name"
    );
    let failure = runtime
        .resolve_executable("/definitely/missing", None, None)
        .await
        .unwrap_err();
    assert_eq!(
        failure.to_string(),
        "subprocess-local: command \"/definitely/missing\" is not an executable file"
    );
    assert_eq!(
        hoh_error::HarnessError::of(failure.as_ref())
            .unwrap()
            .code(),
        "SUBPROCESS_EXECUTABLE_NOT_FOUND"
    );
    let controller = AbortController::new();
    controller.abort_with(hoh_subprocess_local::LocalError::Error(
        "lookup cancelled".into(),
    ));
    let reason = controller.signal().reason().unwrap();
    let failure = runtime
        .resolve_executable("sh", None, Some(controller.signal()))
        .await
        .unwrap_err();
    assert!(
        hoh_error::SharedError::of(failure.as_ref())
            .unwrap()
            .ptr_eq(&reason)
    );
    shutdown(kernel).await;
}
#[tokio::test]
async fn discovers_the_platform_shell_without_inventing_a_missing_default_and_honors_cancellation()
{
    let (kernel, _, runtime) = setup();
    let result = runtime.terminal_environment(None).await.unwrap();
    assert_eq!(result.platform, SubprocessPlatform::Posix);
    if let Some(shell) = std::env::var_os("SHELL").filter(|s| !s.is_empty()) {
        assert_eq!(
            result.default_shell.as_deref(),
            Some(shell.to_string_lossy().as_ref())
        );
    }
    let controller = AbortController::new();
    controller.abort();
    assert!(
        runtime
            .terminal_environment(Some(controller.signal()))
            .await
            .is_err()
    );
    shutdown(kernel).await;
}
#[tokio::test]
async fn validates_terminal_allocation_inputs_before_allocating_a_pty() {
    let (kernel, _, runtime) = setup();
    let mut spec = terminal_spec();
    spec.argv.clear();
    let error = match runtime.spawn_terminal(spec).await {
        Err(error) => error,
        Ok(_) => panic!("empty terminal argv accepted"),
    };
    assert_eq!(
        error.to_string(),
        "subprocess-local: terminal argv must contain a program"
    );
    let controller = AbortController::new();
    controller.abort();
    let mut spec = terminal_spec();
    spec.signal = Some(controller.signal());
    assert!(runtime.spawn_terminal(spec).await.is_err());
    shutdown(kernel).await;
}
#[tokio::test]
async fn warns_once_when_ordinary_spawns_use_the_weaker_linux_fallback() {
    let (kernel, ctx, runtime) = setup();
    for _ in 0..2 {
        let handle = runtime.spawn(spec("true")).unwrap();
        done(&handle).await;
    }
    let records = ctx.logger().buffer();
    let warnings = records
        .iter()
        .filter(|record| record.message.contains("subprocess-local is using weaker"))
        .collect::<Vec<_>>();
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        warnings[0].message,
        "subprocess-local is using weaker process-tree containment because the current user-systemd scope or private bootstrap is unavailable; descendants that escape the process group or direct-parent tree are not guaranteed to terminate or delay waitForExit()"
    );
    shutdown(kernel).await;
}
#[tokio::test]
async fn disposal_kills_an_ordinary_child_and_joins_its_exit() {
    let (kernel, _, runtime) = setup();
    let handle = runtime.spawn(spec("echo ready; exec sleep 60")).unwrap();
    wait_output(&handle, "ready\n").await;
    shutdown(kernel).await;
    assert_eq!(
        handle.done.clone().await.unwrap().signal.as_deref(),
        Some("SIGTERM")
    );
    assert!(handle.wait_for_exit(None).await.unwrap());
}
#[tokio::test]
async fn disposal_kills_both_the_process_and_any_surviving_grandchild() {
    let (kernel, _, runtime) = setup();
    let handle = runtime.spawn(spec("sleep 60 & echo ready; wait")).unwrap();
    wait_output(&handle, "ready\n").await;
    shutdown(kernel).await;
    assert!(handle.wait_for_exit(None).await.unwrap());
}
#[tokio::test]
async fn runtime_can_be_provided_as_its_declared_service_trait() {
    let (kernel, ctx, runtime) = setup();
    let provider: Arc<dyn SubprocessRuntime> = runtime;
    let disposer = provide_subprocess(&ctx, provider).await.unwrap();
    assert!(ctx.get::<Subprocess>().is_some());
    disposer.dispose().await;
    shutdown(kernel).await;
}
#[test]
fn plugin_row_name_is_preserved() {
    use hoh_kernel::Plugin;
    assert_eq!(LocalSubprocessRuntime::NAME, "LocalSubprocessRuntime");
}

#[tokio::test]
async fn logs_one_error_through_the_plugin_logger_when_a_spill_cannot_be_written_and_keeps_the_tail()
 {
    let (kernel, ctx, runtime) = setup();
    let directory = tempfile::tempdir().unwrap();
    runtime.internals.lock().unwrap().spill_dir = Some(directory.path().join("removed"));
    let mut request = spec("printf '%.0sx' {1..4096}");
    request.stdio.stdout = collect(16);
    request.stdio.stderr = SubprocessOutputMode::Pipe;
    let handle = runtime.spawn(request).unwrap();
    assert_eq!(done(&handle).await.exit_code, Some(0));
    let read = out(&handle);
    assert_eq!(read.text, "x".repeat(16));
    assert!(read.lossy);
    assert!(read.spill_path.is_none());
    let records = ctx.logger().buffer();
    let errors = records
        .iter()
        .filter(|record| {
            record
                .message
                .contains("could not write the complete stdout stream")
        })
        .collect::<Vec<_>>();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message.contains("temporary-file cleaner"));
    assert!(errors[0].message.contains("ENOENT"));
    shutdown(kernel).await;
}

#[tokio::test]
async fn disposal_tolerates_a_handle_whose_spawn_already_failed_or_races_teardown() {
    for await_failure in [false, true] {
        let (kernel, _, runtime) = setup();
        let mut request = spec("true");
        request.cwd = "/nonexistent-dir-dsh-subprocess-test".into();
        let handle = runtime.spawn(request).unwrap();
        if await_failure {
            assert!(handle.done.clone().await.is_err());
        }
        shutdown(kernel).await;
        let error = handle
            .done
            .clone()
            .await
            .expect_err("fallback startup must reject even when teardown wins the race");
        if await_failure {
            assert!(error.to_string().contains("ENOENT"), "{error}");
        }
        assert!(handle.wait_for_exit(None).await.unwrap());
    }
}

#[tokio::test]
async fn loading_a_second_implementation_throws_one_processes_service_per_context() {
    let (kernel, ctx, runtime) = setup();
    provide_subprocess(&ctx, runtime).await.unwrap();
    let another =
        Arc::new(LocalSubprocessRuntime::new_with_observations(ctx.clone(), NoManager).unwrap());
    assert!(provide_subprocess(&ctx, another).await.is_err());
    shutdown(kernel).await;
}
