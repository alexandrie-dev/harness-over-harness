//! Public agent types and live-runtime events. Durable transcript facts and turn/step boundaries
//! remain `hoh-session` events.

use crate::types::{InboxTarget, ModelSelection};
use futures_util::future::BoxFuture;
use hoh_error::BoxError;
use hoh_kernel::{Contained, Context, Serial, Waterfall};
use hoh_llm::{
    LlmAttemptId, LlmCallConfig, LlmFailure, MessageId, ReasoningEffortId, ResolvedRetryPolicy,
    StreamChunk, UserMessage,
};
use hoh_scope::ScopeKey;
use hoh_session::{AgentCancelCause, Session, SessionId, SessionSeq};
use hoh_timeout::AbortSignal;
use std::{any::Any, fmt, sync::Arc};

/// An option the driver reads at call time: a constant or a live accessor.
#[derive(Clone)]
pub enum Live<T> {
    Value(T),
    Accessor(Arc<dyn Fn() -> Option<T> + Send + Sync>),
}

impl<T: Clone> Live<T> {
    /// The current value.
    pub fn get(&self) -> Option<T> {
        match self {
            Self::Value(value) => Some(value.clone()),
            Self::Accessor(read) => read(),
        }
    }
}

/// Merge-extensible agent options. Route fields may be live accessors until the Session records a
/// selection or request; step preparation captures their value.
#[derive(Clone, Default)]
pub struct AgentOptions {
    /// Provider route (must have a registered adapter at call time).
    pub provider: Option<Live<String>>,
    /// Model id interpreted by the selected provider adapter.
    pub model: Option<Live<String>>,
    /// Adapter-owned reasoning effort for the selected provider/model route.
    pub reasoning_effort: Option<Live<ReasoningEffortId>>,
    /// Maximum output tokens for each conversation-model request.
    pub max_tokens: Option<Live<u64>>,
}

impl AgentOptions {
    pub fn provider(&self) -> Option<String> {
        self.provider.as_ref().and_then(Live::get)
    }
    pub fn model(&self) -> Option<String> {
        self.model.as_ref().and_then(Live::get)
    }
    pub fn reasoning_effort(&self) -> Option<ReasoningEffortId> {
        self.reasoning_effort.as_ref().and_then(Live::get)
    }
    pub fn max_tokens(&self) -> Option<u64> {
        self.max_tokens.as_ref().and_then(Live::get)
    }
}

/// Options for [`Agent::cancel`].
#[derive(Clone, Copy, Debug, Default)]
pub struct CancelOptions {
    /// Preserve queued and steering inbox items instead of discarding them.
    pub keep_inbox: bool,
}

/// The reason a loop-owned [`AbortSignal`] carries when `cancel` aborts it: the stable caller
/// intent. Read it back with `reason.downcast_ref::<CancelCauseError>()`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CancelCauseError(pub AgentCancelCause);

impl fmt::Display for CancelCauseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            AgentCancelCause::User => f.write_str("agent cancelled by user"),
            AgentCancelCause::Parent => f.write_str("agent cancelled by parent"),
            AgentCancelCause::Hook { reason } => write!(f, "agent cancelled by hook: {reason}"),
            AgentCancelCause::Disposed => f.write_str("agent disposed"),
        }
    }
}
impl std::error::Error for CancelCauseError {}

/// Agent-owned access to the model choice for future steps.
pub trait Model: Send + Sync {
    /// Resolve the choice for the next step to begin preparation. Recorded choices take precedence
    /// over the last request and then the Agent's live options.
    fn current(&self) -> Option<ModelSelection>;
    /// Replace the next step's complete selection. An equal recorded choice is a no-op; an
    /// options-only fallback is always recorded.
    fn select(&self, selection: &ModelSelection) -> Result<(), BoxError>;
}

/// Agent-owned access to pending work; concrete storage belongs to the driver.
pub trait Inbox: Send + Sync {
    /// Prompts awaiting individual turns.
    fn next_turn(&self) -> Vec<UserMessage>;
    /// Input awaiting the next step boundary.
    fn next_step(&self) -> Vec<UserMessage>;
    /// Durably cancel all pending input, clearing next-step before next-turn.
    fn clear(&self) -> Result<(), BoxError>;
    fn append(&self, target: InboxTarget, message: UserMessage) -> Result<(), BoxError>;
    fn prepend(&self, target: InboxTarget, message: UserMessage) -> Result<(), BoxError>;
    /// Replace one pending message in place; whether it was still pending.
    fn replace(&self, message_id: &MessageId, new_message: UserMessage) -> Result<bool, BoxError>;
    /// Remove one pending message; whether it was still pending.
    fn remove(&self, message_id: &MessageId) -> Result<bool, BoxError>;
    /// Standard splice semantics, durably recorded in normalized form. `start` and `delete_count`
    /// follow JavaScript's `splice` (`f64`: negative counts from the end, `NaN` is 0, infinity clamps).
    fn splice(
        &self,
        target: InboxTarget,
        start: f64,
        delete_count: f64,
        inserted: Vec<UserMessage>,
    ) -> Result<Vec<UserMessage>, BoxError>;
}

