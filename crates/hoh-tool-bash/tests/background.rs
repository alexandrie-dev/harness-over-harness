//! Ports of background-start.spec.ts and the pure cases of background.spec.ts and tools.spec.ts
//! (`processSources`, `ringDelta`, `processOutcome`, `processJob`).

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use hoh_error::BoxError;
use hoh_sandbox::{SandboxMode, escalation_hint_marker, sandbox_denial_marker};
use hoh_shell::{
    ShellExecution, ShellProcess, ShellProcessRead, ShellProcessStatus, ShellRunResult,
    ShellSandboxInfo,
    types::{ShellDone, ShellFuture, ShellObservedStreams, shell_done},
};
use hoh_subprocess::{SubprocessOutputRead, SubprocessOutputReader};
use hoh_tool_bash::background::{process_job, process_outcome, process_sources, ring_delta};
use hoh_tool_bash::shim::{JobChannel, JobChunk, JobOutcome, JobOutcomeStatus, JobSourceRead};
use tokio::sync::oneshot;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Poll `condition` until it holds (a stand-in for the source's microtask flushes).
async fn until(condition: impl Fn() -> bool) {
    for _ in 0..5_000 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    panic!("condition not reached before timeout");
}

struct TextReader {
    channel: &'static str,
    text: &'static str,
    reads: Arc<Mutex<Vec<(String, u64)>>>,
    lossy_spill: Option<&'static str>,
}

impl SubprocessOutputReader for TextReader {
    fn read_from(&self, from_byte: u64) -> SubprocessOutputRead {
        lock(&self.reads).push((self.channel.to_owned(), from_byte));
        let offset = (from_byte as usize).min(self.text.len());
        SubprocessOutputRead {
            text: self.text[offset..].to_owned(),
            next_offset: from_byte
                .max(self.text.len() as u64)
                .max(from_byte + self.text.len() as u64 - offset as u64),
            lossy: self.lossy_spill.is_some(),
            spill_path: self.lossy_spill.map(str::to_owned),
        }
    }
}

struct Facts {
    status: ShellProcessStatus,
    exit_code: Option<i64>,
    signal: Option<String>,
    sandbox: Option<ShellSandboxInfo>,
}

/// A `ShellProcess` fixture: the settled-handle literal of tools.spec.ts and the `processHandle()`
/// of background-start.spec.ts.
struct FakeProcess {
    facts: Mutex<Facts>,
    done: ShellDone,
    exit: Mutex<Option<oneshot::Sender<()>>>,
    observed: ShellObservedStreams,
    kill_calls: AtomicUsize,
    read_calls: AtomicUsize,
    kill_panics: bool,
}

impl FakeProcess {
    fn new(status: ShellProcessStatus, exit_code: Option<i64>, signal: Option<&str>) -> Arc<Self> {
        let (sender, receiver) = oneshot::channel::<()>();
        let reads = Arc::new(Mutex::new(Vec::new()));
        Arc::new(Self {
            facts: Mutex::new(Facts {
                status,
                exit_code,
                signal: signal.map(str::to_owned),
                sandbox: None,
            }),
            done: shell_done(async move {
                let _ = receiver.await;
            }),
            exit: Mutex::new(Some(sender)),
            observed: ShellObservedStreams {
                stdout: Arc::new(TextReader {
                    channel: "stdout",
                    text: "output",
                    reads: reads.clone(),
                    lossy_spill: None,
                }),
                stderr: Arc::new(TextReader {
                    channel: "stderr",
                    text: "",
                    reads,
                    lossy_spill: None,
                }),
            },
            kill_calls: AtomicUsize::new(0),
            read_calls: AtomicUsize::new(0),
            kill_panics: false,
        })
    }

    fn running() -> Arc<Self> {
        Self::new(ShellProcessStatus::Running, None, None)
    }

    fn exit(&self) {
        if let Some(sender) = lock(&self.exit).take() {
            let _ = sender.send(());
        }
    }

    fn kills(&self) -> usize {
        self.kill_calls.load(Ordering::SeqCst)
    }
}

impl Drop for FakeProcess {
    fn drop(&mut self) {
        self.exit();
    }
}

impl ShellProcess for FakeProcess {
    fn status(&self) -> ShellProcessStatus {
        lock(&self.facts).status
    }
    fn exit_code(&self) -> Option<i64> {
        lock(&self.facts).exit_code
    }
    fn signal(&self) -> Option<String> {
        lock(&self.facts).signal.clone()
    }
    fn done(&self) -> ShellDone {
        self.done.clone()
    }
    fn sandbox(&self) -> Option<ShellSandboxInfo> {
        lock(&self.facts).sandbox.clone()
    }
    fn read_output(&self) -> ShellProcessRead {
        self.read_calls.fetch_add(1, Ordering::SeqCst);
        ShellProcessRead {
            delta: "output".to_owned(),
            lossy: false,
            stdout_spill_path: None,
            stderr_spill_path: None,
        }
    }
    fn observed(&self) -> ShellObservedStreams {
        self.observed.clone()
    }
    fn kill(&self) -> bool {
        self.kill_calls.fetch_add(1, Ordering::SeqCst);
        if self.kill_panics {
            panic!("termination failed");
        }
        let mut facts = lock(&self.facts);
        if facts.status != ShellProcessStatus::Running {
            return false;
        }
        facts.status = ShellProcessStatus::Killed;
        facts.signal = Some("SIGTERM".to_owned());
        true
    }
}

impl ShellExecution for FakeProcess {
    fn result(&self) -> ShellFuture<'_, ShellRunResult> {
        Box::pin(async { Err::<ShellRunResult, BoxError>("the fixture has no result".into()) })
    }
}

