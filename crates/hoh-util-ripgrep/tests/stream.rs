#[path = "../src/spawn.rs"]
mod spawn;

use hoh_error::{BoxError, SharedError};
use hoh_subprocess::{
    SubprocessCollectedOutputs, SubprocessDone, SubprocessFuture, SubprocessHandle,
    SubprocessLifecycle, SubprocessOutcome, SubprocessOutputMode, SubprocessPlatform,
    SubprocessReadable, SubprocessRuntime, SubprocessSpawnSpec, SubprocessStdinMode,
    SubprocessStdio, SubprocessTerminalEnvironment, SubprocessTerminalHandle,
    SubprocessTerminalSpawnSpec, subprocess_done,
};
use hoh_util_ripgrep::{RipgrepSeparator, read_ripgrep_lines};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::io::{AsyncWriteExt, duplex};

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct TestFailure(&'static str);

fn unused<'a, T: 'a>() -> SubprocessFuture<'a, T> {
    Box::pin(async { Err(Box::new(TestFailure("unused")) as BoxError) })
}

struct Lifecycle {
    terminates: AtomicUsize,
    waits: AtomicUsize,
    released: AtomicBool,
    notify: tokio::sync::Notify,
    gated: bool,
}

impl Lifecycle {
    fn new(gated: bool) -> Arc<Self> {
        Arc::new(Self {
            terminates: AtomicUsize::new(0),
            waits: AtomicUsize::new(0),
            released: AtomicBool::new(!gated),
            notify: tokio::sync::Notify::new(),
            gated,
        })
    }

    fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
}

impl SubprocessLifecycle for Lifecycle {
    fn terminate(&self) {
        self.terminates.fetch_add(1, Ordering::SeqCst);
    }

    fn wait_for_exit(&self, _: Option<hoh_timeout::AbortSignal>) -> SubprocessFuture<'_, bool> {
        self.waits.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            while self.gated && !self.released.load(Ordering::SeqCst) {
                self.notify.notified().await;
            }
            Ok(true)
        })
    }
}

fn outcome() -> SubprocessOutcome {
    SubprocessOutcome {
        exit_code: Some(0),
        signal: None,
    }
}

fn handle(
    stdout: Option<SubprocessReadable>,
    done: SubprocessDone,
    lifecycle: Arc<Lifecycle>,
) -> SubprocessHandle {
    SubprocessHandle {
        stdin: None,
        stdout,
        stderr: None,
        control: None,
        collected: SubprocessCollectedOutputs::default(),
        done,
        lifecycle,
    }
}

fn successful_done() -> SubprocessDone {
    subprocess_done(async { Ok(outcome()) })
}

fn chunked_reader(chunks: Vec<Vec<u8>>) -> SubprocessReadable {
    let (reader, mut writer) = duplex(64);
    tokio::spawn(async move {
        for chunk in chunks {
            writer.write_all(&chunk).await.unwrap();
            tokio::task::yield_now().await;
        }
    });
    Box::new(reader)
}

fn spec() -> SubprocessSpawnSpec {
    let mut environment = indexmap::IndexMap::new();
    environment.insert("TEST".into(), Some("value".into()));
    SubprocessSpawnSpec {
        argv: vec!["--files".into()],
        cwd: "/workspace".into(),
        stdio: SubprocessStdio {
            stdin: SubprocessStdinMode::Ignore,
            stdout: SubprocessOutputMode::Pipe,
            stderr: SubprocessOutputMode::Pipe,
            control: None,
        },
        grace_ms: 5.0,
        signal: Some(hoh_timeout::AbortController::new().signal()),
        env: Some(environment),
    }
}

#[derive(Debug, PartialEq)]
struct SeenSpec {
    argv: Vec<String>,
    cwd: String,
    stdin: SubprocessStdinMode,
    stdout: SubprocessOutputMode,
    stderr: SubprocessOutputMode,
    grace_ms: f64,
    signal: bool,
    env: Option<hoh_subprocess::SubprocessEnvironment>,
}

struct RecordingRuntime {
    handle: Mutex<Option<SubprocessHandle>>,
    seen: Mutex<Vec<SeenSpec>>,
}

