//! Process-local provider for the background-job capability seam (`ctx.jobs`). It keeps every
//! job — lifecycle state, the bounded output ring, and the model cursor — in memory and hands out
//! fresh projections and chunk copies, never live state.
//!
//! Port of `@deepseek-ai/dsh-jobs-local` (`packages/jobs/jobs-local`).

pub mod events;
pub mod pump;
pub mod ring;

use events::{JobEventHub, JobLayer, kernel_error};
use hoh_agent::{Agent, Agents};
use hoh_error::BoxError;
use hoh_jobs::{
    JobAppendOptions, JobCancel, JobError, JobErrorCode, JobEvent, JobEventFilter,
    JobEventListener, JobEvents, JobFuture, JobHandle, JobId, JobOutcome, JobOutcomeStatus,
    JobOutputRead, JobOutputSource, JobOutputView, JobRead, JobRegisteredData, JobRegistryApi,
    JobSettleCause, JobSettledData, JobSourceRead, JobSpec, JobStatus, JobToolCall, JobView,
    KillOutcome,
};
use hoh_kernel::{Cfg, Context, Disposer, KernelError, Plugin, Undo};
use hoh_session::SessionId;
use hoh_timeout::{AbortError, AbortSignal};
use pump::{PumpSink, json_number, start_pump};
use ring::OutputRing;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        Arc, Mutex, MutexGuard, OnceLock, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{oneshot, watch};

/// Timeout code that distinguishes a bounded wait from caller cancellation.
pub const TASK_WAIT_TIMEOUT: &str = "TASK_WAIT_TIMEOUT";

const DEFAULT_MAX_CONCURRENT_JOBS_PER_OWNER: u64 = 10;
const DEFAULT_RETAIN_BYTES: u64 = 256 * 1024;
const DEFAULT_SETTLED_RETAIN_BYTES: u64 = 16 * 1024;
const DEFAULT_PUMP_POLL_MS: u64 = 150;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

const fn default_max_concurrent() -> u64 {
    DEFAULT_MAX_CONCURRENT_JOBS_PER_OWNER
}
const fn default_retain() -> u64 {
    DEFAULT_RETAIN_BYTES
}
const fn default_settled_retain() -> u64 {
    DEFAULT_SETTLED_RETAIN_BYTES
}
const fn default_poll() -> u64 {
    DEFAULT_PUMP_POLL_MS
}

/// Configuration for the process-local job registry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Maximum `running` plus `stopping` jobs per exact owner or in the shared unowned bucket.
    #[serde(default = "default_max_concurrent")]
    #[schemars(range(min = 1, max = 9_007_199_254_740_991u64))]
    pub max_concurrent_jobs_per_owner: u64,
    /// Live ring retention per job in UTF-8 bytes.
    #[serde(default = "default_retain")]
    #[schemars(range(min = 1, max = 9_007_199_254_740_991u64))]
    pub retain_bytes: u64,
    /// Ring retention kept after a job settles, in UTF-8 bytes.
    #[serde(default = "default_settled_retain")]
    #[schemars(range(min = 1, max = 9_007_199_254_740_991u64))]
    pub settled_retain_bytes: u64,
    /// Poll interval for a job's pull sources, in milliseconds.
    #[serde(default = "default_poll")]
    #[schemars(range(min = 1, max = 9_007_199_254_740_991u64))]
    pub pump_poll_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_concurrent_jobs_per_owner: default_max_concurrent(),
            retain_bytes: default_retain(),
            settled_retain_bytes: default_settled_retain(),
            pump_poll_ms: default_poll(),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

fn plain(message: impl Into<String>) -> BoxError {
    std::io::Error::other(message.into()).into()
}

fn job_error(message: impl Into<String>, code: JobErrorCode) -> BoxError {
    JobError::new(message, code).into()
}

fn status_text(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Running => "running",
        JobStatus::Stopping => "stopping",
        JobStatus::Completed => "completed",
        JobStatus::Killed => "killed",
        JobStatus::Failed => "failed",
    }
}

/// One task parked in `wait`, resolved by settlement (or removed by its own timeout/abort).
struct Waiter {
    id: u64,
    signal: Option<AbortSignal>,
    release: oneshot::Sender<bool>,
}

/// The registry's mutable per-job record (never handed out; see [`LocalJobRegistry::view`]).
struct JobData {
    status: JobStatus,
    ring: OutputRing,
    /// The model's consuming cursor; `read_at` never moves it.
    model_cursor: u64,
    /// Whether the first post-settlement read already handed out `result`.
    result_delivered: bool,
    /// Whether the registration commit happened; false exactly during the starter call.
    committed: bool,
    /// Whether `registered` has been announced. Output and progress events wait for it, so a
    /// job's first event is always `registered` even when its producer writes from another thread.
    announced: bool,
    progress: Option<String>,
    detail: Option<String>,
    result: Option<String>,
    finished_at: Option<i64>,
    /// Reason recorded by `kill`, merged into a `killed` settlement's detail.
    kill_reason: Option<String>,
    /// Set once a kill or teardown cancel ran; settlement reports it as the cause.
    settle_cause: Option<JobSettleCause>,
    waiters: Vec<Waiter>,
    /// The spill file each pull source reported on its latest read, by source index.
    spill_paths: Vec<Option<String>>,
}

struct Job {
    id: JobId,
    tool_call: Option<JobToolCall>,
    kind: String,
    label: String,
    output_limit_bytes: Option<u64>,
    /// Exact lifecycle owner; session-id authorization is derived from it.
    owner: Option<Arc<dyn Agent>>,
    started_at: i64,
    cancel: OnceLock<Arc<JobCancel>>,
    data: Mutex<JobData>,
    /// Becomes true with the first effective settlement.
    settled: watch::Sender<bool>,
    /// Becomes true when the producer's `done` resolved or rejected.
    producer_done: watch::Sender<bool>,
}