fn completed(exit_code: Option<i64>) -> Arc<FakeProcess> {
    FakeProcess::new(ShellProcessStatus::Completed, exit_code, None)
}

fn with_sandbox(process: &FakeProcess, sandbox: ShellSandboxInfo) {
    lock(&process.facts).sandbox = Some(sandbox);
}

fn outcome(status: JobOutcomeStatus, detail: &str) -> JobOutcome {
    JobOutcome {
        status,
        detail: Some(detail.to_owned()),
        result: None,
    }
}

mod process_outcome_cases {
    use super::*;

    #[test]
    fn maps_a_signal_killed_process_to_killed_with_the_signal_detail() {
        let process = FakeProcess::new(ShellProcessStatus::Killed, Some(0), Some("SIGTERM"));
        assert_eq!(
            process_outcome(&*process, &[]),
            outcome(JobOutcomeStatus::Killed, "signal: SIGTERM")
        );
    }

    #[test]
    fn maps_a_killed_process_without_a_recorded_signal() {
        let process = FakeProcess::new(ShellProcessStatus::Killed, None, None);
        assert_eq!(
            process_outcome(&*process, &[]),
            outcome(JobOutcomeStatus::Killed, "killed before exit")
        );
    }

    #[test]
    fn maps_a_completed_process_to_its_exit_code() {
        assert_eq!(
            process_outcome(&*completed(Some(3)), &[]),
            outcome(JobOutcomeStatus::Completed, "exit code: 3")
        );
    }

    #[test]
    fn defensively_reads_a_null_exit_code_as_zero() {
        assert_eq!(
            process_outcome(&*completed(None), &[]),
            outcome(JobOutcomeStatus::Completed, "exit code: 0")
        );
    }

    #[test]
    fn appends_sandbox_facts_to_the_terminal_detail() {
        let denied = ShellSandboxInfo {
            mode: SandboxMode::ReadOnly,
            denied: true,
            enforcement: None,
            runner_failed: None,
        };
        let process = completed(Some(1));
        with_sandbox(&process, denied.clone());
        assert_eq!(
            process_outcome(&*process, &[SandboxMode::WorkspaceWrite])
                .detail
                .unwrap(),
            format!(
                "exit code: 1; {} {}",
                sandbox_denial_marker(SandboxMode::ReadOnly),
                escalation_hint_marker("command")
            )
        );
        assert_eq!(
            process_outcome(&*process, &[]).detail.unwrap(),
            format!(
                "exit code: 1; {}",
                sandbox_denial_marker(SandboxMode::ReadOnly)
            )
        );
        let runner = completed(Some(1));
        with_sandbox(
            &runner,
            ShellSandboxInfo {
                denied: false,
                runner_failed: Some(true),
                ..denied.clone()
            },
        );
        assert!(
            process_outcome(&*runner, &[])
                .detail
                .unwrap()
                .contains("the sandbox runner itself failed under read-only mode")
        );
        let clean = completed(Some(0));
        with_sandbox(
            &clean,
            ShellSandboxInfo {
                denied: false,
                ..denied
            },
        );
        assert_eq!(
            process_outcome(&*clean, &[]).detail.unwrap(),
            "exit code: 0"
        );
    }
}

mod process_sources_cases {
    use super::*;