impl SubprocessRuntime for RecordingRuntime {
    fn resolve_executable<'a>(
        &'a self,
        _: &'a str,
        _: Option<&'a indexmap::IndexMap<String, String>>,
        _: Option<hoh_timeout::AbortSignal>,
    ) -> SubprocessFuture<'a, String> {
        unused()
    }

    fn terminal_environment(
        &self,
        _: Option<hoh_timeout::AbortSignal>,
    ) -> SubprocessFuture<'_, SubprocessTerminalEnvironment> {
        Box::pin(async {
            Ok(SubprocessTerminalEnvironment {
                platform: SubprocessPlatform::Posix,
                default_shell: None,
            })
        })
    }

    fn spawn(&self, spec: SubprocessSpawnSpec) -> Result<SubprocessHandle, BoxError> {
        self.seen.lock().unwrap().push(SeenSpec {
            argv: spec.argv,
            cwd: spec.cwd,
            stdin: spec.stdio.stdin,
            stdout: spec.stdio.stdout,
            stderr: spec.stdio.stderr,
            grace_ms: spec.grace_ms,
            signal: spec.signal.is_some(),
            env: spec.env,
        });
        self.handle
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| Box::new(TestFailure("missing handle")) as BoxError)
    }

    fn spawn_terminal(
        &self,
        _: SubprocessTerminalSpawnSpec,
    ) -> SubprocessFuture<'_, SubprocessTerminalHandle> {
        unused()
    }
}

struct FailingRuntime {
    failure: SharedError,
}

impl SubprocessRuntime for FailingRuntime {
    fn resolve_executable<'a>(
        &'a self,
        _: &'a str,
        _: Option<&'a indexmap::IndexMap<String, String>>,
        _: Option<hoh_timeout::AbortSignal>,
    ) -> SubprocessFuture<'a, String> {
        unused()
    }

    fn terminal_environment(
        &self,
        _: Option<hoh_timeout::AbortSignal>,
    ) -> SubprocessFuture<'_, SubprocessTerminalEnvironment> {
        unused()
    }

    fn spawn(&self, _: SubprocessSpawnSpec) -> Result<SubprocessHandle, BoxError> {
        Err(Box::new(self.failure.clone()))
    }

    fn spawn_terminal(
        &self,
        _: SubprocessTerminalSpawnSpec,
    ) -> SubprocessFuture<'_, SubprocessTerminalHandle> {
        unused()
    }
}

#[tokio::test]
async fn forwards_explicit_settings_and_prepends_no_config() {
    let lifecycle = Lifecycle::new(false);
    let runtime = RecordingRuntime {
        handle: Mutex::new(Some(handle(None, successful_done(), lifecycle))),
        seen: Mutex::new(Vec::new()),
    };
    let returned = spawn::spawn_ripgrep_at(&runtime, spec(), "/packaged/rg".into()).unwrap();
    assert!(returned.stdout.is_none());
    assert_eq!(
        *runtime.seen.lock().unwrap(),
        [SeenSpec {
            argv: vec![
                "/packaged/rg".into(),
                "--no-config".into(),
                "--files".into()
            ],
            cwd: "/workspace".into(),
            stdin: SubprocessStdinMode::Ignore,
            stdout: SubprocessOutputMode::Pipe,
            stderr: SubprocessOutputMode::Pipe,
            grace_ms: 5.0,
            signal: true,
            env: Some(indexmap::IndexMap::from([(
                "TEST".into(),
                Some("value".into())
            )])),
        }]
    );
}

#[test]
fn preserves_synchronous_provider_failures() {
    let failure = SharedError::new(Box::new(TestFailure("cannot spawn")) as BoxError);
    let runtime = FailingRuntime {
        failure: failure.clone(),
    };
    let error = match spawn::spawn_ripgrep_at(&runtime, spec(), "/packaged/rg".into()) {
        Ok(_) => panic!("spawn unexpectedly succeeded"),
        Err(error) => error,
    };
    assert!(SharedError::of(error.as_ref()).unwrap().ptr_eq(&failure));
}

