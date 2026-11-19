//! The execution handle of the local bash executor: launch, settlement, consuming reads, kill and the
//! memoized result projection (the object literal `proc` of `executeArgv` in the source).

use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, OnceLock},
};

use futures_util::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use hoh_error::{BoxError, SharedError};
use hoh_shell::{
    CollectedOutput, ShellExecSpec, ShellExecution, ShellExpiryPolicy, ShellProcess,
    ShellProcessRead, ShellProcessStatus, ShellRunResult, ShellSandboxInfo,
    types::{ShellDone, ShellFuture, ShellObservedStreams, shell_done},
};
use hoh_subprocess::{
    SubprocessLifecycle, SubprocessOutcome, SubprocessOutputRead, SubprocessOutputReader,
};
use hoh_timeout::{AbortController, AbortSignal, Deadline, timeout_of};

use crate::{LocalBashExecutor, js_error_string, lock};

pub type ArgvFuture = Pin<Box<dyn Future<Output = Result<Vec<String>, BoxError>> + Send>>;
/// Argv preparation that runs under the execution's cancellation signal.
pub type PrepareArgv = Box<dyn FnOnce(AbortSignal) -> ArgvFuture + Send>;
/// Settlement hook of the source's protected `onProcessDone(proc, stderr, providerRejected, providerError)`.
pub type ProcessDoneHook =
    Arc<dyn Fn(&BashExecution, &str, bool, Option<&SharedError>) + Send + Sync>;

/// `onStarted` of the source: installs provider facts before the handle can settle.
pub type StartedHook = Box<dyn FnOnce(&BashExecution) + Send>;
type SharedResult = Shared<BoxFuture<'static, Result<ShellRunResult, SharedError>>>;

/// `argvOrPrepare` of the source: an exact argv or a preparation of one.
pub enum ArgvSource {
    Ready(Vec<String>),
    Prepare(PrepareArgv),
}

/// Subclass extension points of `executeArgv`: `onStarted` and the overridable `onProcessDone`.
#[derive(Default)]
pub struct ExecuteHooks {
    /// Installs provider facts synchronously before the handle can settle; not called when
    /// preparation timed out.
    pub on_started: Option<StartedHook>,
    /// Called after exit facts or provider-failure output are stamped and before `done` resolves.
    pub on_process_done: Option<ProcessDoneHook>,
}

/// How `classify` of the source reads the first cause, per expiry policy.
enum Classify {
    /// `onExpiry: 'kill'`: one fused deadline of timeout and upstream cancellation.
    Deadline(AbortSignal),
    /// `onExpiry: 'none'`: callers stop the process through `kill()` or `spec.signal`.
    Spec(Option<AbortSignal>),
}

impl Classify {
    /// `(timedOut, aborted)`.
    fn get(&self) -> (bool, bool) {
        match self {
            Self::Deadline(signal) => {
                let timed_out = is_bash_timeout(signal);
                (timed_out, signal.aborted() && !timed_out)
            }
            Self::Spec(signal) => (false, signal.as_ref().is_some_and(AbortSignal::aborted)),
        }
    }
}

fn is_bash_timeout(signal: &AbortSignal) -> bool {
    timeout_of(signal.reason().as_ref(), Some("BASH_TIMEOUT")).is_some()
}

struct ProviderFailure {
    error: SharedError,
    note: String,
}

struct State {
    status: ShellProcessStatus,
    exit_code: Option<i64>,
    signal: Option<String>,
    sandbox: Option<ShellSandboxInfo>,
    stdout_offset: u64,
    stderr_offset: u64,
    failure_reported: bool,
}

struct Inner {
    state: Mutex<State>,
    /// Set once by the settlement; shared with the observed stderr reader.
    failure: Arc<Mutex<Option<ProviderFailure>>>,
    stdout: Arc<dyn SubprocessOutputReader>,
    stderr: Arc<dyn SubprocessOutputReader>,
    observed: ShellObservedStreams,
    lifecycle: Option<Arc<dyn SubprocessLifecycle>>,
    spawn_signal: Option<AbortSignal>,
    classify: Classify,
    timeout_ms: f64,
    done: OnceLock<ShellDone>,
    result: Mutex<Option<SharedResult>>,
    on_process_done: Option<ProcessDoneHook>,
}

/// The live execution handle of one command. Cheap to clone; all clones share one process.
#[derive(Clone)]
pub struct BashExecution {
    inner: Arc<Inner>,
}

impl BashExecution {
    /// Attach execution facts to a process (the source's `proc.sandbox = ...` in a subclass).
    pub fn set_sandbox(&self, sandbox: Option<ShellSandboxInfo>) {
        lock(&self.inner.state).sandbox = sandbox;
    }

    /// Stamp the settled status as `killed` (a subclass classification).
    fn settle_status(&self, status: ShellProcessStatus) {
        lock(&self.inner.state).status = status;
    }
}