    #[test]
    fn reads_nothing_while_the_process_is_not_spawned_yet() {
        let sources = process_sources(|| None);
        assert_eq!(sources[0].channel, Some(JobChannel::Stdout));
        assert_eq!(sources[1].channel, Some(JobChannel::Stderr));
        assert_eq!(
            (sources[0].read)(0),
            JobSourceRead {
                text: String::new(),
                next_offset: 0,
                lossy: false,
                spill_path: None
            }
        );
        assert_eq!(
            (sources[1].read)(3),
            JobSourceRead {
                text: String::new(),
                next_offset: 3,
                lossy: false,
                spill_path: None
            }
        );
    }

    struct ScriptedReader {
        channel: &'static str,
        text: &'static str,
        reads: Arc<Mutex<Vec<(String, u64)>>>,
    }

    impl SubprocessOutputReader for ScriptedReader {
        fn read_from(&self, from: u64) -> SubprocessOutputRead {
            lock(&self.reads).push((self.channel.to_owned(), from));
            SubprocessOutputRead {
                text: self.text.to_owned(),
                next_offset: from + self.text.len() as u64,
                lossy: false,
                spill_path: None,
            }
        }
    }

    #[tokio::test]
    async fn forwards_each_read_to_the_matching_stream_reader_at_its_own_offset_once_the_process_exists()
     {
        let reads = Arc::new(Mutex::new(Vec::new()));
        let process = FakeProcess::running();
        let observed = ShellObservedStreams {
            stdout: Arc::new(ScriptedReader {
                channel: "stdout",
                text: "out",
                reads: reads.clone(),
            }),
            stderr: Arc::new(ScriptedReader {
                channel: "stderr",
                text: "err!",
                reads: reads.clone(),
            }),
        };
        let live: Arc<dyn ShellExecution> = Arc::new(ObservedOnly(observed, process));
        let sources = process_sources(move || Some(live.clone()));
        assert_eq!(
            (sources[0].read)(2),
            JobSourceRead {
                text: "out".into(),
                next_offset: 5,
                lossy: false,
                spill_path: None
            }
        );
        assert_eq!(
            (sources[1].read)(7),
            JobSourceRead {
                text: "err!".into(),
                next_offset: 11,
                lossy: false,
                spill_path: None
            }
        );
        assert_eq!(
            *lock(&reads),
            vec![("stdout".to_owned(), 2), ("stderr".to_owned(), 7)]
        );
    }

    #[tokio::test]
    async fn passes_a_lossy_reads_spill_file_through_so_the_models_notice_can_name_it() {
        struct LossyStderr;
        impl SubprocessOutputReader for LossyStderr {
            fn read_from(&self, from: u64) -> SubprocessOutputRead {
                SubprocessOutputRead {
                    text: "tail".into(),
                    next_offset: from + 4,
                    lossy: true,
                    spill_path: Some("/spill/err.log".into()),
                }
            }
        }
        struct QuietStdout;
        impl SubprocessOutputReader for QuietStdout {
            fn read_from(&self, from: u64) -> SubprocessOutputRead {
                SubprocessOutputRead {
                    text: String::new(),
                    next_offset: from,
                    lossy: false,
                    spill_path: None,
                }
            }
        }
        let observed = ShellObservedStreams {
            stdout: Arc::new(QuietStdout),
            stderr: Arc::new(LossyStderr),
        };
        let live: Arc<dyn ShellExecution> =
            Arc::new(ObservedOnly(observed, FakeProcess::running()));
        let sources = process_sources(move || Some(live.clone()));
        assert_eq!(
            (sources[1].read)(0),
            JobSourceRead {
                text: "tail".into(),
                next_offset: 4,
                lossy: true,
                spill_path: Some("/spill/err.log".into()),
            }
        );
    }

    /// A process whose only meaningful member is `observed` (`Pick<ShellProcess, 'observed'>`).
    struct ObservedOnly(ShellObservedStreams, Arc<FakeProcess>);

    impl ShellProcess for ObservedOnly {
        fn status(&self) -> ShellProcessStatus {
            self.1.status()
        }
        fn exit_code(&self) -> Option<i64> {
            self.1.exit_code()
        }
        fn signal(&self) -> Option<String> {
            self.1.signal()
        }
        fn done(&self) -> ShellDone {
            self.1.done()
        }
        fn read_output(&self) -> ShellProcessRead {
            self.1.read_output()
        }
        fn observed(&self) -> ShellObservedStreams {
            self.0.clone()
        }
        fn kill(&self) -> bool {
            self.1.kill()
        }
    }
    impl ShellExecution for ObservedOnly {
        fn result(&self) -> ShellFuture<'_, ShellRunResult> {
            self.1.result()
        }
    }
}

