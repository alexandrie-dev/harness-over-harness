//! One foreground deadline covers sandbox preparation and native execution. Virtual time
//! (`start_paused`) replaces the fake `setTimeout`/`clearTimeout` of the source; the native
//! subprocess is a scripted handle whose termination is driven by the abort signal it was given.

mod support;

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use hoh_error::{BoxError, SharedError};
use hoh_sandbox::{ConfinedArgv, SandboxEnforcement, SandboxMode};
use hoh_shell::{ShellExecRequest, ShellRunResult, ShellSandboxInfo, types::CollectedOutput};
use hoh_subprocess::{
    SubprocessCollectedOutputs, SubprocessFuture, SubprocessHandle, SubprocessLifecycle,
    SubprocessOutcome, SubprocessOutputRead, SubprocessOutputReader, subprocess_done,
};
use hoh_timeout::{AbortController, AbortSignal, TimeoutReason};
use serde_json::json;
use support::*;
use tokio::sync::oneshot;

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

struct Terminating {
    terminate: Arc<dyn Fn() + Send + Sync>,
    done: hoh_subprocess::SubprocessDone,
}
impl SubprocessLifecycle for Terminating {
    fn terminate(&self) {
        (self.terminate)();
    }
    fn wait_for_exit(&self, _signal: Option<AbortSignal>) -> SubprocessFuture<'_, bool> {
        let done = self.done.clone();
        Box::pin(async move {
            let _ = done.await;
            Ok(true)
        })
    }
}

#[derive(Debug)]
struct Plain(&'static str);
impl std::fmt::Display for Plain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Plain {}

fn reason(text: &'static str) -> BoxError {
    Box::new(Plain(text))
}

type Prepared = Result<ConfinedArgv, BoxError>;

/// A foreground run observed from outside, like the source's `observed` record.
struct Run {
    observed: Arc<Mutex<Option<Result<ShellRunResult, BoxError>>>>,
}

impl Run {
    fn done(&self) -> bool {
        lock(&self.observed).is_some()
    }
    fn result(&self) -> ShellRunResult {
        match lock(&self.observed).as_ref() {
            Some(Ok(result)) => result.clone(),
            other => panic!("expected a result, saw {other:?}"),
        }
    }
    fn error(&self) -> BoxError {
        match lock(&self.observed).take() {
            Some(Err(error)) => error,
            other => panic!("expected an error, saw {other:?}"),
        }
    }
}

struct Fixture {
    s: Setup,
    entered: Mutex<Option<oneshot::Receiver<Option<AbortSignal>>>>,
    prepared: Mutex<Option<oneshot::Sender<Prepared>>>,
    spawned: Arc<Mutex<Option<AbortSignal>>>,
    terminated: Arc<AtomicUsize>,
}

fn fixture_wrap() -> ConfinedArgv {
    ConfinedArgv {
        argv: vec!["fixture-runner".into()],
        enforcement: SandboxEnforcement::Full,
        denial_signatures: vec![],
        runner_failure_rules: vec![],
    }
}

async fn fixture() -> Fixture {
    let (entered_tx, entered_rx) = oneshot::channel();
    let (prepared_tx, prepared_rx) = oneshot::channel::<Prepared>();
    let entered_tx = Mutex::new(Some(entered_tx));
    let prepared_rx = Mutex::new(Some(prepared_rx));
    let behavior: Behavior = Arc::new(move |_argv, _policy, signal| {
        if let Some(entered) = lock(&entered_tx).take() {
            let _ = entered.send(signal);
        }
        let pending = lock(&prepared_rx).take();
        Box::pin(async move {
            match pending {
                Some(pending) => pending
                    .await
                    .unwrap_or_else(|_| Err(reason("confinement dropped"))),
                None => Err(reason("confinement already answered")),
            }
        })
    });
    let s = setup(Options {
        behavior,
        executor: json!({"timeoutMs": 100, "graceMs": 100}),
        ..Options::default()
    })
    .await;
    let spawned = Arc::new(Mutex::new(None));
    let terminated = Arc::new(AtomicUsize::new(0));
    {
        let spawned = spawned.clone();
        let terminated = terminated.clone();
        s.subprocess.set(SpawnMode::Handle(Arc::new(move |spec| {
            *lock(&spawned) = spec.signal.clone();
            let (complete, completion) = oneshot::channel::<SubprocessOutcome>();
            let complete = Arc::new(Mutex::new(Some(complete)));
            let terminate: Arc<dyn Fn() + Send + Sync> = {
                let complete = complete.clone();
                let terminated = terminated.clone();
                Arc::new(move || {
                    terminated.fetch_add(1, Ordering::SeqCst);
                    if let Some(complete) = lock(&complete).take() {
                        let _ = complete.send(SubprocessOutcome {
                            exit_code: None,
                            signal: Some("SIGTERM".into()),
                        });
                    }
                })
            };
            if let Some(signal) = spec.signal.clone() {
                let terminate = terminate.clone();
                tokio::spawn(async move {
                    signal.cancelled().await;
                    terminate();
                });
            }
            let done = subprocess_done(async move {
                completion
                    .await
                    .map_err(|_| SharedError::new(Plain("completion dropped")))
            });
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
                done: done.clone(),
                lifecycle: Arc::new(Terminating { terminate, done }),
            }
        })));
    }
    Fixture {
        s,
        entered: Mutex::new(Some(entered_rx)),
        prepared: Mutex::new(Some(prepared_tx)),
        spawned,
        terminated,
    }
}

