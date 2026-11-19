//! Interfaces of packages that are not ported yet. Everything here is replaced when its owner
//! appears; nothing belongs to the source package.

use std::{future::Future, pin::Pin};

use hoh_error::BoxError;
use hoh_llm::ToolCallId;
use hoh_sandbox::SandboxExecutionPolicy;
use hoh_session::{Session, SessionId};
use hoh_timeout::AbortSignal;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

// SHIM(hoh-jobs): `JobId` of `@deepseek-ai/dsh-jobs`; replace when hoh-jobs provides it.
hoh_brand::branded_string!(pub JobId);

// SHIM(hoh-jobs): `JobStatus`; replace when hoh-jobs provides it.
/// Job lifecycle: `running`, optionally `stopping`, then exactly one terminal status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobStatus {
    Running,
    Stopping,
    Completed,
    Killed,
    Failed,
}

impl JobStatus {
    /// Whether the job is still live (`running` or `stopping`).
    pub fn is_live(self) -> bool {
        matches!(self, Self::Running | Self::Stopping)
    }
}

// SHIM(hoh-jobs): `JobChannel`; replace when hoh-jobs provides it.
/// Stream label of one output chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobChannel {
    Stdout,
    Stderr,
    Log,
}

// SHIM(hoh-jobs): `JobChunk`; replace when hoh-jobs provides it.
/// One chunk of a job's output ring.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobChunk {
    /// Absolute offset of the chunk's first byte.
    pub at: u64,
    pub text: String,
    pub channel: Option<JobChannel>,
    /// Bytes immediately before this chunk were lost.
    pub gap_before: bool,
}

// SHIM(hoh-jobs): the output coordinates of `JobView.output`; replace when hoh-jobs provides it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JobOutputView {
    pub total: u64,
    pub earliest: u64,
    /// Complete-stream files the job's pull sources currently keep; absent when none.
    pub spill_paths: Option<Vec<String>>,
}

// SHIM(hoh-jobs): `JobView`; replace when hoh-jobs provides it.
/// Read-only projection of one job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobView {
    pub id: JobId,
    pub kind: String,
    pub label: String,
    pub owner: Option<SessionId>,
    pub status: JobStatus,
    pub progress: Option<String>,
    pub detail: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub output: JobOutputView,
}

// SHIM(hoh-jobs): `JobRead`; replace when hoh-jobs provides it.
/// Output and post-read state returned by the consuming `read`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobRead {
    pub chunks: Vec<JobChunk>,
    pub lossy: bool,
    pub result: Option<String>,
    pub job: JobView,
}

// SHIM(hoh-jobs): `JobOutcome`; replace when hoh-jobs provides it.
/// How a job ended: finished, cancelled, or broke.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobOutcomeStatus {
    Completed,
    Killed,
    Failed,
}

/// Terminal result supplied by a producer through [`JobHooks::done`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobOutcome {
    pub status: JobOutcomeStatus,
    pub detail: Option<String>,
    pub result: Option<String>,
}

// SHIM(hoh-jobs): `JobSourceRead`; replace when hoh-jobs provides it.
/// One incremental read from a [`JobOutputSource`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobSourceRead {
    pub text: String,
    pub next_offset: u64,
    pub lossy: bool,
    pub spill_path: Option<String>,
}

/// A pull source the registry pumps into the job's output ring.
pub struct JobOutputSource {
    pub channel: Option<JobChannel>,
    /// Read everything captured since `from_byte` without consuming it.
    pub read: Box<dyn Fn(u64) -> JobSourceRead + Send + Sync>,
}

/// Tool dispatch that created a job; nested calls retain their root invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobToolCall {
    pub call_id: ToolCallId,
    pub root_call_id: ToolCallId,
}

/// Termination request of a job: synchronous, idempotent, takes the optional reason.
pub type CancelHook = Box<dyn Fn(Option<&str>) + Send + Sync>;

/// Hooks through which the runtime controls and observes producer work.
pub struct JobHooks {
    /// Request termination; synchronous and idempotent.
    pub cancel: CancelHook,
    /// Resolves after the producer releases its resources; never rejects.
    pub done: BoxFuture<'static, JobOutcome>,
}

/// Producer declaration passed to [`JobRegistryApi::start`].
pub struct JobSpec {
    /// Producer kind, also the id prefix (`bash`).
    pub kind: String,
    /// One-line model-facing label (the command).
    pub label: String,
    pub owner: Option<SessionId>,
    pub tool_call: Option<JobToolCall>,
    pub output: Vec<JobOutputSource>,
    /// Start the work after preflight and synchronously return its hooks.
    pub run: Box<dyn FnOnce() -> JobHooks + Send>,
}

/// Result of [`JobRegistryApi::kill`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KillOutcome {
    Requested,
    AlreadyFinished,
}

// SHIM(hoh-jobs): the `jobs` service (`JobRegistry`); only the members tool-bash calls. Replace when
// hoh-jobs provides it.
pub trait JobRegistryApi: Send + Sync + 'static {
    /// Preflight, then start and register the work; the registry-issued `<kind>-<short-uuid>` id.
    fn start(&self, spec: JobSpec) -> Result<JobId, BoxError>;
    /// Consume the ring from the model cursor.
    fn read(&self, id: &JobId, caller: Option<&SessionId>) -> Result<JobRead, BoxError>;
    /// Request cancellation, then mark the job stopping.
    fn kill(
        &self,
        id: &JobId,
        caller: Option<&SessionId>,
        reason: Option<&str>,
    ) -> Result<KillOutcome, BoxError>;
    /// Wait for settlement or timeout without cancelling the job; the caller's abort rejects only
    /// while the job is live.
    fn wait(
        &self,
        id: &JobId,
        timeout_ms: f64,
        caller: Option<&SessionId>,
        signal: Option<AbortSignal>,
    ) -> BoxFuture<'static, Result<JobView, BoxError>>;
    /// Drop one settled job's record from the visible set.
    fn remove(&self, id: &JobId, caller: Option<&SessionId>) -> Result<(), BoxError>;
}

hoh_kernel::service!(pub Jobs: dyn JobRegistryApi = "jobs");

// SHIM(hoh-sandbox-policy): the `sandboxPolicy` service; only `resolve`. Replace when
// hoh-sandbox-policy provides it.
pub trait SandboxPolicyApi: Send + Sync + 'static {
    /// The complete policy for one capability call; `session` is the calling session.
    fn resolve(&self, session: Option<&Session>) -> Result<SandboxExecutionPolicy, BoxError>;
}

hoh_kernel::service!(pub SandboxPolicyKey: dyn SandboxPolicyApi = "sandboxPolicy");
