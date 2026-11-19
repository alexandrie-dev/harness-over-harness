//! Durable agent session-event vocabulary shared with type-only consumers
//! (`@deepseek-ai/dsh-agent/types`).

use hoh_llm::{ReasoningEffortId, UserMessage};
use hoh_session::{OptionalSessionSeq, SessionSeq};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One of the two ordered pending-message lists owned by an agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InboxTarget {
    #[serde(rename = "next-turn")]
    NextTurn,
    #[serde(rename = "next-step")]
    NextStep,
}

impl InboxTarget {
    /// The durable tag (`next-turn` / `next-step`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NextTurn => "next-turn",
            Self::NextStep => "next-step",
        }
    }
}

/// Complete pending Inbox value reconstructed from durable splices.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct InboxState {
    #[serde(rename = "next-turn")]
    pub next_turn: Vec<UserMessage>,
    #[serde(rename = "next-step")]
    pub next_step: Vec<UserMessage>,
}

impl InboxState {
    /// The list a target names.
    pub fn list(&self, target: InboxTarget) -> &Vec<UserMessage> {
        match target {
            InboxTarget::NextTurn => &self.next_turn,
            InboxTarget::NextStep => &self.next_step,
        }
    }
}

/// Wire-JSON pending Inbox value: each message stays raw JSON.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct InboxWireState {
    #[serde(rename = "next-turn")]
    pub next_turn: Vec<Value>,
    #[serde(rename = "next-step")]
    pub next_step: Vec<Value>,
}

/// The latest step boundary (`step/start` or `step/end`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepBoundary {
    pub kind: StepBoundaryKind,
    pub seq: SessionSeq,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepBoundaryKind {
    Start,
    End,
}

/// Turn and step positions recorded in a Session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnBoundaryProjection {
    /// Seq of the open turn's `turn/start`, or `null` between turns.
    pub open_turn_start_seq: OptionalSessionSeq,
    /// Seq of the latest `step/start` event, or `null` before the first step.
    pub last_step_start_seq: OptionalSessionSeq,
    /// The latest step boundary and its seq, or `null` before the first step boundary.
    pub last_step_boundary: Option<StepBoundary>,
    /// Turn number of the latest `turn/start`; 0 before the first turn.
    pub last_turn: u64,
}

/// Complete provider, model, and optional reasoning effort chosen for one Session's model steps.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelection {
    /// Registered provider route.
    pub provider: String,
    /// Provider-owned model id.
    pub model: String,
    /// Adapter-owned reasoning effort, or provider/default behavior when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffortId>,
}

/// Client view of the durable model-selection fold.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSelectionProjection {
    /// Selection used by the latest recorded model request.
    #[serde(rename = "lastUsed")]
    pub last_used: Option<ModelSelection>,
    /// Selection the next model step uses; `null` while the Agent's initial options supply the route.
    pub next: Option<ModelSelection>,
}