impl ShellProcess for BashExecution {
    fn status(&self) -> ShellProcessStatus {
        lock(&self.inner.state).status
    }

    fn exit_code(&self) -> Option<i64> {
        lock(&self.inner.state).exit_code
    }

    fn signal(&self) -> Option<String> {
        lock(&self.inner.state).signal.clone()
    }

    fn done(&self) -> ShellDone {
        match self.inner.done.get() {
            Some(done) => done.clone(),
            // Unreachable after `launch` returned: `done` is set before the handle is published.
            None => shell_done(async {}),
        }
    }

    fn sandbox(&self) -> Option<ShellSandboxInfo> {
        lock(&self.inner.state).sandbox.clone()
    }

    fn read_output(&self) -> ShellProcessRead {
        let (out, err, provider_note) = {
            let mut state = lock(&self.inner.state);
            let out = self.inner.stdout.read_from(state.stdout_offset);
            let err = self.inner.stderr.read_from(state.stderr_offset);
            state.stdout_offset = out.next_offset;
            state.stderr_offset = err.next_offset;
            // The consuming read folds the provider failure note in exactly once.
            let note = match lock(&self.inner.failure).as_ref() {
                Some(failure) if !state.failure_reported => {
                    state.failure_reported = true;
                    failure.note.clone()
                }
                _ => String::new(),
            };
            (out, err, note)
        };
        let failure_separator = if !err.text.is_empty() && !err.text.ends_with('\n') {
            "\n"
        } else {
            ""
        };
        let err_text = if provider_note.is_empty() {
            err.text.clone()
        } else {
            format!("{}{failure_separator}{provider_note}", err.text)
        };
        // Single newline between sections: stdout chunks usually end with one already; add it only
        // when missing.
        let separator = if !out.text.is_empty() && !out.text.ends_with('\n') {
            "\n"
        } else {
            ""
        };
        let delta = if err_text.is_empty() {
            out.text.clone()
        } else {
            format!("{}{separator}[stderr]\n{err_text}", out.text)
        };
        ShellProcessRead {
            delta,
            lossy: out.lossy || err.lossy,
            stdout_spill_path: out.spill_path,
            stderr_spill_path: err.spill_path,
        }
    }

    fn observed(&self) -> ShellObservedStreams {
        self.inner.observed.clone()
    }

    fn kill(&self) -> bool {
        {
            let mut state = lock(&self.inner.state);
            if state.status != ShellProcessStatus::Running {
                return false;
            }
            state.status = ShellProcessStatus::Killed;
        }
        if let Some(lifecycle) = &self.inner.lifecycle {
            lifecycle.terminate();
        }
        true
    }
}

impl ShellExecution for BashExecution {
    fn result(&self) -> ShellFuture<'_, ShellRunResult> {
        let shared = {
            let mut slot = lock(&self.inner.result);
            // `resultPromise ??= ...`: one projection, repeated calls share its outcome.
            slot.get_or_insert_with(|| {
                let execution = self.clone();
                async move { project_result(&execution).await }
                    .boxed()
                    .shared()
            })
            .clone()
        };
        Box::pin(async move { shared.await.map_err(|error| Box::new(error) as BoxError) })
    }
}

/// Project a settled collect-mode reader into the final `CollectedOutput` shape.
fn final_output(reader: &Arc<dyn SubprocessOutputReader>) -> CollectedOutput {
    let read = reader.read_from(0);
    CollectedOutput {
        text: read.text,
        truncated: read.lossy,
        spill_path: read.spill_path,
    }
}

async fn project_result(execution: &BashExecution) -> Result<ShellRunResult, SharedError> {
    execution.done().await;
    let inner = &execution.inner;
    // Infrastructure-failure parity with the historical foreground path: a spawn that never
    // produced a process rejects the projection.
    if let Some(failure) = lock(&inner.failure).as_ref() {
        return Err(failure.error.clone());
    }
    let (timed_out, aborted) = inner.classify.get();
    let (exit_code, signal, sandbox) = {
        let state = lock(&inner.state);
        (state.exit_code, state.signal.clone(), state.sandbox.clone())
    };
    Ok(ShellRunResult {
        exit_code,
        signal,
        timed_out,
        aborted,
        timeout_ms: inner.timeout_ms,
        stdout: final_output(&inner.stdout),
        stderr: final_output(&inner.stderr),
        sandbox,
    })
}

struct EmptyReader;

impl SubprocessOutputReader for EmptyReader {
    fn read_from(&self, _from_byte: u64) -> SubprocessOutputRead {
        SubprocessOutputRead {
            text: String::new(),
            next_offset: 0,
            lossy: false,
            spill_path: None,
        }
    }
}

