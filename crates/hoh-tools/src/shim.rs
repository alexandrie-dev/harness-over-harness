//! Interfaces of packages that are not ported yet. Everything here is replaced when its owner appears.

use std::{future::Future, pin::Pin, sync::Arc};

use hoh_error::BoxError;
use hoh_llm::{ReasoningEffortId, ToolCallId};
use hoh_scope::ScopeKey;
use hoh_session::Session;
use hoh_timeout::AbortSignal;
use indexmap::IndexMap;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

// SHIM(hoh-agent): the surface of `Agent` that the tools package reads (`agent.session`, and the agent
// itself as the scope key of its registrations); replace when hoh-agent provides it.
/// The calling agent: its scope identity and the session its tool events are logged to.
pub trait Agent: Send + Sync + 'static {
    /// The agent's scope key. In dsh the agent object itself is the key.
    fn scope_key(&self) -> ScopeKey;
    /// The live session this agent drives.
    fn session(&self) -> Session;
}

// SHIM(hoh-agent): `ModelSelection` of dsh-agent; replace when hoh-agent provides it.
/// Provider, model, and optional reasoning effort captured for one step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelSelection {
    pub provider: String,
    pub model: String,
    pub reasoning_effort: Option<ReasoningEffortId>,
}

// SHIM(hoh-user-approval): the `approval` service and its request/outcome types; replace when
// hoh-user-approval provides them.
/// One approval request routed through the calling agent.
pub struct ApprovalRequest {
    pub agent: Arc<dyn Agent>,
    pub tool_name: String,
    pub call_id: ToolCallId,
    pub reason: Option<String>,
    pub display_reason: Option<IndexMap<String, String>>,
    pub signal: AbortSignal,
}

/// The answer of the approval channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalOutcome {
    AllowedOnce,
    Rejected,
    Cancelled,
    Unavailable,
}

/// The `approval` service.
pub trait ApprovalApi: Send + Sync + 'static {
    fn request(&self, request: ApprovalRequest)
    -> BoxFuture<'_, Result<ApprovalOutcome, BoxError>>;
}

hoh_kernel::service!(pub Approval: dyn ApprovalApi = "approval");
