//! Generic-job adaptation for bash process handles: the terminal outcome the registry records, the
//! pull sources it pumps, and the ring read a foreground call renders when it stops waiting.

use std::sync::{Arc, Mutex, MutexGuard};

use hoh_error::{BoxError, SharedError};
use hoh_sandbox::{SandboxMode, escalation_hint_marker, sandbox_denial_marker};
use hoh_shell::{ShellExecution, ShellProcess, ShellProcessStatus, ShellSandboxInfo};
use hoh_timeout::{AbortController, AbortSignal};

use crate::render::runner_failed_notice;
use crate::shim::{
    BoxFuture, JobChannel, JobChunk, JobHooks, JobOutcome, JobOutcomeStatus, JobOutputSource,
    JobSourceRead,
};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Sandbox facts worth the terminal detail: a runner that never ran the command, or a denial (with
/// the escalation hint this composition offers). Oldest first.
fn sandbox_notes(
    sandbox: Option<&ShellSandboxInfo>,
    escalation_modes: &[SandboxMode],
) -> Vec<String> {
    let Some(sandbox) = sandbox else {
        return Vec::new();
    };
    if sandbox.runner_failed == Some(true) {
        return vec![runner_failed_notice(sandbox.mode)];
    }
    if sandbox.denied {
        let mut notes = vec![sandbox_denial_marker(sandbox.mode)];
        if !escalation_modes.is_empty() {
            notes.push(escalation_hint_marker("command"));
        }
        return notes;
    }
    Vec::new()
}

/// Map a settled background process onto the generic job-outcome vocabulary: `killed` stays
/// `killed` (detail: the signal when one is known), everything else is `completed` with the exit
/// code as detail. A nonzero command exit is reported, not failed, exactly like the foreground
/// rendering. Sandbox facts join the detail, since a job's terminal reason is the one line every
/// reader — the model's status line, the roster row — shows.
pub fn process_outcome(process: &dyn ShellProcess, escalation_modes: &[SandboxMode]) -> JobOutcome {
    let (status, detail) = if process.status() == ShellProcessStatus::Killed {
        (
            JobOutcomeStatus::Killed,
            match process.signal() {
                Some(signal) => format!("signal: {signal}"),
                None => "killed before exit".to_owned(),
            },
        )
    } else {
        (
            JobOutcomeStatus::Completed,
            format!("exit code: {}", process.exit_code().unwrap_or(0)),
        )
    };
    let notes = sandbox_notes(process.sandbox().as_ref(), escalation_modes);
    let detail = if notes.is_empty() {
        detail
    } else {
        format!("{detail}; {}", notes.join(" "))
    };
    JobOutcome {
        status,
        detail: Some(detail),
        result: None,
    }
}

/// The process's non-consuming stream readers as registry pull sources. They bind lazily because the
/// process is spawned inside the starter, after the registry admitted the job; a read before the
/// spawn yields nothing, and the pump keeps the model's consuming cursor untouched. A rejected
/// spawn's stderr reader carries the provider's `subprocess failed before reporting an outcome: …`
/// note. One source per stream, stdout first.
pub fn process_sources(
    process: impl Fn() -> Option<Arc<dyn ShellExecution>> + Clone + Send + Sync + 'static,
) -> Vec<JobOutputSource> {
    [
        (JobChannel::Stdout, "stdout"),
        (JobChannel::Stderr, "stderr"),
    ]
    .into_iter()
    .map(|(channel, name)| {
        let process = process.clone();
        JobOutputSource {
            channel: Some(channel),
            read: Box::new(move |from_byte| match process() {
                None => JobSourceRead {
                    text: String::new(),
                    next_offset: from_byte,
                    lossy: false,
                    spill_path: None,
                },
                Some(live) => {
                    let observed = live.observed();
                    let reader = if name == "stdout" {
                        observed.stdout
                    } else {
                        observed.stderr
                    };
                    let read = reader.read_from(from_byte);
                    JobSourceRead {
                        text: read.text,
                        next_offset: read.next_offset,
                        lossy: read.lossy,
                        spill_path: read.spill_path,
                    }
                }
            }),
        }
    })
    .collect()
}

/// The ring chunks of one consuming registry read as the shell tools render a process read: stdout
/// chunks in order, then every stderr chunk in one `[stderr]` section, so the output a foreground
/// call hands over when it stops waiting reads exactly like the `job_output` reads that follow it.
pub fn ring_delta(chunks: &[JobChunk]) -> String {
    let out: String = chunks
        .iter()
        .filter(|chunk| chunk.channel != Some(JobChannel::Stderr))
        .map(|chunk| chunk.text.as_str())
        .collect();
    let err: String = chunks
        .iter()
        .filter(|chunk| chunk.channel == Some(JobChannel::Stderr))
        .map(|chunk| chunk.text.as_str())
        .collect();
    let separator = if !out.is_empty() && !out.ends_with('\n') {
        "\n"
    } else {
        ""
    };
    if err.is_empty() {
        out
    } else {
        format!("{out}{separator}[stderr]\n{err}")
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_owned())
        })
        .unwrap_or_else(|| "termination failed".to_owned())
}