#[test]
fn ring_delta_renders_stdout_chunks_in_order_and_every_stderr_chunk_in_one_trailing_section() {
    let chunk = |at, text: &str, channel| JobChunk {
        at,
        text: text.to_owned(),
        channel: Some(channel),
        gap_before: false,
    };
    assert_eq!(ring_delta(&[]), "");
    assert_eq!(
        ring_delta(&[
            chunk(0, "a\n", JobChannel::Stdout),
            chunk(2, "warn\n", JobChannel::Stderr),
            chunk(7, "b", JobChannel::Stdout),
        ]),
        "a\nb\n[stderr]\nwarn\n"
    );
    assert_eq!(
        ring_delta(&[chunk(0, "only\n", JobChannel::Stderr)]),
        "[stderr]\nonly\n"
    );
}

mod process_job_cases {
    use super::*;
    use hoh_timeout::AbortSignal;

    type Prepared = oneshot::Receiver<Arc<FakeProcess>>;

    fn start_after(
        prepared: Prepared,
        published: Arc<Mutex<Option<Arc<dyn ShellExecution>>>>,
    ) -> hoh_tool_bash::background::StartProcess {
        Box::new(move |_signal| {
            Box::pin(async move {
                let process: Arc<dyn ShellExecution> = prepared
                    .await
                    .map_err(|error| Box::new(error) as BoxError)?;
                *lock(&published) = Some(process.clone());
                Ok(process)
            })
        })
    }

    #[tokio::test]
    async fn keeps_observation_empty_during_preparation_and_reads_published_output_without_consuming_it()
     {
        let child = FakeProcess::running();
        let (publish, prepared) = oneshot::channel();
        let published: Arc<Mutex<Option<Arc<dyn ShellExecution>>>> = Arc::default();
        let sources = process_sources({
            let published = published.clone();
            move || lock(&published).clone()
        });
        let outcome_calls = Arc::new(AtomicUsize::new(0));
        let hooks = process_job(start_after(prepared, published.clone()), {
            let calls = outcome_calls.clone();
            move |process| {
                calls.fetch_add(1, Ordering::SeqCst);
                process_outcome(process, &[])
            }
        });
        assert_eq!((sources[0].read)(0).text, "");
        assert_eq!(outcome_calls.load(Ordering::SeqCst), 0);
        publish.send(child.clone()).map_err(|_| ()).unwrap();
        until(|| lock(&published).is_some()).await;
        assert_eq!((sources[0].read)(0).text, "output");
        assert_eq!((sources[0].read)(0).text, "output");
        assert_eq!(child.read_calls.load(Ordering::SeqCst), 0);
        {
            let mut facts = lock(&child.facts);
            facts.status = ShellProcessStatus::Completed;
            facts.exit_code = Some(5);
        }
        child.exit();
        assert_eq!(
            hooks.hooks.done.await,
            outcome(JobOutcomeStatus::Completed, "exit code: 5")
        );
    }

    #[tokio::test]
    async fn aborts_pending_preparation_through_the_job_owned_signal() {
        let received: Arc<Mutex<Option<AbortSignal>>> = Arc::default();
        let hooks = process_job(
            Box::new({
                let received = received.clone();
                move |signal: AbortSignal| {
                    *lock(&received) = Some(signal.clone());
                    Box::pin(async move {
                        let reason = signal.cancelled().await;
                        Err::<Arc<dyn ShellExecution>, BoxError>(reason.to_string().into())
                    })
                }
            }),
            |process| process_outcome(process, &[]),
        );
        let signal = lock(&received).clone().expect("the starter ran");
        assert!(!signal.aborted());
        (hooks.hooks.cancel)(Some("cancel pending confinement"));
        (hooks.hooks.cancel)(Some("later cancellation"));
        assert_eq!(
            signal.reason().unwrap().to_string(),
            "cancel pending confinement"
        );
        assert_eq!(
            hooks.hooks.done.await,
            outcome(JobOutcomeStatus::Killed, "cancel pending confinement")
        );
    }