/// Serves the provider failure note as the whole stderr stream once the provider rejected.
struct ObservedStderr {
    base: Arc<dyn SubprocessOutputReader>,
    failure: Arc<Mutex<Option<ProviderFailure>>>,
}

impl SubprocessOutputReader for ObservedStderr {
    fn read_from(&self, from_byte: u64) -> SubprocessOutputRead {
        let note = lock(&self.failure)
            .as_ref()
            .map(|failure| failure.note.clone());
        match note {
            None => self.base.read_from(from_byte),
            Some(note) => {
                let bytes = note.as_bytes();
                let offset =
                    usize::try_from(from_byte).map_or(bytes.len(), |from| from.min(bytes.len()));
                SubprocessOutputRead {
                    text: String::from_utf8_lossy(&bytes[offset..]).into_owned(),
                    next_offset: bytes.len() as u64,
                    lossy: false,
                    spill_path: None,
                }
            }
        }
    }
}

enum Spawned {
    Handle(hoh_subprocess::SubprocessDone),
    /// Preparation timed out: nothing spawned, an already settled outcome.
    Settled(SubprocessOutcome),
    /// A synchronous spawn throw, contained into the same settled-killed shape.
    Rejected(SharedError),
}

#[derive(Debug, thiserror::Error)]
#[error("bash-local: subprocess implementation dropped a requested collect stream")]
struct MissingCollectStream;

pub(crate) async fn launch(
    executor: &LocalBashExecutor,
    spec: ShellExecSpec,
    argv: ArgvSource,
    hooks: ExecuteHooks,
) -> Result<BashExecution, BoxError> {
    // Deadline wiring by expiry policy: the spawn signal, the first-cause classification and the
    // disarm the settlement continuation runs. `ShellExpiryPolicy` has exactly two members.
    let (spawn_signal, classify, mut deadline): (Option<AbortSignal>, Classify, Option<Deadline>) =
        match spec.on_expiry {
            ShellExpiryPolicy::Kill => {
                // One fused deadline combines timeout and upstream cancellation; only this
                // executor's timeout reason counts as timedOut, outer deadlines as aborts.
                let deadline =
                    hoh_timeout::deadline(spec.signal.as_ref(), spec.timeout_ms, "BASH_TIMEOUT")?;
                (
                    Some(deadline.signal.clone()),
                    Classify::Deadline(deadline.signal.clone()),
                    Some(deadline),
                )
            }
            ShellExpiryPolicy::None => (
                spec.signal.clone(),
                Classify::Spec(spec.signal.clone()),
                None,
            ),
        };

    let mut preparation_timed_out = false;
    let argv = match argv {
        ArgvSource::Ready(argv) => argv,
        ArgvSource::Prepare(prepare) => {
            let signal = spawn_signal
                .clone()
                .unwrap_or_else(|| AbortController::new().signal());
            let prepared = async {
                signal.throw_if_aborted()?;
                prepare(signal.clone()).await
            };
            let outcome = tokio::select! {
                result = prepared => result,
                reason = signal.cancelled() => Err(Box::new(reason) as BoxError),
            };
            let outcome = match outcome {
                Ok(argv) => signal
                    .throw_if_aborted()
                    .map(|()| argv)
                    .map_err(|reason| Box::new(reason) as BoxError),
                Err(error) => Err(error),
            };
            match outcome {
                Ok(argv) => argv,
                Err(error) => {
                    if !classify.get().0 {
                        if let Some(deadline) = &mut deadline {
                            deadline.dispose();
                        }
                        return Err(error);
                    }
                    preparation_timed_out = true;
                    Vec::new()
                }
            }
        }
    };

    // A synchronous spawn throw is contained into the same settled-killed shape as an asynchronous
    // spawn rejection, so execute() itself never throws for a spawn problem and result() carries
    // the failure uniformly.
    let mut handle = None;
    let mut spawn_error = None;
    if !preparation_timed_out {
        match executor
            .subprocess
            .spawn(executor.spawn_spec(&spec, argv, spawn_signal.clone()))
        {
            Ok(running) => handle = Some(running),
            Err(error) => spawn_error = Some(SharedError::new(error)),
        }
    }
    let (stdout, stderr, lifecycle, spawned) = match handle {
        Some(running) => {
            let (Some(stdout), Some(stderr)) = (
                running.collected.stdout.clone(),
                running.collected.stderr.clone(),
            ) else {
                return Err(Box::new(MissingCollectStream));
            };
            let lifecycle: Arc<dyn SubprocessLifecycle> = running.lifecycle.clone();
            (
                stdout,
                stderr,
                Some(lifecycle),
                Spawned::Handle(running.done),
            )
        }
        None => {
            let stdout: Arc<dyn SubprocessOutputReader> = Arc::new(EmptyReader);
            let stderr: Arc<dyn SubprocessOutputReader> = Arc::new(EmptyReader);
            let spawned = match spawn_error {
                Some(error) => Spawned::Rejected(error),
                None => Spawned::Settled(SubprocessOutcome {
                    exit_code: None,
                    signal: None,
                }),
            };
            (stdout, stderr, None, spawned)
        }
    };
    let running = lifecycle.is_some();

    // A provider rejection produces no process output, so the subprocess service has nothing to
    // buffer: once the provider rejected, its stage-neutral note is the whole stderr stream.
    let failure: Arc<Mutex<Option<ProviderFailure>>> = Arc::default();
    let observed = ShellObservedStreams {
        stdout: stdout.clone(),
        stderr: Arc::new(ObservedStderr {
            base: stderr.clone(),
            failure: failure.clone(),
        }),
    };
    let inner = Arc::new(Inner {
        state: Mutex::new(State {
            status: ShellProcessStatus::Running,
            exit_code: None,
            signal: None,
            sandbox: None,
            stdout_offset: 0,
            stderr_offset: 0,
            failure_reported: false,
        }),
        failure,
        stdout,
        stderr,
        observed,
        lifecycle,
        spawn_signal: spawn_signal.clone(),
        classify,
        timeout_ms: spec.timeout_ms,
        done: OnceLock::new(),
        result: Mutex::new(None),
        on_process_done: hooks.on_process_done,
    });
    let execution = BashExecution { inner };

    // The settlement continuation of `spawned.then(...)`. It runs on its own task because nothing
    // awaits `done` in the source either: the process outlives the executor fiber, owned by the
    // subprocess service (PORT_NOTES).
    // In the source the continuation is a microtask, so the synchronous `onStarted` always runs
    // first; the gate reproduces that order on a multi-threaded runtime. Dropping the sender
    // (a panicking hook) opens it too.
    let (started_gate, started) = tokio::sync::oneshot::channel::<()>();
    let settling = execution.clone();
    let task = tokio::spawn(async move {
        let _ = started.await;
        let outcome = match spawned {
            Spawned::Handle(done) => done.await,
            Spawned::Settled(outcome) => Ok(outcome),
            Spawned::Rejected(error) => Err(error),
        };
        match outcome {
            Ok(outcome) => settle_outcome(&settling, outcome),
            Err(error) => settle_rejection(&settling, error, running),
        }
        if let Some(deadline) = &mut deadline {
            deadline.dispose();
        }
    });
    let _ = execution.inner.done.set(shell_done(async move {
        let _ = task.await;
    }));

    if !preparation_timed_out && let Some(on_started) = hooks.on_started {
        on_started(&execution);
    }
    let _ = started_gate.send(());
    Ok(execution)
}