/// An agent's lifecycle state, emitted on every transition as `agent/status`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentStatus {
    Idle,
    Running,
}

impl AgentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
        }
    }
}

/// Whether and with which messages the loop enters a proposed step.
#[derive(Clone, Debug, PartialEq)]
pub enum PreStepDecision {
    Reject,
    Enter {
        messages: Vec<UserMessage>,
        /// Start a distinct model-message series before this step's admitted messages.
        starts_request_series: bool,
    },
}

/// Action returned by a listener that owns model-request recovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestErrorAction {
    Retry,
}

/// Why a session lifecycle began; seeded creates are `startup`, persisted loads are `resume`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionStartSource {
    Startup,
    Resume,
    Clear,
    Compact,
}

impl SessionStartSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Resume => "resume",
            Self::Clear => "clear",
            Self::Compact => "compact",
        }
    }
}

/// Durable settlement of one assistant attempt.
#[derive(Clone, Debug, PartialEq)]
pub enum AssistantStreamOutcome {
    Committed {
        event_type: &'static str,
        seq: SessionSeq,
    },
    Abandoned,
}

/// One process-local live assistant streaming publication.
#[derive(Clone, Debug, PartialEq)]
pub enum AssistantStreamFrame {
    Start {
        attempt_id: LlmAttemptId,
        /// Monotone within one attached Agent lifecycle; replacement restarts at 1.
        revision: u64,
        turn: u64,
        step: u64,
    },
    Chunk {
        attempt_id: LlmAttemptId,
        revision: u64,
        /// Dense zero-based position within the attempt.
        index: u64,
        /// Safe-integer timestamp reused by the durable embedded stream.
        time: i64,
        chunk: StreamChunk,
    },
    End {
        attempt_id: LlmAttemptId,
        revision: u64,
        /// Number of chunk frames emitted by this attempt.
        index: u64,
        outcome: AssistantStreamOutcome,
    },
}

/// The type-erased result of a maintenance task.
pub type MaintenanceFuture = BoxFuture<'static, Result<Box<dyn Any + Send>, BoxError>>;

/// A one-shot non-turn maintenance task (type-erased result).
pub type MaintenanceTask = Box<dyn FnOnce(AbortSignal) -> MaintenanceFuture + Send>;

/// The live-agent handle. `Agent` of `@deepseek-ai/dsh-agent` with the runtime face merged in.
pub trait Agent: Send + Sync + 'static {
    /// Session-backed Agent identity.
    fn id(&self) -> &SessionId;
    /// Identity of this agent's scope (`createScope(ctx, agent)` key of the source).
    fn scope_key(&self) -> ScopeKey;
    /// Route fallback and output controls.
    fn options(&self) -> &AgentOptions;
    /// The live session this agent drives; its log is the durable source of truth.
    fn session(&self) -> &Session;
    fn model(&self) -> &dyn Model;
    fn inbox(&self) -> &dyn Inbox;
    /// The current lifecycle state, mirrored on every `agent/status` transition.
    fn status(&self) -> AgentStatus;
    /// Agent-scoped context; its contributions are agent-local and unwind on disposal.
    fn ctx(&self) -> &Context;

    /// Clear queued and steering work — unless `keep_inbox` — and abort the active turn or
    /// between-turn task with `cause` as the signal reason ([`CancelCauseError`]). The first cause
    /// wins; with no active activity this is a no-op. Queue-write failures still abort the activity
    /// and propagate to the caller.
    fn cancel(&self, cause: AgentCancelCause, options: CancelOptions) -> Result<(), BoxError>;
    /// Resolve after the current whole-agent activity reaches quiescence.
    fn when_idle(&self) -> BoxFuture<'static, ()>;
    /// Run one non-turn maintenance task from the true idle phase; fails synchronously when
    /// turn-driving or another maintenance task owns the agent. Typed form: [`run_maintenance`].
    fn run_maintenance_erased(&self, task: MaintenanceTask) -> Result<MaintenanceFuture, BoxError>;
    /// Route identified input to an inbox boundary and optionally wake the driver.
    fn send(&self, message: UserMessage, target: InboxTarget, wakeup: bool)
    -> Result<(), BoxError>;
    /// Queue an ordinary follow-up turn and wake the driver.
    fn followup(&self, message: UserMessage) -> Result<(), BoxError>;
    /// Submit steering for the nearest step.
    fn steer(&self, message: UserMessage) -> Result<(), BoxError>;
    /// Queue model-facing context for the next pre-step without waking the driver.
    fn inject(&self, message: UserMessage) -> Result<(), BoxError>;
}

