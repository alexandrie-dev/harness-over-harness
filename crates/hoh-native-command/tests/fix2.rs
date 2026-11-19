#![cfg(unix)]
mod common;
use common::*;
use hoh_native_command::{runner::*, *};
use std::{error::Error, future::Future, time::Duration};

struct Fixture {
    root: tempfile::TempDir,
    ignore_term: bool,
}
impl Fixture {
    fn new(ignore_term: bool) -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
            ignore_term,
        }
    }
    fn marker(&self) -> std::path::PathBuf {
        self.root.path().join("pid")
    }
    fn args(&self) -> Vec<String> {
        vec![
            "-c".into(),
            format!(
                "{}printf 'started\\n'; printf 'partial✓' >&2; printf '%s' \"$$\" > \"$1\"; exec /bin/sleep 3",
                if self.ignore_term {
                    "trap '' TERM; "
                } else {
                    ""
                }
            ),
            "fixture".into(),
            self.marker().to_string_lossy().into_owned(),
        ]
    }
    async fn started(&self) -> i32 {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(text) = tokio::fs::read_to_string(self.marker()).await
                    && let Ok(pid) = text.parse()
                {
                    break pid;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap()
    }
}
struct Cleanup(rustix::process::Pid);
impl Cleanup {
    async fn finish(self) {
        let pid = self.0;
        drop(self);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match rustix::process::test_kill_process(pid) {
                    Err(rustix::io::Errno::SRCH) => break,
                    Ok(()) => tokio::task::yield_now().await,
                    Err(error) => panic!("failed to check fixture process: {error}"),
                }
            }
        })
        .await
        .expect("background reaper must remove the terminated child's zombie");
    }
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        // Test fixture cleanup only: the runner must never escalate cancellation to SIGKILL.
        let _ = rustix::process::kill_process(self.0, rustix::process::Signal::KILL);
    }
}
async fn abort_after_started<T: std::fmt::Debug>(
    fixture: &Fixture,
    pending: impl Future<Output = Result<T, SharedError>>,
    controller: &AbortController,
) -> (SharedError, Cleanup) {
    tokio::pin!(pending);
    let pid = tokio::select! {
        result = &mut pending => panic!("command completed before abort: {result:?}"),
        pid = fixture.started() => pid,
    };
    let cleanup = Cleanup(rustix::process::Pid::from_raw(pid).unwrap());
    // Give the runner a poll to consume both pipes after the fixture's ready marker.
    tokio::select! { biased; result = &mut pending => panic!("early completion: {result:?}"), _ = tokio::task::yield_now() => {} }
    controller.abort_with(std::io::Error::other("closed"));
    let error = tokio::time::timeout(Duration::from_secs(1), &mut pending)
        .await
        .expect("abort must return without waiting for the process or pipes")
        .unwrap_err();
    (error, cleanup)
}
#[tokio::test]
async fn abort_returns_promptly_with_partial_output_and_an_identical_nested_reason() {
    for ignore_term in [true, false] {
        let fixture = Fixture::new(ignore_term);
        let controller = AbortController::new();
        let request = controller.signal();
        let args = fixture.args();
        let (error, cleanup) = abort_after_started(
            &fixture,
            run_native_command("/bin/sh", &args, &request, Window::Hidden),
            &controller,
        )
        .await;
        assert_eq!(error.to_string(), "The operation was aborted");
        let detail = detail(&error);
        assert_eq!(detail.code, Some(CommandCode::Name("ABORT_ERR".into())));
        assert_eq!(detail.stdout.as_deref(), Some("started\n"));
        assert_eq!(detail.stderr.as_deref(), Some("partial✓"));
        let abort_error = detail.cause.as_ref().unwrap();
        assert!(
            abort_error
                .downcast_ref::<hoh_timeout::AbortError>()
                .is_some()
        );
        assert_eq!(abort_error.to_string(), "The operation was aborted");
        let reason = SharedError::of(abort_error.source().unwrap()).unwrap();
        assert!(reason.ptr_eq(&request.reason().unwrap()));
        if ignore_term {
            assert!(rustix::process::test_kill_process(cleanup.0).is_ok());
        }
        cleanup.finish().await;
    }
}
struct ExplorerFixtureRunner {
    args: Vec<String>,
}
impl NativeCommandRunner for ExplorerFixtureRunner {
    fn run<'a>(
        &'a self,
        command: &'a str,
        _: &'a [String],
        signal: &'a AbortSignal,
        window: Window,
    ) -> CommandFuture<'a, Result<CommandOutput, SharedError>> {
        assert_eq!(command, "explorer.exe");
        assert_eq!(window, Window::Visible);
        Box::pin(run_native_command("/bin/sh", &self.args, signal, window))
    }
}
#[tokio::test]
async fn explorer_opener_rethrows_the_reason_after_a_real_command_aborts() {
    let fixture = Fixture::new(true);
    let controller = AbortController::new();
    let request = controller.signal();
    let run = ExplorerFixtureRunner {
        args: fixture.args(),
    };
    let internals = facts("win32", &run);
    let (error, cleanup) = abort_after_started(
        &fixture,
        open_native_path("C:\\work\\file.txt", &request, &internals),
        &controller,
    )
    .await;
    assert!(error.ptr_eq(&request.reason().unwrap()));
    assert!(rustix::process::test_kill_process(cleanup.0).is_ok());
    cleanup.finish().await;
}

#[tokio::test]
async fn default_abort_reason_is_reachable_through_the_runner_abort_error() {
    let controller = AbortController::new();
    controller.abort();
    let request = controller.signal();
    let error = run_native_command(
        "/bin/sh",
        &["-c".into(), "exit 0".into()],
        &request,
        Window::Hidden,
    )
    .await
    .unwrap_err();
    assert_eq!(
        detail(&error).code,
        Some(CommandCode::Name("ABORT_ERR".into()))
    );
    let abort_error = detail(&error).cause.as_ref().unwrap();
    let reason = SharedError::of(abort_error.source().unwrap()).unwrap();
    assert!(reason.ptr_eq(&request.reason().unwrap()));
    assert!(reason.downcast_ref::<hoh_timeout::AbortError>().is_some());
    assert_eq!(reason.to_string(), "This operation was aborted");
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[tokio::test]
async fn no_interpreter_script_is_refused_by_the_real_runner_on_glibc() {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("no-interpreter");
    {
        // Linux returns ETXTBSY when exec races an outstanding writer. Keep creation synchronous
        // and close that writer explicitly before testing the runner's no-shebang ENOEXEC path.
        let mut script = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o755)
            .open(&path)
            .unwrap();
        script
            .write_all("printf 'no-interpreter✓\\n'\n".as_bytes())
            .unwrap();
        script.sync_all().unwrap();
    }
    let error = run_native_command(path.to_str().unwrap(), &[], &signal(), Window::Hidden)
        .await
        .expect_err("observe whether Rust/libc falls back to /bin/sh");
    assert_eq!(
        detail(&error).code,
        Some(CommandCode::Name("ENOEXEC".into()))
    );
    assert_eq!(error.to_string(), "spawn ENOEXEC");
    assert_eq!(detail(&error).stdout, None);
    assert_eq!(detail(&error).stderr, None);
}