fn stderr_text(execution: &BashExecution) -> String {
    execution.inner.stderr.read_from(0).text
}

fn notify_done(
    execution: &BashExecution,
    stderr: &str,
    provider_rejected: bool,
    provider_error: Option<&SharedError>,
) {
    if let Some(hook) = &execution.inner.on_process_done {
        hook(execution, stderr, provider_rejected, provider_error);
    }
}

fn settle_outcome(execution: &BashExecution, outcome: SubprocessOutcome) {
    let aborted = execution
        .inner
        .spawn_signal
        .as_ref()
        .is_some_and(AbortSignal::aborted);
    {
        let mut state = lock(&execution.inner.state);
        // Any signal termination is killed, including a command signaling itself.
        if state.status == ShellProcessStatus::Running {
            state.status = if aborted || outcome.signal.is_some() {
                ShellProcessStatus::Killed
            } else {
                ShellProcessStatus::Completed
            };
        }
        state.exit_code = outcome.exit_code;
        state.signal = outcome.signal;
    }
    notify_done(execution, &stderr_text(execution), false, None);
}

fn settle_rejection(execution: &BashExecution, error: SharedError, running: bool) {
    // A live handle whose rejection follows this execution's own termination — kill() or the spawn
    // signal's abort — reports its terminal outcome: the result projection classifies it from the
    // deadline wiring; nothing is a provider failure. A synchronous spawn throw never produced a
    // handle and stays a failure.
    let own_termination = running
        && (execution.status() == ShellProcessStatus::Killed
            || execution
                .inner
                .spawn_signal
                .as_ref()
                .is_some_and(AbortSignal::aborted));
    if own_termination {
        execution.settle_status(ShellProcessStatus::Killed);
        notify_done(execution, &stderr_text(execution), false, None);
        return;
    }
    // Provider failures settle the handle as killed and surface on stderr for every reader.
    execution.settle_status(ShellProcessStatus::Killed);
    let note = format!(
        "subprocess failed before reporting an outcome: {}",
        js_error_string(&error)
    );
    *lock(&execution.inner.failure) = Some(ProviderFailure {
        error: error.clone(),
        note: note.clone(),
    });
    notify_done(execution, &note, true, Some(&error));
}