impl Fixture {
    async fn entered(&self) -> Option<AbortSignal> {
        let entered = lock(&self.entered).take().expect("entered once");
        entered.await.expect("the provider was consulted")
    }

    fn prepare(&self, answer: Prepared) {
        if let Some(prepared) = lock(&self.prepared).take() {
            let _ = prepared.send(answer);
        }
    }

    fn confines(&self) -> usize {
        self.s.sandbox.calls().len()
    }

    fn start(&self, timeout_ms: f64, signal: Option<AbortSignal>) -> Run {
        let observed = Arc::new(Mutex::new(None));
        let spec = self.s.resolve(ShellExecRequest {
            timeout_ms: Some(timeout_ms),
            signal,
            ..request("fixture command")
        });
        let shell = self.s.shell.clone();
        let sink = observed.clone();
        tokio::spawn(async move {
            let outcome = match shell.execute(spec).await {
                Ok(process) => process.result().await,
                Err(error) => Err(error),
            };
            *lock(&sink) = Some(outcome);
        });
        Run { observed }
    }
}

async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

/// `vi.advanceTimersByTimeAsync`: move virtual time, then let every woken task run.
async fn advance(ms: u64) {
    settle().await;
    tokio::time::advance(Duration::from_millis(ms)).await;
    settle().await;
}

fn empty() -> CollectedOutput {
    CollectedOutput {
        text: String::new(),
        truncated: false,
        spill_path: None,
    }
}

async fn times_out_preparation_and_prevents_late_outcome_from_spawning(late_success: bool) {
    let test = fixture().await;
    let run = test.start(10.0, None);
    let signal = test.entered().await.expect("preparation carries a signal");
    advance(10).await;
    assert!(run.done());
    assert!(signal.aborted());
    assert_eq!(
        run.result(),
        ShellRunResult {
            exit_code: None,
            signal: None,
            timed_out: true,
            aborted: false,
            timeout_ms: 10.0,
            stdout: empty(),
            stderr: empty(),
            sandbox: Some(ShellSandboxInfo {
                mode: SandboxMode::ReadOnly,
                denied: false,
                enforcement: None,
                runner_failed: None,
            }),
        }
    );
    assert_eq!(test.s.subprocess.spawn_count(), 0);
    test.prepare(if late_success {
        Ok(fixture_wrap())
    } else {
        Err(reason("late preparation rejection"))
    });
    advance(0).await;
    assert_eq!(test.s.subprocess.spawn_count(), 0);
    test.s.close().await;
}

#[tokio::test(start_paused = true)]
async fn times_out_preparation_and_prevents_late_success_from_spawning() {
    times_out_preparation_and_prevents_late_outcome_from_spawning(true).await;
}

#[tokio::test(start_paused = true)]
async fn times_out_preparation_and_prevents_late_rejection_from_spawning() {
    times_out_preparation_and_prevents_late_outcome_from_spawning(false).await;
}