type Uuid = Arc<dyn Fn() -> String + Send + Sync>;

#[derive(Default)]
struct State {
    store: indexmap::IndexMap<JobId, Arc<Job>>,
    /// Owner agents with attached scope cleanup, with the exact disposer.
    owner_cleanups: Vec<(Arc<dyn Agent>, Disposer)>,
}

/// Slots claimed by `start` calls whose starter has not registered a job yet, by owner identity.
/// The limit check, the claim, and the registration all happen under this lock, so parallel
/// starts of one owner cannot pass the check together.
#[derive(Default)]
struct Admission {
    reserved: std::collections::HashMap<usize, u64>,
}

fn owner_key(owner: &Option<Arc<dyn Agent>>) -> usize {
    owner
        .as_ref()
        .map_or(0, |agent| Arc::as_ptr(agent).cast::<()>() as usize)
}

/// A claimed admission slot; released on drop unless the registration took it over.
struct Reservation {
    inner: Arc<Inner>,
    key: usize,
    held: bool,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.held {
            release_slot(&mut lock(&self.inner.admission), self.key);
        }
    }
}

fn release_slot(admission: &mut Admission, key: usize) {
    if let Some(count) = admission.reserved.get_mut(&key) {
        *count -= 1;
        if *count == 0 {
            admission.reserved.remove(&key);
        }
    }
}

struct Inner {
    ctx: Context,
    config: Config,
    state: Mutex<State>,
    admission: Mutex<Admission>,
    hub: JobEventHub,
    uuid: Uuid,
    wait_ids: AtomicU64,
}

fn same_agent(left: &Arc<dyn Agent>, right: &Arc<dyn Agent>) -> bool {
    std::ptr::eq(
        Arc::as_ptr(left).cast::<()>(),
        Arc::as_ptr(right).cast::<()>(),
    )
}

fn same_owner(left: &Option<Arc<dyn Agent>>, right: &Option<Arc<dyn Agent>>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => same_agent(left, right),
        _ => false,
    }
}

/// The in-memory `jobs` registry. See the service definition in `hoh-jobs` for the ownership,
/// isolation, and lifecycle semantics this implementation honors.
#[derive(Clone)]
pub struct LocalJobRegistry {
    inner: Arc<Inner>,
}

impl LocalJobRegistry {
    /// Construct a registry bound to `ctx` (its logger and agent registry lookup). Normal
    /// compositions mount [`LocalJobRegistryPlugin`] instead.
    pub fn new(ctx: Context, config: Config) -> Result<Self, BoxError> {
        Self::with_uuid_source(
            ctx,
            config,
            Arc::new(|| hoh_util_crypto::random_uuid().as_str().to_owned()),
        )
    }

    /// Like [`new`](Self::new) with an injected UUID source, so a test can force an id collision
    /// (the source mocks `node:crypto`).
    #[doc(hidden)]
    pub fn with_uuid_source(ctx: Context, config: Config, uuid: Uuid) -> Result<Self, BoxError> {
        let layers = hoh_scope::ScopedLayers::new(|_| Ok(JobLayer::default()), || Ok(()))?;
        Ok(Self {
            inner: Arc::new(Inner {
                hub: JobEventHub::new(layers, ctx.clone()),
                ctx,
                config,
                state: Mutex::new(State::default()),
                admission: Mutex::new(Admission::default()),
                uuid,
                wait_ids: AtomicU64::new(1),
            }),
        })
    }

    fn warn(&self, message: impl std::fmt::Display) {
        self.inner.ctx.logger().warn(message);
    }

    fn emit(&self, event: &JobEvent, owner: Option<&Arc<dyn Agent>>) {
        self.inner.hub.emit(event, owner);
    }

