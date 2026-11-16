mod common;
use common::*;
use hoh_native_command::runner::*;
use std::error::Error;

#[tokio::test]
async fn captures_utf8_stdout_and_stderr_on_exit_0() {
    // A shell is an explicit fixture executable; the library never invokes one implicitly.
    #[cfg(unix)]
    let (command, args) = (
        "/bin/sh",
        vec!["-c".into(), "printf 'out✓'; printf err >&2".into()],
    );
    #[cfg(windows)]
    let (command, args) = ("powershell.exe", vec!["-NoProfile".into(), "-NonInteractive".into(), "-Command".into(), "[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false); [Console]::Out.Write('out✓'); [Console]::Error.Write('err')".into()]);
    let result = run_native_command(command, &args, &signal(), Window::Hidden)
        .await
        .unwrap();
    assert_eq!(
        result,
        CommandOutput {
            stdout: "out✓".into(),
            stderr: "err".into()
        }
    );
}
#[tokio::test]
async fn rejects_a_non_zero_exit_with_code_stdout_and_stderr_attached() {
    #[cfg(unix)]
    let (command, args) = (
        "/bin/sh",
        vec![
            "-c".into(),
            "printf partial; printf boom >&2; exit 3".into(),
        ],
    );
    #[cfg(windows)]
    let (command, args) = (
        "powershell.exe",
        vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            "[Console]::Out.Write('partial'); [Console]::Error.Write('boom'); exit 3".into(),
        ],
    );
    let failure = run_native_command(command, &args, &signal(), Window::Hidden)
        .await
        .unwrap_err();
    assert_eq!(detail(&failure).code, Some(CommandCode::Exit(3)));
    assert_eq!(detail(&failure).stdout.as_deref(), Some("partial"));
    assert_eq!(detail(&failure).stderr.as_deref(), Some("boom"));
    assert!(failure.source().is_some());
}
#[tokio::test]
async fn rejects_a_missing_executable_with_the_spawn_enoent_code() {
    let failure = run_native_command(
        "dsh-definitely-missing-command",
        &[],
        &signal(),
        Window::Hidden,
    )
    .await
    .unwrap_err();
    assert_eq!(
        detail(&failure).code,
        Some(CommandCode::Name("ENOENT".into()))
    );
}
#[tokio::test]
async fn terminates_the_child_when_the_signal_aborts() {
    let abort = AbortController::new();
    let request = abort.signal();
    #[cfg(unix)]
    let root = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    let marker = root.path().join("sleeping.pid");
    #[cfg(unix)]
    let (command, args) = (
        "/bin/sh",
        vec![
            "-c".into(),
            "printf '%s' \"$$\" > \"$1\"; exec /bin/sleep 60".into(),
            "fixture".into(),
            marker.to_string_lossy().into_owned(),
        ],
    );
    #[cfg(windows)]
    let (command, args) = (
        "powershell.exe",
        vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            "Start-Sleep -Seconds 60".into(),
        ],
    );
    let pending = run_native_command(command, &args, &request, Window::Hidden);
    tokio::pin!(pending);
    #[cfg(unix)]
    let pid = tokio::select! {
        result = &mut pending => panic!("unexpected result: {result:?}"),
        result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Ok(text) = tokio::fs::read_to_string(&marker).await
                    && let Ok(pid) = text.parse::<i32>() { break pid; }
                tokio::task::yield_now().await;
            }
        }) => result.unwrap(),
    };
    #[cfg(windows)]
    tokio::select! { result = &mut pending => panic!("unexpected result: {result:?}"), _ = tokio::task::yield_now() => {} }
    let reason = SharedError::new(std::io::Error::other("closed"));
    abort.abort_with(reason.clone());
    let failure = pending.await.unwrap_err();
    assert!(failure.ptr_eq(&reason));
    assert!(request.reason().unwrap().ptr_eq(&failure));
    #[cfg(unix)]
    assert_eq!(
        rustix::process::test_kill_process(rustix::process::Pid::from_raw(pid).unwrap()),
        Err(rustix::io::Errno::SRCH)
    );
}

#[tokio::test]
async fn already_aborted_commands_return_the_original_shared_reason() {
    let controller = AbortController::new();
    let reason = SharedError::new(std::io::Error::other("closed"));
    controller.abort_with(reason.clone());
    let error = run_native_command(
        "dsh-definitely-missing-command",
        &[],
        &controller.signal(),
        Window::Hidden,
    )
    .await
    .unwrap_err();
    assert!(error.ptr_eq(&reason));
}

#[cfg(unix)]
#[tokio::test]
async fn passes_metacharacters_as_literal_argv_and_limits_each_output_stream() {
    let text = "测试 ' $(touch nope); &";
    let result = run_native_command(
        "/usr/bin/printf",
        &["%s".into(), text.into()],
        &signal(),
        Window::Visible,
    )
    .await
    .unwrap();
    assert_eq!(result.stdout, text);
    let error = run_native_command(
        "/usr/bin/head",
        &["-c".into(), "1048577".into(), "/dev/zero".into()],
        &signal(),
        Window::Hidden,
    )
    .await
    .unwrap_err();
    assert_eq!(
        detail(&error).code,
        Some(CommandCode::Name(
            "ERR_CHILD_PROCESS_STDIO_MAXBUFFER".into()
        ))
    );
    assert_eq!(detail(&error).stdout.as_ref().unwrap().len(), 1048576);
}