/// The error that ended a `failed` job, kept with its own class and code: empty while the job runs
/// and for a completed or killed job.
#[derive(Clone, Default)]
pub struct ProcessFailure(Arc<Mutex<Option<SharedError>>>);

impl ProcessFailure {
    /// The original failure, any thrown value preserved.
    pub fn get(&self) -> Option<SharedError> {
        lock(&self.0).clone()
    }
}

/// Starts the process with job-owned cancellation.
pub type StartProcess = Box<
    dyn FnOnce(AbortSignal) -> BoxFuture<'static, Result<Arc<dyn ShellExecution>, BoxError>> + Send,
>;

/// Job hooks plus the original error of a job that failed before its process produced an outcome.
pub struct ProcessJob {
    pub hooks: JobHooks,
    pub failure: ProcessFailure,
}

/// Adapt asynchronous shell preparation after job admission without exposing a partial process.
/// `start` starts the process with job-owned cancellation; `outcome` projects the settled process
/// into the job outcome. The hooks' completion includes preparation and process settlement.
///
/// The preparation runs on its own task from this call on, as the source's asynchronous function
/// does: the registry owns the work through `cancel` and `done` (PORT_NOTES).
pub fn process_job(
    start: StartProcess,
    outcome: impl FnOnce(&dyn ShellProcess) -> JobOutcome + Send + 'static,
) -> ProcessJob {
    let controller = AbortController::new();
    let process: Arc<Mutex<Option<Arc<dyn ShellExecution>>>> = Arc::default();
    let failure = ProcessFailure::default();

    let task = {
        let signal = controller.signal();
        let process = process.clone();
        let failure = failure.clone();
        // The source's async function runs up to its first `await` inside the call: the starter is
        // invoked synchronously, and only its awaiting happens on the task.
        let starting = start(signal.clone());
        tokio::spawn(async move {
            match starting.await {
                Ok(started) => {
                    *lock(&process) = Some(started.clone());
                    // `try { if (aborted) process.kill() } finally { await process.done }`: a
                    // throwing termination request is rethrown after the process is joined.
                    let kill_failure = if signal.aborted() {
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| started.kill()))
                            .err()
                    } else {
                        None
                    };
                    started.done().await;
                    match kill_failure {
                        None => outcome(&*started),
                        Some(payload) => {
                            let message = panic_message(payload.as_ref());
                            *lock(&failure.0) = Some(SharedError::new(message.clone()));
                            JobOutcome {
                                status: JobOutcomeStatus::Failed,
                                detail: Some(message),
                                result: None,
                            }
                        }
                    }
                }
                Err(error) => {
                    let killed = signal.aborted() && lock(&process).is_none();
                    let detail = error.to_string();
                    if !killed {
                        *lock(&failure.0) = Some(SharedError::new(error));
                    }
                    JobOutcome {
                        status: if killed {
                            JobOutcomeStatus::Killed
                        } else {
                            JobOutcomeStatus::Failed
                        },
                        detail: Some(detail),
                        result: None,
                    }
                }
            }
        })
    };
    let done: BoxFuture<'static, JobOutcome> = Box::pin(async move {
        task.await.unwrap_or_else(|error| JobOutcome {
            status: JobOutcomeStatus::Failed,
            detail: Some(error.to_string()),
            result: None,
        })
    });
    let cancel = {
        let signal = controller.signal();
        Box::new(move |reason: Option<&str>| {
            if signal.aborted() {
                return;
            }
            match reason {
                Some(reason) => controller.abort_with(reason.to_owned()),
                None => controller.abort(),
            }
            let running = lock(&process).clone();
            if let Some(running) = running {
                running.kill();
            }
        })
    };
    ProcessJob {
        hooks: JobHooks { cancel, done },
        failure,
    }
}
