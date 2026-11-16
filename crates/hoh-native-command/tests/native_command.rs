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
    let result = run_native_command(command, &args, &Signal::default(), Window::Hidden)
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
    let failure = run_native_command(command, &args, &Signal::default(), Window::Hidden)
        .await
        .unwrap_err();
    assert_eq!(failure.code, Some(CommandCode::Exit(3)));
    assert_eq!(failure.stdout, "partial");
    assert_eq!(failure.stderr, "boom");
    assert!(failure.source().is_some());
}
#[tokio::test]
async fn rejects_a_missing_executable_with_the_spawn_enoent_code() {
    let failure = run_native_command(
        "dsh-definitely-missing-command",
        &[],
        &Signal::default(),
        Window::Hidden,
    )
    .await
    .unwrap_err();
    assert_eq!(failure.code, Some(CommandCode::Name("ENOENT".into())));
}
#[tokio::test]
async fn terminates_the_child_when_the_signal_aborts() {
    let abort = Signal::default();
    #[cfg(unix)]
    let (command, args) = ("/bin/sleep", vec!["60".into()]);
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
    let pending = run_native_command(command, &args, &abort, Window::Hidden);
    tokio::pin!(pending);
    tokio::select! { result = &mut pending => panic!("unexpected result: {result:?}"), _ = tokio::task::yield_now() => {} }
    abort.abort("closed");
    let failure = pending.await.unwrap_err();
    assert_eq!(failure.code, Some(CommandCode::Name("ABORT_ERR".into())));
    assert!(failure.source().is_some());
}

#[tokio::test]
async fn already_aborted_commands_keep_abort_code_and_original_reason() {
    let signal = Signal::default();
    signal.abort("closed");
    let error = run_native_command(
        "dsh-definitely-missing-command",
        &[],
        &signal,
        Window::Hidden,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, Some(CommandCode::Name("ABORT_ERR".into())));
    assert_eq!(error.source().unwrap().to_string(), "closed");
}

#[cfg(unix)]
#[tokio::test]
async fn passes_metacharacters_as_literal_argv_and_limits_each_output_stream() {
    let text = "测试 ' $(touch nope); &";
    let result = run_native_command(
        "/usr/bin/printf",
        &["%s".into(), text.into()],
        &Signal::default(),
        Window::Visible,
    )
    .await
    .unwrap();
    assert_eq!(result.stdout, text);
    let error = run_native_command(
        "/usr/bin/head",
        &["-c".into(), "1048577".into(), "/dev/zero".into()],
        &Signal::default(),
        Window::Hidden,
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.code,
        Some(CommandCode::Name(
            "ERR_CHILD_PROCESS_STDIO_MAXBUFFER".into()
        ))
    );
    assert_eq!(error.stdout.len(), 1048576);
}