#[tokio::test(start_paused = true)]
async fn passes_the_remaining_deadline_to_native_execution_instead_of_restarting_it() {
    let test = fixture().await;
    let run = test.start(100.0, None);
    let preparation_signal = test.entered().await.expect("preparation carries a signal");
    advance(60).await;
    test.prepare(Ok(fixture_wrap()));
    advance(0).await;
    let spawn_signal = lock(&test.spawned).clone().expect("native spawn happened");
    // The same deadline signal, not a restarted one: it is not aborted until the original 100ms
    // elapse, and both views then carry the identical reason.
    assert!(!spawn_signal.aborted());
    assert!(!preparation_signal.aborted());
    advance(39).await;
    assert!(!run.done());
    advance(1).await;
    let spawn_reason = spawn_signal.reason().expect("deadline fired");
    assert!(spawn_reason.ptr_eq(&preparation_signal.reason().expect("deadline fired")));
    assert_eq!(
        spawn_reason
            .downcast_ref::<TimeoutReason>()
            .map(|r| r.timeout_ms),
        Some(100.0)
    );
    let result = run.result();
    assert!(result.timed_out);
    assert!(!result.aborted);
    assert_eq!(result.signal.as_deref(), Some("SIGTERM"));
    assert_eq!(
        result.sandbox.and_then(|sandbox| sandbox.enforcement),
        Some(SandboxEnforcement::Full)
    );
    assert_eq!(test.terminated.load(Ordering::SeqCst), 1);
    test.s.close().await;
}

#[tokio::test(start_paused = true)]
async fn preserves_upstream_cancellation_when_it_wins_during_preparation() {
    let test = fixture().await;
    let controller = AbortController::new();
    let stopped = reason("caller stopped preparation");
    let run = test.start(10.0, Some(controller.signal()));
    test.entered().await;
    controller.abort_with(stopped);
    advance(20).await;
    assert!(run.done());
    let error = run.error();
    assert_eq!(error.to_string(), "caller stopped preparation");
    let expected = controller.signal().reason().expect("aborted");
    assert!(
        error
            .downcast_ref::<SharedError>()
            .is_some_and(|thrown| thrown.ptr_eq(&expected)),
        "the caller's own reason is rethrown"
    );
    assert_eq!(test.s.subprocess.spawn_count(), 0);
    test.prepare(Ok(fixture_wrap()));
    advance(0).await;
    assert_eq!(test.s.subprocess.spawn_count(), 0);
    test.s.close().await;
}

#[tokio::test(start_paused = true)]
async fn keeps_timeout_as_the_first_cause_when_upstream_cancellation_follows_its_notification() {
    let test = fixture().await;
    let controller = Arc::new(AbortController::new());
    let run = test.start(10.0, Some(controller.signal()));
    let signal = test.entered().await.expect("preparation carries a signal");
    {
        let controller = controller.clone();
        tokio::spawn(async move {
            signal.cancelled().await;
            controller.abort_with(reason("later caller abort"));
        });
    }
    advance(10).await;
    assert!(controller.signal().aborted());
    let result = run.result();
    assert!(result.timed_out);
    assert!(!result.aborted);
    assert_eq!(test.s.subprocess.spawn_count(), 0);
    test.s.close().await;
}

#[tokio::test(start_paused = true)]
async fn does_not_prepare_after_a_pre_existing_caller_cancellation() {
    let test = fixture().await;
    let controller = AbortController::new();
    controller.abort_with(reason("already cancelled"));
    let run = test.start(10.0, Some(controller.signal()));
    advance(0).await;
    let error = run.error();
    assert_eq!(error.to_string(), "already cancelled");
    let expected = controller.signal().reason().expect("aborted");
    assert!(
        error
            .downcast_ref::<SharedError>()
            .is_some_and(|thrown| thrown.ptr_eq(&expected))
    );
    assert_eq!(test.confines(), 0);
    assert_eq!(test.s.subprocess.spawn_count(), 0);
    test.s.close().await;
}

#[tokio::test(start_paused = true)]
async fn preserves_preparation_failures_and_disposes_the_deadline() {
    let test = fixture().await;
    let run = test.start(10.0, None);
    test.entered().await;
    let original = SharedError::new(reason("confinement unavailable"));
    test.prepare(Err(Box::new(original.clone())));
    advance(0).await;
    let error = run.error();
    assert_eq!(error.to_string(), "confinement unavailable");
    // `rejects.toBe(error)`: the provider's own error object, not a copy.
    assert!(
        error
            .downcast_ref::<SharedError>()
            .is_some_and(|thrown| thrown.ptr_eq(&original)),
        "{error:?}"
    );
    assert_eq!(test.s.subprocess.spawn_count(), 0);
    // The disposed deadline never fires afterwards: nothing is left to abort or spawn.
    advance(1_000).await;
    assert_eq!(test.s.subprocess.spawn_count(), 0);
    test.s.close().await;
}
