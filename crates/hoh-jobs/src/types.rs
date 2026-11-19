//! Types shared by job producers, registry implementations and consumers.

pub use crate::brand::JobId;
pub use crate::view::{JobChannel, JobChunk, JobKind, JobStatus, JobView};
use hoh_error::BoxError;
use hoh_kernel::{Context, Disposer, KernelError};
use hoh_llm::ToolCallId;
use hoh_session::SessionId;
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin, sync::Arc};

pub type JobFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Tool dispatch that created a job; nested calls retain their root invocation.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobToolCall {
    pub call_id: ToolCallId,
    pub root_call_id: ToolCallId,
}

/// Durable identity and registration time of one owned job execution.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRegisteredData {
    pub job_id: JobId,
    pub owner: SessionId,
    pub kind: String,
    pub label: String,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<JobToolCall>,
}

/// Terminal status supplied by a producer.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobOutcomeStatus {
    Completed,
    Killed,
    Failed,
}

/// Why a job settled.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobSettleCause {
    Producer,
    Kill,
    Teardown,
}

/// Registry settlement and its durable registration facts.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSettledData {
    pub job_id: JobId,
    pub owner: SessionId,
    pub kind: String,
    pub label: String,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<JobToolCall>,
    pub finished_at: i64,
    pub status: JobOutcomeStatus,
    pub cause: JobSettleCause,
    pub producer_reported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Terminal result supplied by a producer through [`JobHooks::done`].
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobOutcome {
    pub status: JobOutcomeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

/// One incremental read from a [`JobOutputSource`].
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSourceRead {
    pub text: String,
    pub next_offset: u64,
    pub lossy: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spill_path: Option<String>,
}

/// A pull source the registry pumps into the output ring.
pub struct JobOutputSource {
    pub channel: Option<JobChannel>,
    pub read: Box<dyn Fn(u64) -> Result<JobSourceRead, BoxError> + Send + Sync>,
}

/// Options for one [`JobHandle::append`] operation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JobAppendOptions {
    pub channel: Option<JobChannel>,
    pub gap_before: bool,
}

type AppendJob = dyn Fn(&str, JobAppendOptions) + Send + Sync;
type UpdateProgress = dyn Fn(&str) + Send + Sync;

/// Producer face of one registered job.
#[derive(Clone)]
pub struct JobHandle {
    pub id: JobId,
    append: Arc<AppendJob>,
    update_progress: Arc<UpdateProgress>,
}

impl JobHandle {
    pub fn new(
        id: JobId,
        append: impl Fn(&str, JobAppendOptions) + Send + Sync + 'static,
        update_progress: impl Fn(&str) + Send + Sync + 'static,
    ) -> Self {
        Self {
            id,
            append: Arc::new(append),
            update_progress: Arc::new(update_progress),
        }
    }

    pub fn append(&self, text: &str) {
        (self.append)(text, JobAppendOptions::default());
    }

    pub fn append_with_options(&self, text: &str, options: JobAppendOptions) {
        (self.append)(text, options);
    }

    pub fn update_progress(&self, line: &str) {
        (self.update_progress)(line);
    }
}

pub type JobCancel = Box<dyn Fn(Option<&str>) -> Result<(), BoxError> + Send + Sync>;

/// Hooks through which the runtime controls and observes producer work.
pub struct JobHooks {
    pub cancel: JobCancel,
    pub done: JobFuture<'static, Result<JobOutcome, BoxError>>,
}

/// Producer declaration passed to [`crate::JobRegistryApi::start`].
pub struct JobSpec {
    pub kind: JobKind,
    pub label: String,
    pub owner: Option<SessionId>,
    pub tool_call: Option<JobToolCall>,
    pub output_limit_bytes: Option<u64>,
    pub output: Option<Vec<JobOutputSource>>,
    pub run: Box<dyn FnOnce(JobHandle) -> Result<JobHooks, BoxError> + Send>,
}

/// Output and post-read state returned by the consuming registry read.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRead {
    pub chunks: Vec<JobChunk>,
    pub lossy: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    pub job: JobView,
}

/// Result of one non-consuming output read.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobOutputRead {
    pub chunks: Vec<JobChunk>,
    pub next: u64,
    pub lossy: bool,
}

/// Result of a cancellation request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KillOutcome {
    Requested,
    AlreadyFinished,
}

/// One lifecycle or output commit.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum JobEvent {
    Registered {
        job: JobView,
    },
    Progress {
        job: JobView,
    },
    Stopping {
        job: JobView,
    },
    Removed {
        job: JobView,
    },
    Settled {
        job: JobView,
        cause: JobSettleCause,
        awaited: bool,
    },
    Output {
        id: JobId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<SessionId>,
        total: u64,
    },
}

/// Scope selector for a subscription.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobEventOwners {
    All,
    Scope,
}

/// Which jobs a subscription hears about.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum JobEventFilter {
    Owner { owner: SessionId },
    Owners { owners: JobEventOwners },
}

/// Contained event callback. The provider also contains panics at dispatch.
pub type JobEventListener = Arc<dyn Fn(&JobEvent) -> Result<(), BoxError> + Send + Sync>;

/// The registry's context-bound event stream.
pub trait JobEvents: Send + Sync + 'static {
    fn subscribe(
        &self,
        ctx: &Context,
        filter: JobEventFilter,
        listener: JobEventListener,
    ) -> Result<Disposer, KernelError>;
}