    fn job_data<'a>(job: &'a Job) -> MutexGuard<'a, JobData> {
        lock(&job.data)
    }

    /// Resolve a spec's owner session to its live Agent. An owned registration needs the agent
    /// registry, and the session must currently have a live instance: that instance's disposal is
    /// what cancels and drops the job.
    fn resolve_owner(
        &self,
        session: Option<&SessionId>,
    ) -> Result<Option<Arc<dyn Agent>>, BoxError> {
        let Some(session) = session else {
            return Ok(None);
        };
        let Some(agents) = self.inner.ctx.get::<Agents>() else {
            return Err(job_error(
                "background job ownership requires the agent registry (load @deepseek-ai/dsh-agent)",
                JobErrorCode::OwnerRegistryUnavailable,
            ));
        };
        match agents.get(session) {
            Some(owner) => Ok(Some(owner)),
            None => Err(job_error(
                format!(
                    "session \"{session}\" has no live agent (background job owner must be live)"
                ),
                JobErrorCode::OwnerNotLive,
            )),
        }
    }

    /// Whether an attached job controller can collect and stop work owned by `owner`. The global
    /// layer holds every controller attached from an unscoped context and therefore serves every
    /// owner; a scoped controller serves exactly the agents composed under it.
    fn serves_owner(&self, owner: Option<&Arc<dyn Agent>>) -> bool {
        let layers = self.inner.hub.layers();
        if !layers.global.controllers.is_empty() {
            return true;
        }
        let scope = owner.and_then(|agent| hoh_scope::scope_of(agent.ctx()));
        layers
            .chain_layers(scope.as_ref())
            .iter()
            .any(|layer| !layer.controllers.is_empty())
    }

    /// Count authoritative active records for one exact owner or the shared unowned bucket.
    fn active_job_count(&self, owner: &Option<Arc<dyn Agent>>) -> u64 {
        let jobs: Vec<Arc<Job>> = lock(&self.inner.state).store.values().cloned().collect();
        jobs.iter()
            .filter(|job| same_owner(&job.owner, owner) && Self::job_data(job).status.is_live())
            .count() as u64
    }

    /// Look up a job and enforce caller access.
    fn expect(&self, id: &JobId, caller: Option<&SessionId>) -> Result<Arc<Job>, BoxError> {
        let job = lock(&self.inner.state).store.get(id).cloned();
        let Some(job) = job else {
            return Err(job_error(
                format!("unknown job {id}"),
                JobErrorCode::NotFound,
            ));
        };
        Self::assert_access(&job, caller)?;
        Ok(job)
    }

    /// The isolation fence: a job with an owner is reachable only by callers whose session id
    /// matches; an unowned job is open, and a caller-less view can never match an owned one.
    fn assert_access(job: &Job, caller: Option<&SessionId>) -> Result<(), BoxError> {
        if let Some(owner) = &job.owner
            && Some(owner.id()) != caller
        {
            return Err(job_error(
                format!("job {} belongs to another session", job.id),
                JobErrorCode::ForeignSession,
            ));
        }
        Ok(())
    }

    /// Project a fresh read-only view from the mutable record.
    fn view(job: &Job) -> JobView {
        let data = Self::job_data(job);
        Self::view_of(job, &data)
    }

    fn view_of(job: &Job, data: &JobData) -> JobView {
        let mut spill_paths: Vec<String> = Vec::new();
        for path in data.spill_paths.iter().flatten() {
            if !spill_paths.contains(path) {
                spill_paths.push(path.clone());
            }
        }
        JobView {
            id: job.id.clone(),
            kind: job.kind.clone(),
            label: job.label.clone(),
            owner: job.owner.as_ref().map(|owner| owner.id().clone()),
            output_limit_bytes: job.output_limit_bytes,
            status: data.status,
            progress: data.progress.clone(),
            detail: data.detail.clone(),
            started_at: job.started_at,
            finished_at: data.finished_at,
            output: JobOutputView {
                total: data.ring.total,
                earliest: data.ring.earliest,
                spill_paths: (!spill_paths.is_empty()).then_some(spill_paths),
            },
        }
    }

    /// Publish committed facts before live subscribers or completion waiters react.
    fn record_session_event(&self, job: &Job, settlement: Option<Settlement>) {
        let Some(owner) = &job.owner else {
            return;
        };
        let registered = JobRegisteredData {
            job_id: job.id.clone(),
            owner: owner.id().clone(),
            kind: job.kind.clone(),
            label: job.label.clone(),
            started_at: job.started_at,
            tool_call: job.tool_call.clone(),
        };
        let (name, value) = match settlement {
            None => ("job/registered", serde_json::to_value(registered)),
            Some(settlement) => (
                "job/settled",
                serde_json::to_value(JobSettledData {
                    job_id: registered.job_id,
                    owner: registered.owner,
                    kind: registered.kind,
                    label: registered.label,
                    started_at: registered.started_at,
                    tool_call: registered.tool_call,
                    finished_at: settlement.finished_at,
                    status: settlement.status,
                    cause: settlement.cause,
                    producer_reported: settlement.producer_reported,
                    detail: settlement.detail,
                }),
            ),
        };
        let appended = value
            .map_err(BoxError::from)
            .and_then(|data| owner.session().append(name, data, None));
        if let Err(error) = appended {
            // Recording cannot orphan already-started work or prevent waiter release.
            self.warn(format_args!(
                "jobs: failed to record lifecycle for {}: Error: {error}",
                job.id
            ));
        }
    }

    /// Append one chunk to the ring. A producer chunk against a settled job is logged and
    /// dropped; the registry's own pump drains silently after settlement. A chunk staged inside
    /// the starter call is retained and signals no observer: the registration commit publishes it.
    fn append_ring(&self, job: &Arc<Job>, text: &str, options: JobAppendOptions, producer: bool) {
        let event = {
            let mut data = Self::job_data(job);
            if data.committed && data.status.is_terminal() {
                drop(data);
                if producer {
                    self.warn(format_args!(
                        "jobs: append to settled job {} dropped",
                        job.id
                    ));
                }
                return;
            }
            if !data
                .ring
                .append(text, options, self.inner.config.retain_bytes)
            {
                return;
            }
            data.announced.then(|| JobEvent::Output {
                id: job.id.clone(),
                owner: job.owner.as_ref().map(|owner| owner.id().clone()),
                total: data.ring.total,
            })
        };
        if let Some(event) = event {
            self.emit(&event, job.owner.as_ref());
        }
    }

    /// Replace the live progress line through a producer face; a write against a settled job is
    /// logged and dropped. A write staged inside the starter call seeds the registered projection
    /// and signals no observer.
    fn update_progress(&self, job: &Arc<Job>, line: &str) {
        let event = {
            let mut data = Self::job_data(job);
            if data.committed && data.status.is_terminal() {
                drop(data);
                self.warn(format_args!(
                    "jobs: progress update on settled job {} dropped",
                    job.id
                ));
                return;
            }
            data.progress = Some(line.to_owned());
            data.announced.then(|| JobEvent::Progress {
                job: Self::view_of(job, &data),
            })
        };
        if let Some(event) = event {
            self.emit(&event, job.owner.as_ref());
        }
    }

    /// Contain a failing pull source: the first failure is logged, and the source reads as
    /// exhausted from then on, so the job runs to its own settlement.
    fn guard_source(&self, job: &Arc<Job>, source: JobOutputSource) -> JobOutputSource {
        let failed = AtomicBool::new(false);
        let registry = self.clone();
        let id = job.id.clone();
        let read = source.read;
        JobOutputSource {
            channel: source.channel,
            read: Box::new(move |from| {
                if failed.load(Ordering::SeqCst) {
                    return Ok(JobSourceRead {
                        text: String::new(),
                        next_offset: from,
                        lossy: false,
                        spill_path: None,
                    });
                }
                match read(from) {
                    Ok(read) => Ok(read),
                    Err(error) => {
                        failed.store(true, Ordering::SeqCst);
                        registry.warn(format_args!(
                            "jobs: output source for {id} failed; its stream stops here: Error: {error}"
                        ));
                        Ok(JobSourceRead {
                            text: String::new(),
                            next_offset: from,
                            lossy: false,
                            spill_path: None,
                        })
                    }
                }
            }),
        }
    }

    /// Record the first terminal outcome, release waiters, then announce the settlement.
    /// First-wins preserves a teardown force-failure against late producer settlement. The
    /// settled event follows every released waiter and reports whether it released one: a
    /// timed-out or aborted wait has already left the set, so only a wait still owed the
    /// projection counts.
    fn settle(
        &self,
        job: &Arc<Job>,
        outcome: JobOutcome,
        cause: JobSettleCause,
        producer_reported: bool,
    ) {
        let (finished_at, detail) = {
            let mut data = Self::job_data(job);
            if data.status.is_terminal() {
                return;
            }
            data.status = match outcome.status {
                JobOutcomeStatus::Completed => JobStatus::Completed,
                JobOutcomeStatus::Killed => JobStatus::Killed,
                JobOutcomeStatus::Failed => JobStatus::Failed,
            };
            // A killed settlement carries the recorded kill reason in its detail: producer facts
            // first. A job that outran its kill request keeps the producer detail alone.
            if outcome.status == JobOutcomeStatus::Killed && data.kill_reason.is_some() {
                let reason = data.kill_reason.clone().unwrap_or_default();
                data.detail = Some(match &outcome.detail {
                    Some(detail) => format!("{detail}; {reason}"),
                    None => reason,
                });
            } else if let Some(detail) = &outcome.detail {
                data.detail = Some(detail.clone());
            }
            data.progress = None;
            data.result = outcome.result.clone();
            let finished_at = now();
            data.finished_at = Some(finished_at);
            (finished_at, data.detail.clone())
        };
        self.record_session_event(
            job,
            Some(Settlement {
                finished_at,
                status: outcome.status,
                cause,
                producer_reported,
                detail,
            }),
        );
        // Settlement ends the stream: trim to the settled cap before any observer reads the
        // terminal projection, but never below the bytes the model cursor has not consumed.
        let (waiters, view) = {
            let mut data = Self::job_data(job);
            let unconsumed = data.ring.total.saturating_sub(data.model_cursor);
            let cap = self.inner.config.settled_retain_bytes.max(unconsumed);
            data.ring.trim(cap);
            (std::mem::take(&mut data.waiters), Self::view_of(job, &data))
        };
        let mut awaited = false;
        for waiter in waiters {
            // A wait whose signal already aborted lost the race to its own abort.
            let owed = !waiter.signal.as_ref().is_some_and(AbortSignal::aborted);
            awaited |= owed;
            let _ = waiter.release.send(owed);
        }
        job.settled.send_replace(true);
        self.emit(
            &JobEvent::Settled {
                job: view,
                cause,
                awaited,
            },
            job.owner.as_ref(),
        );
        // The ring's stream ends with settlement; the signal follows the committed settlement so
        // an observer that wakes on it reads the terminal state.
        self.emit_output(job);
    }

    fn emit_output(&self, job: &Arc<Job>) {
        let total = Self::job_data(job).ring.total;
        self.emit(
            &JobEvent::Output {
                id: job.id.clone(),
                owner: job.owner.as_ref().map(|owner| owner.id().clone()),
                total,
            },
            job.owner.as_ref(),
        );
    }

    /// Consume the ring from the model cursor; the result rides the first read after settlement.
    /// A terminal read is the point the settled stream drops to the settled cap.
    fn read_job(&self, job: &Arc<Job>) -> JobRead {
        let mut data = Self::job_data(job);
        let read = data.ring.read_from(data.model_cursor);
        data.model_cursor = data.ring.total;
        let terminal = data.status.is_terminal();
        let result = if terminal && !data.result_delivered {
            data.result.clone()
        } else {
            None
        };
        if result.is_some() {
            data.result_delivered = true;
        }
        if terminal {
            data.ring.trim(self.inner.config.settled_retain_bytes);
        }
        JobRead {
            chunks: read.chunks,
            lossy: read.lossy,
            result,
            job: Self::view_of(job, &data),
        }
    }

    fn kill_job(&self, job: &Arc<Job>, reason: Option<&str>) -> Result<KillOutcome, BoxError> {
        // The kill intent is recorded before `cancel` runs: a producer that settles inside its
        // own cancel (on another worker) must see the cause and the reason. A failing cancel
        // leaves lifecycle state unchanged, so the previous values come back.
        let previous = {
            let mut data = Self::job_data(job);
            if data.status.is_terminal() {
                return Ok(KillOutcome::AlreadyFinished);
            }
            let previous = (data.kill_reason.clone(), data.settle_cause);
            // Last writer wins on purpose: the detail reports the latest kill intent.
            if let Some(reason) = reason {
                data.kill_reason = Some(reason.to_owned());
            }
            data.settle_cause = Some(JobSettleCause::Kill);
            previous
        };
        if let Some(cancel) = job.cancel.get()
            && let Err(error) = cancel(reason)
        {
            let mut data = Self::job_data(job);
            if !data.status.is_terminal() {
                data.kill_reason = previous.0;
                data.settle_cause = previous.1;
            }
            return Err(error);
        }
        let view = {
            let mut data = Self::job_data(job);
            // A settlement that ran inside `cancel` already announced the end: it keeps its
            // terminal status and no `stopping` follows it.
            if data.status.is_terminal() {
                return Ok(KillOutcome::Requested);
            }
            data.status = JobStatus::Stopping;
            Self::view_of(job, &data)
        };
        self.emit(&JobEvent::Stopping { job: view }, job.owner.as_ref());
        Ok(KillOutcome::Requested)
    }

    /// Cancel live jobs during teardown with per-job containment. A failing cancel force-fails
    /// the record and reports a possible orphan; a cancel that returns without settling remains
    /// indistinguishable from a slow stop and may stall.
    fn cancel_for_teardown(&self, jobs: &[Arc<Job>], reason: &str) {
        for job in jobs {
            {
                let mut data = Self::job_data(job);
                if data.status.is_terminal() {
                    continue;
                }
                // Whatever settles this job from here on, its owner or the service is being
                // destroyed: the settlement announces `teardown`.
                data.settle_cause = Some(JobSettleCause::Teardown);
            }
            let cancelled = match job.cancel.get() {
                Some(cancel) => cancel(Some(reason)),
                None => Ok(()),
            };
            match cancelled {
                Ok(()) => {
                    let view = {
                        let mut data = Self::job_data(job);
                        // A stop that settled inside `cancel` keeps its terminal status.
                        if data.status.is_terminal() {
                            continue;
                        }
                        data.status = JobStatus::Stopping;
                        Self::view_of(job, &data)
                    };
                    // The producer's stop can be slow; announcing the transition here keeps an
                    // observer from showing `running` for that whole window.
                    self.emit(&JobEvent::Stopping { job: view }, job.owner.as_ref());
                }
                Err(error) => {
                    let detail = format!(
                        "cancel threw during teardown; work may be orphaned: Error: {error}"
                    );
                    self.warn(format_args!(
                        "jobs: cancel of {} threw during teardown; job record forced failed and work may be orphaned: Error: {error}",
                        job.id
                    ));
                    self.settle(
                        job,
                        JobOutcome {
                            status: JobOutcomeStatus::Failed,
                            detail: Some(detail),
                            result: None,
                        },
                        JobSettleCause::Teardown,
                        false,
                    );
                }
            }
        }
    }

    /// Drop settled records and announce each removal, the one visible-set change no per-job
    /// record carries.
    fn drop_jobs(&self, jobs: &[Arc<Job>]) {
        for job in jobs {
            lock(&self.inner.state).store.shift_remove(&job.id);
            self.emit(
                &JobEvent::Removed {
                    job: Self::view(job),
                },
                job.owner.as_ref(),
            );
        }
    }

    async fn all_settled(jobs: &[Arc<Job>]) {
        for job in jobs {
            let mut settled = job.settled.subscribe();
            let _ = settled.wait_for(|done| *done).await;
        }
    }

    /// Cancel, await terminal records, and drop every job owned by one exact agent lifecycle.
    async fn dispose_owned(&self, owner: &Arc<dyn Agent>) {
        let owned: Vec<Arc<Job>> = lock(&self.inner.state)
            .store
            .values()
            .filter(|job| {
                job.owner
                    .as_ref()
                    .is_some_and(|candidate| same_agent(candidate, owner))
            })
            .cloned()
            .collect();
        self.cancel_for_teardown(&owned, "owner disposed");
        Self::all_settled(&owned).await;
        self.drop_jobs(&owned);
    }

    /// Cancel live jobs, await settlement, drop every record, and detach owner effects.
    async fn dispose_all(&self) {
        let all: Vec<Arc<Job>> = lock(&self.inner.state).store.values().cloned().collect();
        self.cancel_for_teardown(&all, "jobs service disposed");
        Self::all_settled(&all).await;
        // A subscriber mounted outside this service is still reachable here; without the
        // removals it keeps the rows it last received after a registry reload.
        self.drop_jobs(&all);
        // Detach cross-fiber owner effects after the shared store is quiescent.
        let cleanups = std::mem::take(&mut lock(&self.inner.state).owner_cleanups);
        for (_, disposer) in cleanups {
            disposer.dispose().await;
        }
    }

    /// Attach one awaited cleanup through the exact owner's scope. This survives producer reloads
    /// and joins agent quiescence; the retained disposer lets service teardown detach the
    /// cross-fiber effect.
    fn ensure_owner_cleanup(&self, owner: &Arc<dyn Agent>) -> Result<(), BoxError> {
        if lock(&self.inner.state)
            .owner_cleanups
            .iter()
            .any(|(candidate, _)| same_agent(candidate, owner))
        {
            return Ok(());
        }
        let registry = self.clone();
        let target = owner.clone();
        // Record only after attach succeeds; a disposing scope rejects new effects.
        let detach = owner.ctx().effect("jobs.ownerCleanup()", move || {
            Undo::future(move || async move {
                lock(&registry.inner.state)
                    .owner_cleanups
                    .retain(|(candidate, _)| !same_agent(candidate, &target));
                registry.dispose_owned(&target).await;
                Ok(())
            })
        })?;
        lock(&self.inner.state)
            .owner_cleanups
            .push((owner.clone(), detach));
        Ok(())
    }

    fn start_job(&self, spec: JobSpec) -> Result<JobId, BoxError> {
        let owner = self.resolve_owner(spec.owner.as_ref())?;
        if !self.serves_owner(owner.as_ref()) {
            return Err(job_error(
                "background jobs unavailable: no job controller serves this agent (load @deepseek-ai/dsh-tool-jobs in its composition)",
                JobErrorCode::ControllerUnavailable,
            ));
        }
        if spec.kind.is_empty() {
            return Err(plain("invalid job kind: expected a non-empty string"));
        }
        if spec.label.is_empty() {
            return Err(plain("invalid job label: expected a non-empty string"));
        }
        if let Some(limit) = spec.output_limit_bytes
            && (limit == 0 || limit > MAX_SAFE_INTEGER)
        {
            return Err(plain(format!(
                "invalid outputLimitBytes: expected a positive safe integer, got {limit}"
            )));
        }
        if let Some(owner) = &owner {
            self.ensure_owner_cleanup(owner)?;
        }
        let limit = self.inner.config.max_concurrent_jobs_per_owner;
        let key = owner_key(&owner);
        let mut reservation = {
            let mut admission = lock(&self.inner.admission);
            let reserved = admission.reserved.get(&key).copied().unwrap_or(0);
            if self.active_job_count(&owner) + reserved >= limit {
                return Err(job_error(
                    format!(
                        "background job limit reached for this owner (limit: {limit}); use job_kill to stop an unneeded job, wait for it to finish, then retry"
                    ),
                    JobErrorCode::LimitReached,
                ));
            }
            *admission.reserved.entry(key).or_insert(0) += 1;
            Reservation {
                inner: self.inner.clone(),
                key,
                held: true,
            }
        };

        // The producer uses the same short UUID identity as controls and durable lifecycle
        // records. Retry a collision while this registry still holds an earlier id.
        let id = loop {
            let uuid = (self.inner.uuid)().replace('-', "");
            let candidate: JobId = format!(
                "{}-{}",
                spec.kind,
                uuid.chars().take(16).collect::<String>()
            )
            .into();
            if !lock(&self.inner.state).store.contains_key(&candidate) {
                break candidate;
            }
        };
        let (settled, _) = watch::channel(false);
        let (producer_done, _) = watch::channel(false);
        let job = Arc::new(Job {
            id: id.clone(),
            tool_call: spec.tool_call.clone(),
            kind: spec.kind.clone(),
            label: spec.label.clone(),
            output_limit_bytes: spec.output_limit_bytes,
            owner: owner.clone(),
            started_at: now(),
            cancel: OnceLock::new(),
            data: Mutex::new(JobData {
                status: JobStatus::Running,
                ring: OutputRing::new(),
                model_cursor: 0,
                result_delivered: false,
                committed: false,
                announced: false,
                progress: None,
                detail: None,
                result: None,
                finished_at: None,
                kill_reason: None,
                settle_cause: None,
                waiters: Vec::new(),
                spill_paths: Vec::new(),
            }),
            settled,
            producer_done,
        });
        let append_registry = self.clone();
        let append_job = job.clone();
        let progress_registry = self.clone();
        let progress_job = job.clone();
        let handle = JobHandle::new(
            id.clone(),
            move |text, options| append_registry.append_ring(&append_job, text, options, true),
            move |line| progress_registry.update_progress(&progress_job, line),
        );
        let hooks = (spec.run)(handle)?;
        let _ = job.cancel.set(Arc::new(hooks.cancel));

        // Marking the record committed is the commit: writes staged inside the starter are
        // already in the ring, and every later handle call reaches the registered record.
        Self::job_data(&job).committed = true;
        {
            // Registering and giving up the claimed slot are one step for the limit check.
            let mut admission = lock(&self.inner.admission);
            lock(&self.inner.state)
                .store
                .insert(id.clone(), job.clone());
            release_slot(&mut admission, key);
            reservation.held = false;
        }
        self.record_session_event(&job, None);
        // The announcement precedes the pump because the pump drains its sources once
        // synchronously, and that drain may append and announce output: a job's first event is
        // always `registered`.
        let registered = Self::view(&job);
        let (announced_total, announced_progress) =
            (registered.output.total, registered.progress.clone());
        self.emit(
            &JobEvent::Registered { job: registered },
            job.owner.as_ref(),
        );
        // From here on output and progress announce themselves. What a producer wrote on another
        // thread while `registered` was going out is in the ring and the projection; one
        // catch-up event tells observers so.
        let (caught_up_output, caught_up_progress) = {
            let mut data = Self::job_data(&job);
            data.announced = true;
            (
                (data.ring.total > announced_total).then_some(data.ring.total),
                (data.progress != announced_progress && data.progress.is_some())
                    .then(|| Self::view_of(&job, &data)),
            )
        };
        if let Some(total) = caught_up_output {
            self.emit(
                &JobEvent::Output {
                    id: job.id.clone(),
                    owner: job.owner.as_ref().map(|owner| owner.id().clone()),
                    total,
                },
                job.owner.as_ref(),
            );
        }
        if let Some(view) = caught_up_progress {
            self.emit(&JobEvent::Progress { job: view }, job.owner.as_ref());
        }

        let pump = match spec.output {
            Some(sources) if !sources.is_empty() => {
                let guarded = sources
                    .into_iter()
                    .map(|source| self.guard_source(&job, source))
                    .collect();
                let sink = Arc::new(Sink {
                    registry: self.clone(),
                    job: job.clone(),
                });
                let mut producer_rx = job.producer_done.subscribe();
                let mut settled_rx = job.settled.subscribe();
                let until = async move {
                    tokio::select! {
                        _ = producer_rx.wait_for(|done| *done) => {},
                        _ = settled_rx.wait_for(|done| *done) => {},
                    }
                };
                Some(start_pump(
                    guarded,
                    sink,
                    self.inner.config.pump_poll_ms as f64,
                    until,
                )?)
            }
            _ => None,
        };

        // The producer's settlement or a registry-forced one ends the pump; the pump's final
        // drain then lands before this registry trims the ring.
        let registry = self.clone();
        let watched = job.clone();
        let done = hooks.done;
        tokio::spawn(async move {
            let (outcome, producer_reported) = match done.await {
                Ok(outcome) => (outcome, true),
                Err(error) => {
                    // Contain a producer contract violation (`done` rejected) so cleanup and
                    // waiters cannot hang.
                    registry.warn(format_args!(
                        "jobs: job {} producer done promise rejected (producer contract violation): Error: {error}",
                        watched.id
                    ));
                    (
                        JobOutcome {
                            status: JobOutcomeStatus::Failed,
                            detail: Some(format!("Error: {error}")),
                            result: None,
                        },
                        false,
                    )
                }
            };
            watched.producer_done.send_replace(true);
            if let Some(pump) = pump {
                match pump.done.await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => registry.warn(format_args!(
                        "jobs: output pump for {} failed: Error: {error}",
                        watched.id
                    )),
                    Err(error) => registry.warn(format_args!(
                        "jobs: output pump for {} failed: Error: {error}",
                        watched.id
                    )),
                }
            }
            let cause = Self::job_data(&watched)
                .settle_cause
                .unwrap_or(JobSettleCause::Producer);
            registry.settle(&watched, outcome, cause, producer_reported);
        });
        Ok(id)
    }

    fn wait_job(
        &self,
        job: Arc<Job>,
        timeout_ms: f64,
        signal: Option<AbortSignal>,
    ) -> JobFuture<'static, Result<JobView, BoxError>> {
        let registry = self.clone();
        // The synchronous prefix runs at call time, as the source's `async` method does until
        // its first `await`: validation and the waiter registration happen before this returns.
        if !timeout_ms.is_finite() || timeout_ms <= 0.0 {
            let error = plain(format!(
                "invalid wait timeout: expected a positive number of milliseconds, got {}",
                json_number(timeout_ms)
            ));
            return Box::pin(async move { Err(error) });
        }
        let (release, released) = oneshot::channel();
        let waiter_id = registry.inner.wait_ids.fetch_add(1, Ordering::Relaxed);
        {
            let mut data = Self::job_data(&job);
            if data.status.is_terminal() {
                drop(data);
                let view = Self::view(&job);
                return Box::pin(async move { Ok(view) });
            }
            if signal.as_ref().is_some_and(AbortSignal::aborted) {
                drop(data);
                return Box::pin(async move { Err(wait_aborted()) });
            }
            // The source's `deadline` refuses a delay no timer can hold; a settled job needs none.
            if timeout_ms > hoh_timeout::MAX_TIMER_DELAY_MS as f64 {
                drop(data);
                let error = plain(format!(
                    "deadline timeoutMs must be a positive finite number no greater than {}",
                    hoh_timeout::MAX_TIMER_DELAY_MS
                ));
                return Box::pin(async move { Err(error) });
            }
            data.waiters.push(Waiter {
                id: waiter_id,
                signal: signal.clone(),
                release,
            });
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(timeout_ms / 1000.0);
        let sleep = tokio::time::sleep_until(deadline);
        // A wait whose future is dropped before it finished must not stay registered: settlement
        // would count it as collecting the result.
        let guard = WaiterGuard {
            registry: registry.clone(),
            job: job.clone(),
            waiter_id,
        };
        Box::pin(async move {
            let _guard = guard;
            let mut released = released;
            let aborted = async {
                match &signal {
                    Some(signal) => {
                        signal.cancelled().await;
                    }
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::select! {
                biased;
                outcome = &mut released => {
                    // `false`: the signal had already aborted when settlement ran, so the
                    // wait lost the race to its own abort.
                    if outcome == Ok(false) {
                        return Err(wait_aborted());
                    }
                    Ok(Self::view(&job))
                }
                () = aborted => {
                    registry.leave_wait(&job, waiter_id, released, Some(wait_aborted()))
                }
                () = sleep => {
                    registry.leave_wait(&job, waiter_id, released, None)
                }
            }
        })
    }

    /// Number of waits currently parked on `id` (test access to the source's `waitResolvers`).
    #[doc(hidden)]
    pub fn wait_resolver_count(&self, id: &JobId) -> usize {
        lock(&self.inner.state)
            .store
            .get(id)
            .map_or(0, |job| Self::job_data(job).waiters.len())
    }

    /// The waiter's own abort or timeout fired: unregister it. When settlement already took it
    /// out of the set the wait is owed the terminal projection, unless its signal had aborted
    /// first.
    fn leave_wait(
        &self,
        job: &Arc<Job>,
        waiter_id: u64,
        mut released: oneshot::Receiver<bool>,
        abort: Option<BoxError>,
    ) -> Result<JobView, BoxError> {
        let removed = {
            let mut data = Self::job_data(job);
            let before = data.waiters.len();
            data.waiters.retain(|waiter| waiter.id != waiter_id);
            data.waiters.len() != before
        };
        if removed {
            return match abort {
                Some(error) => Err(error),
                None => Ok(Self::view(job)),
            };
        }
        match released.try_recv() {
            Ok(true) => Ok(Self::view(job)),
            _ => match abort {
                Some(error) => Err(error),
                None => Ok(Self::view(job)),
            },
        }
    }
}

/// Removes a wait's registration when its future goes away (a no-op once settlement or the
/// wait's own timeout or abort took it).
struct WaiterGuard {
    registry: LocalJobRegistry,
    job: Arc<Job>,
    waiter_id: u64,
}

impl Drop for WaiterGuard {
    fn drop(&mut self) {
        let waiter_id = self.waiter_id;
        LocalJobRegistry::job_data(&self.job)
            .waiters
            .retain(|waiter| waiter.id != waiter_id);
        let _ = &self.registry;
    }
}

/// The cancellation a wait reports when its caller's signal aborts; the name lets the tool
/// registry classify it.
fn wait_aborted() -> BoxError {
    Box::new(AbortError::new("wait aborted"))
}

struct Settlement {
    finished_at: i64,
    status: JobOutcomeStatus,
    cause: JobSettleCause,
    producer_reported: bool,
    detail: Option<String>,
}

struct Sink {
    registry: LocalJobRegistry,
    job: Arc<Job>,
}

impl PumpSink for Sink {
    fn append(&self, text: &str, options: JobAppendOptions) {
        self.registry.append_ring(&self.job, text, options, false);
    }

    fn spill(&self, index: usize, path: Option<&str>) {
        let mut data = LocalJobRegistry::job_data(&self.job);
        if data.spill_paths.len() <= index {
            data.spill_paths.resize(index + 1, None);
        }
        data.spill_paths[index] = path.map(str::to_owned);
    }
}

impl JobEvents for LocalJobRegistry {
    /// A subscription is an effect of `ctx`, and `{ owners: 'scope' }` names its scope.
    fn subscribe(
        &self,
        ctx: &Context,
        filter: JobEventFilter,
        listener: JobEventListener,
    ) -> Result<Disposer, KernelError> {
        self.inner.hub.subscribe(ctx, filter, listener)
    }
}

impl JobRegistryApi for LocalJobRegistry {
    fn events(&self) -> &dyn JobEvents {
        self
    }

    fn start(&self, spec: JobSpec) -> Result<JobId, BoxError> {
        self.start_job(spec)
    }

    fn list(&self, caller: Option<&SessionId>) -> Vec<JobView> {
        let jobs: Vec<Arc<Job>> = lock(&self.inner.state).store.values().cloned().collect();
        jobs.iter()
            .filter(|job| {
                job.owner
                    .as_ref()
                    .is_none_or(|owner| Some(owner.id()) == caller)
            })
            .map(|job| Self::view(job))
            .collect()
    }

    fn get(&self, id: &JobId, caller: Option<&SessionId>) -> Result<JobView, BoxError> {
        Ok(Self::view(&*self.expect(id, caller)?))
    }

    fn read(&self, id: &JobId, caller: Option<&SessionId>) -> Result<JobRead, BoxError> {
        Ok(self.read_job(&self.expect(id, caller)?))
    }

    fn read_at(
        &self,
        id: &JobId,
        from: f64,
        caller: Option<&SessionId>,
    ) -> Result<JobOutputRead, BoxError> {
        let job = self.expect(id, caller)?;
        if !from.is_finite() || from < 0.0 || from.fract() != 0.0 || from > MAX_SAFE_INTEGER as f64
        {
            return Err(plain(format!(
                "invalid output read offset: expected a non-negative safe integer, got {}",
                json_number(from)
            )));
        }
        Ok(Self::job_data(&job).ring.read_from(from as u64))
    }

    fn kill(
        &self,
        id: &JobId,
        caller: Option<&SessionId>,
        reason: Option<&str>,
    ) -> Result<KillOutcome, BoxError> {
        self.kill_job(&self.expect(id, caller)?, reason)
    }

    fn wait(
        &self,
        id: &JobId,
        timeout_ms: f64,
        caller: Option<&SessionId>,
        signal: Option<AbortSignal>,
    ) -> JobFuture<'static, Result<JobView, BoxError>> {
        match self.expect(id, caller) {
            Ok(job) => self.wait_job(job, timeout_ms, signal),
            Err(error) => Box::pin(async move { Err(error) }),
        }
    }

    fn remove(&self, id: &JobId, caller: Option<&SessionId>) -> Result<(), BoxError> {
        let job = self.expect(id, caller)?;
        let status = Self::job_data(&job).status;
        if !status.is_terminal() {
            return Err(plain(format!(
                "job {id} is still {}; kill it and wait for settlement before removing it",
                status_text(status)
            )));
        }
        self.drop_jobs(&[job]);
        Ok(())
    }

    fn attach_controller(&self, ctx: &Context, _name: &str) -> Result<Disposer, KernelError> {
        // One entry per call keeps duplicate labels independently disposable.
        self.inner
            .hub
            .layers()
            .effect(
                ctx,
                |layer| Ok(layer.controllers.append(())),
                hoh_scope::store::EffectOptions::new("jobs.attachController()"),
            )
            .map_err(kernel_error)
    }
}

/// The `LocalJobRegistry` plugin: provides `jobs` and tears every job down on unload.
pub struct LocalJobRegistryPlugin;

impl Plugin for LocalJobRegistryPlugin {
    const NAME: &'static str = "LocalJobRegistry";
    type Config = Config;
    type Deps = ();

    async fn apply(ctx: Context, _: (), config: Cfg<Config>) -> Result<(), hoh_kernel::Error> {
        let registry = LocalJobRegistry::new(ctx.clone(), (*config.get()).clone())?;
        let teardown = registry.clone();
        ctx.effect("jobs teardown", move || {
            Undo::future(move || async move {
                teardown.dispose_all().await;
                Ok(())
            })
        })?;
        hoh_jobs::provide_jobs(&ctx, Arc::new(registry)).await?;
        Ok(())
    }
}