#[tokio::test]
async fn decodes_split_utf8_and_delimiters_including_an_unterminated_final_record() {
    let bytes = "雪\0two\0tail".as_bytes();
    let stdout = chunked_reader(vec![
        bytes[..1].to_vec(),
        bytes[1..4].to_vec(),
        bytes[4..].to_vec(),
    ]);
    let lifecycle = Lifecycle::new(false);
    let mut lines = Vec::new();
    let result = read_ripgrep_lines(
        handle(Some(stdout), successful_done(), lifecycle.clone()),
        RipgrepSeparator::Nul,
        |line| {
            lines.push(line.to_owned());
            Ok(true)
        },
    )
    .await
    .unwrap();
    assert_eq!(lines, ["雪", "two", "tail"]);
    assert_eq!(
        result,
        hoh_util_ripgrep::RipgrepLinesResult {
            outcome: outcome(),
            stopped: false,
        }
    );
    assert_eq!(lifecycle.terminates.load(Ordering::SeqCst), 0);
    assert_eq!(lifecycle.waits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn replaces_invalid_utf8_per_complete_record() {
    let stdout = chunked_reader(vec![vec![b'a', 0xff], vec![b'b', 0]]);
    let lifecycle = Lifecycle::new(false);
    let mut lines = Vec::new();
    read_ripgrep_lines(
        handle(Some(stdout), successful_done(), lifecycle),
        RipgrepSeparator::Nul,
        |line| {
            lines.push(line.to_owned());
            Ok(true)
        },
    )
    .await
    .unwrap();
    assert_eq!(lines, ["a\u{fffd}b"]);
}

#[tokio::test]
async fn stops_callbacks_at_the_cap_while_draining_subsequent_chunks() {
    let stdout = chunked_reader(vec![b"one\ntwo\n".to_vec(), b"three\n".to_vec()]);
    let lifecycle = Lifecycle::new(false);
    let mut lines = Vec::new();
    let result = read_ripgrep_lines(
        handle(Some(stdout), successful_done(), lifecycle.clone()),
        RipgrepSeparator::Newline,
        |line| {
            lines.push(line.to_owned());
            Ok(false)
        },
    )
    .await
    .unwrap();
    assert!(result.stopped);
    assert_eq!(lines, ["one"]);
    assert_eq!(lifecycle.terminates.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn accepts_an_empty_stream_and_can_stop_at_the_final_unterminated_record() {
    let lifecycle = Lifecycle::new(false);
    let mut called = false;
    let result = read_ripgrep_lines(
        handle(Some(chunked_reader(vec![])), successful_done(), lifecycle),
        RipgrepSeparator::Newline,
        |_| {
            called = true;
            Ok(true)
        },
    )
    .await
    .unwrap();
    assert!(!result.stopped);
    assert!(!called);

    let lifecycle = Lifecycle::new(false);
    let result = read_ripgrep_lines(
        handle(
            Some(chunked_reader(vec![b"last".to_vec()])),
            successful_done(),
            lifecycle.clone(),
        ),
        RipgrepSeparator::Newline,
        |_| Ok(false),
    )
    .await
    .unwrap();
    assert!(result.stopped);
    assert_eq!(lifecycle.terminates.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn terminates_a_handle_without_piped_stdout_and_waits_for_its_outcome() {
    let lifecycle = Lifecycle::new(false);
    let done = subprocess_done(async {
        Err(SharedError::new(
            Box::new(TestFailure("launch failure")) as BoxError
        ))
    });
    let error = read_ripgrep_lines(
        handle(None, done, lifecycle.clone()),
        RipgrepSeparator::Newline,
        |_| Ok(true),
    )
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), "ripgrep requires piped stdout");
    assert_eq!(lifecycle.terminates.load(Ordering::SeqCst), 1);
    assert_eq!(lifecycle.waits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn propagates_callback_failures_after_terminating_and_awaiting_cleanup() {
    let lifecycle = Lifecycle::new(false);
    let failure = SharedError::new(Box::new(TestFailure("consumer failed")) as BoxError);
    let expected = failure.clone();
    let error = read_ripgrep_lines(
        handle(
            Some(chunked_reader(vec![b"line\n".to_vec()])),
            successful_done(),
            lifecycle.clone(),
        ),
        RipgrepSeparator::Newline,
        move |_| Err(Box::new(expected.clone())),
    )
    .await
    .unwrap_err();
    assert!(SharedError::of(error.as_ref()).unwrap().ptr_eq(&failure));
    assert_eq!(lifecycle.terminates.load(Ordering::SeqCst), 1);
    assert_eq!(lifecycle.waits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn settles_a_provider_failure_even_if_stdout_stays_open() {
    let (stdout, mut writer) = duplex(64);
    let lifecycle = Lifecycle::new(false);
    let failure = SharedError::new(Box::new(TestFailure("provider failed")) as BoxError);
    let expected = failure.clone();
    let done = subprocess_done(async move { Err(expected) });
    let error = read_ripgrep_lines(
        handle(Some(Box::new(stdout)), done, lifecycle.clone()),
        RipgrepSeparator::Newline,
        |_| Ok(true),
    )
    .await
    .unwrap_err();
    assert!(SharedError::of(error.as_ref()).unwrap().ptr_eq(&failure));
    assert!(writer.write_all(b"closed").await.is_err());
    assert_eq!(lifecycle.terminates.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn waits_for_the_managed_process_range_after_stdout_and_outcome_finish() {
    let lifecycle = Lifecycle::new(true);
    let task_lifecycle = lifecycle.clone();
    let task = tokio::spawn(async move {
        read_ripgrep_lines(
            handle(
                Some(chunked_reader(vec![b"line\n".to_vec()])),
                successful_done(),
                task_lifecycle,
            ),
            RipgrepSeparator::Newline,
            |_| Ok(true),
        )
        .await
    });
    while lifecycle.waits.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    assert!(!task.is_finished());
    lifecycle.release();
    assert!(!task.await.unwrap().unwrap().stopped);
}