/// Typed `runMaintenance<T>`: runs `task` on `agent`'s maintenance phase and returns its result.
pub fn run_maintenance<T, F, Fut>(
    agent: &dyn Agent,
    task: F,
) -> Result<impl std::future::Future<Output = Result<T, BoxError>> + Send + 'static, BoxError>
where
    T: Send + 'static,
    F: FnOnce(AbortSignal) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, BoxError>> + Send + 'static,
{
    let erased: MaintenanceTask = Box::new(move |signal| {
        Box::pin(async move { Ok(Box::new(task(signal).await?) as Box<dyn Any + Send>) })
    });
    let future = agent.run_maintenance_erased(erased)?;
    Ok(async move {
        let value = future.await?;
        // The erased task above always boxes a `T`.
        Ok(*value
            .downcast::<T>()
            .map_err(|_| BoxError::from("maintenance result type mismatch"))?)
    })
}

// ---- lifecycle events -----------------------------------------------------------------------

/// `agent/created` (serial): an entered agent is ready for per-agent initialization after factory
/// setup. Listeners run in order and are awaited before creation resolves; a failure fails creation.
pub struct AgentCreated {
    pub agent: Arc<dyn Agent>,
    pub source: SessionStartSource,
    pub signal: Option<AbortSignal>,
}
hoh_kernel::event!(AgentCreated = "agent/created", Serial<()>);

/// `agent/disposed`: an agent left the registry.
pub struct AgentDisposed {
    pub agent: Arc<dyn Agent>,
}
hoh_kernel::event!(AgentDisposed = "agent/disposed", Contained);

/// `agent/status`: status changed (`idle` ⇄ `running`).
pub struct AgentStatusChanged {
    pub agent: Arc<dyn Agent>,
    pub status: AgentStatus,
}
hoh_kernel::event!(AgentStatusChanged = "agent/status", Contained);

/// `agent/inbox/inserted`: one message entered the live inbox.
pub struct AgentInboxInserted {
    pub agent: Arc<dyn Agent>,
    pub message: UserMessage,
}
hoh_kernel::event!(AgentInboxInserted = "agent/inbox/inserted", Contained);

/// `agent/inbox/claimed`: one message left the inbox inside its open turn.
pub struct AgentInboxClaimed {
    pub agent: Arc<dyn Agent>,
    pub message: UserMessage,
    pub turn: u64,
}
hoh_kernel::event!(AgentInboxClaimed = "agent/inbox/claimed", Contained);

/// `agent/inbox/discarded`: one message was discarded from the live inbox.
pub struct AgentInboxDiscarded {
    pub agent: Arc<dyn Agent>,
    pub message: UserMessage,
}
hoh_kernel::event!(AgentInboxDiscarded = "agent/inbox/discarded", Contained);

// ---- the machine's extension points ---------------------------------------------------------

/// `agent/pre-step` (waterfall): reject a proposed step or replace the messages that enter it.
/// Calling `next` preserves the current messages.
pub struct AgentPreStep {
    pub agent: Arc<dyn Agent>,
    /// Messages removed from the inbox for this step.
    pub messages: Vec<UserMessage>,
    pub turn: u64,
    pub step: u64,
    pub signal: AbortSignal,
    pub model_selection: ModelSelection,
}
hoh_kernel::event!(AgentPreStep = "agent/pre-step", Waterfall<PreStepDecision>);

/// `agent/request` (waterfall, deprecated in the source): replace the frozen call configuration.
pub struct AgentRequest {
    pub agent: Arc<dyn Agent>,
    pub turn: u64,
    pub step: u64,
    pub signal: AbortSignal,
}
hoh_kernel::event!(AgentRequest = "agent/request", Waterfall<LlmCallConfig>);

/// `agent/request-error` (waterfall): handle one failed model-request attempt before the loop
/// retries or closes its step. `Some(Retry)` without calling `next` means the listener owns recovery.
pub struct AgentRequestError {
    pub agent: Arc<dyn Agent>,
    pub turn: u64,
    pub step: u64,
    pub provider: String,
    pub failure: LlmFailure,
    pub retry_policy: Option<ResolvedRetryPolicy>,
    pub signal: AbortSignal,
}
hoh_kernel::event!(
    AgentRequestError = "agent/request-error",
    Waterfall<Option<RequestErrorAction>>
);

/// `agent/assistant-stream`: process-local assistant-stream publication.
pub struct AgentAssistantStream {
    pub agent: Arc<dyn Agent>,
    pub frame: AssistantStreamFrame,
}
hoh_kernel::event!(AgentAssistantStream = "agent/assistant-stream", Contained);

/// `agent/turn-stopping` (serial): the turn is about to close. A listener that objects steers
/// (`agent.steer(..)`) and the machine re-reads its inbox.
pub struct AgentTurnStopping {
    pub agent: Arc<dyn Agent>,
    pub turn: u64,
    pub signal: AbortSignal,
}
hoh_kernel::event!(AgentTurnStopping = "agent/turn-stopping", Serial<()>);

/// `agent/error`: a step or turn errored. The failure is carried verbatim.
pub struct AgentError {
    pub agent: Arc<dyn Agent>,
    pub turn: u64,
    pub step: u64,
    pub error: hoh_error::SharedError,
}
hoh_kernel::event!(AgentError = "agent/error", Contained);