    #[tokio::test]
    async fn kills_a_process_that_materializes_after_cancellation_and_awaits_its_exit() {
        let child = FakeProcess::running();
        let (publish, prepared) = oneshot::channel();
        let hooks = process_job(start_after(prepared, Arc::default()), |process| {
            process_outcome(process, &[])
        });
        let settled = Arc::new(AtomicUsize::new(0));
        let done = tokio::spawn({
            let settled = settled.clone();
            async move {
                let outcome = hooks.hooks.done.await;
                settled.fetch_add(1, Ordering::SeqCst);
                outcome
            }
        });
        (hooks.hooks.cancel)(Some("cancel before process publication"));
        publish.send(child.clone()).map_err(|_| ()).unwrap();
        until(|| child.kills() == 1).await;
        assert_eq!(child.kills(), 1);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(settled.load(Ordering::SeqCst), 0);
        child.exit();
        assert_eq!(
            done.await.unwrap(),
            outcome(JobOutcomeStatus::Killed, "signal: SIGTERM")
        );
    }

    #[tokio::test]
    async fn cancels_a_published_process_once_and_keeps_done_pending_until_exit() {
        let child = FakeProcess::running();
        let hooks = process_job(
            Box::new({
                let child = child.clone();
                move |_signal| Box::pin(async move { Ok(child as Arc<dyn ShellExecution>) })
            }),
            |process| process_outcome(process, &[]),
        );
        let settled = Arc::new(AtomicUsize::new(0));
        let done = tokio::spawn({
            let settled = settled.clone();
            async move {
                let outcome = hooks.hooks.done.await;
                settled.fetch_add(1, Ordering::SeqCst);
                outcome
            }
        });
        // The starter has published the process once it has run.
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        (hooks.hooks.cancel)(Some("stop"));
        (hooks.hooks.cancel)(Some("stop again"));
        until(|| child.kills() == 1).await;
        assert_eq!(child.kills(), 1);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(settled.load(Ordering::SeqCst), 0);
        child.exit();
        assert_eq!(done.await.unwrap().status, JobOutcomeStatus::Killed);
    }

    #[tokio::test]
    async fn joins_a_late_process_even_when_its_termination_request_throws() {
        let mut fixture = FakeProcess::running();
        Arc::get_mut(&mut fixture).unwrap().kill_panics = true;
        let child = fixture;
        let (publish, prepared) = oneshot::channel();
        let hooks = process_job(start_after(prepared, Arc::default()), |process| {
            process_outcome(process, &[])
        });
        let settled = Arc::new(AtomicUsize::new(0));
        let done = tokio::spawn({
            let settled = settled.clone();
            async move {
                let outcome = hooks.hooks.done.await;
                settled.fetch_add(1, Ordering::SeqCst);
                outcome
            }
        });
        (hooks.hooks.cancel)(Some("stop before publication"));
        publish.send(child.clone()).map_err(|_| ()).unwrap();
        until(|| child.kills() == 1).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(settled.load(Ordering::SeqCst), 0);
        child.exit();
        assert_eq!(
            done.await.unwrap(),
            outcome(JobOutcomeStatus::Failed, "termination failed")
        );
    }

    #[tokio::test]
    async fn reports_a_startup_failure_through_job_completion_without_publishing_output() {
        let rendered = Arc::new(AtomicUsize::new(0));
        let hooks = process_job(
            Box::new(|_signal| {
                Box::pin(async {
                    Err::<Arc<dyn ShellExecution>, BoxError>("remote sandbox unavailable".into())
                })
            }),
            {
                let rendered = rendered.clone();
                move |process| {
                    rendered.fetch_add(1, Ordering::SeqCst);
                    process_outcome(process, &[])
                }
            },
        );
        assert_eq!(
            hooks.hooks.done.await,
            outcome(JobOutcomeStatus::Failed, "remote sandbox unavailable")
        );
        assert_eq!(rendered.load(Ordering::SeqCst), 0);
        assert!(hooks.failure.get().is_some());
    }

    #[tokio::test]
    async fn reports_a_primitive_upstream_abort_reason_as_a_failed_startup() {
        let upstream = hoh_timeout::AbortController::new();
        upstream.abort_with("upstream preparation stopped");
        let signal = upstream.signal();
        let hooks = process_job(
            Box::new(move |_signal| {
                Box::pin(async move {
                    signal
                        .throw_if_aborted()
                        .map_err(|reason| Box::new(reason) as BoxError)?;
                    Err::<Arc<dyn ShellExecution>, BoxError>("unreachable".into())
                })
            }),
            |process| process_outcome(process, &[]),
        );
        assert_eq!(
            hooks.hooks.done.await,
            outcome(JobOutcomeStatus::Failed, "upstream preparation stopped")
        );
    }
}
