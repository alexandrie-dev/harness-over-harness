//! Consumer-facing contracts of the subagent provider seam.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use hoh_agent::{Agent, AgentOptions};
use hoh_error::BoxError;
use hoh_llm::{ContentBlock, MessageId};
use hoh_session::{SessionEvent, SessionId};
use hoh_timeout::AbortSignal;
use serde::{Deserialize, Serialize};
use serde_json::Value;

hoh_brand::branded_string!(pub SubagentRunId);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubagentStopReason {
    Completed,
    Aborted,
    Error,
    MaxTokens,
    Refusal,
    Other(String),
}

impl SubagentStopReason {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Completed => "completed",
            Self::Aborted => "aborted",
            Self::Error => "error",
            Self::MaxTokens => "max-tokens",
            Self::Refusal => "refusal",
            Self::Other(reason) => reason,
        }
    }
}

impl Serialize for SubagentStopReason {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SubagentStopReason {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Ok(match value.as_str() {
            "completed" => Self::Completed,
            "aborted" => Self::Aborted,
            "error" => Self::Error,
            "max-tokens" => Self::MaxTokens,
            "refusal" => Self::Refusal,
            _ => Self::Other(value),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SubagentResult {
    pub output: Vec<ContentBlock>,
    pub structured: Option<Value>,
    pub diagnostic: Option<String>,
    pub stop_reason: SubagentStopReason,
}

impl SubagentResult {
    pub fn completed(output: Vec<ContentBlock>) -> Self {
        Self {
            output,
            structured: None,
            diagnostic: None,
            stop_reason: SubagentStopReason::Completed,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SubagentCapabilities {
    pub agent_options: bool,
    pub agent_preset: bool,
    pub output_schema: bool,
    pub depth_limit: bool,
}

pub const NO_START_CAPABILITIES: SubagentCapabilities = SubagentCapabilities {
    agent_options: false,
    agent_preset: false,
    output_schema: false,
    depth_limit: false,
};

#[derive(Clone)]
pub struct SubagentStartRequest {
    pub cwd: Option<String>,
    pub agent_preset: Option<String>,
    pub label: Option<String>,
    pub prompt: Vec<ContentBlock>,
    pub parent: Arc<dyn Agent>,
    pub signal: AbortSignal,
    pub agent_options: Option<AgentOptions>,
    pub output_schema: Option<Value>,
    pub max_depth: Option<u64>,
}

#[derive(Clone)]
pub struct ResolvedSubagentStartRequest {
    pub cwd: String,
    pub agent_preset: Option<String>,
    pub label: Option<String>,
    pub prompt: Vec<ContentBlock>,
    pub parent: Arc<dyn Agent>,
    pub signal: AbortSignal,
    pub agent_options: Option<AgentOptions>,
    pub output_schema: Option<Value>,
    pub max_depth: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubagentDeliveryTarget {
    Parent,
    Caller,
}

#[derive(Clone)]
pub struct SubagentActivationSpec {
    pub provider: String,
    pub label: String,
    pub name: Option<String>,
    pub child_id: Option<SessionId>,
    pub delivery: SubagentDeliveryTarget,
    pub request: SubagentStartRequest,
}

#[derive(Clone)]
pub struct ContinuableCreateRequest {
    pub cwd: String,
    pub session_id: SessionId,
    pub parent: Arc<dyn Agent>,
    pub signal: AbortSignal,
}

#[derive(Clone, Debug, Default)]
pub struct ContinuableCreateSpec {
    pub seed: Option<Vec<Arc<SessionEvent>>>,
}

pub trait SubagentRun: Send + Sync {
    fn id(&self) -> &SessionId;
    fn result(&self) -> BoxFuture<'static, Result<SubagentResult, BoxError>>;
    fn dispose(&self) -> BoxFuture<'static, Result<(), BoxError>>;
}

pub trait SubagentProvider: Send + Sync {
    fn name(&self) -> &str;
    fn capabilities(&self) -> SubagentCapabilities;
    fn inherits_parent_context(&self) -> bool;
    fn agent_route_defaults(&self) -> Option<(&str, &str)> {
        None
    }
    fn supports_start(&self) -> bool {
        false
    }
    fn supports_continuable(&self) -> bool {
        false
    }
    fn start(
        &self,
        _request: ResolvedSubagentStartRequest,
    ) -> Option<BoxFuture<'static, Result<Arc<dyn SubagentRun>, BoxError>>> {
        None
    }
    fn prepare_continuable(
        &self,
        _request: ContinuableCreateRequest,
    ) -> Option<BoxFuture<'static, Result<ContinuableCreateSpec, BoxError>>> {
        None
    }
}

pub struct SubagentActivation {
    pub child_id: SessionId,
    pub name: String,
    pub message_id: Option<MessageId>,
    pub result: BoxFuture<'static, Result<SubagentResult, BoxError>>,
    pub dispose: Arc<dyn Fn() -> BoxFuture<'static, Result<(), BoxError>> + Send + Sync>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubagentMemberNames {
    pub self_name: String,
    pub parent: Option<String>,
}

#[derive(Clone)]
pub enum SubagentInterruptAuthority {
    User,
    Ancestor(Arc<dyn Agent>),
}

#[derive(Clone, Debug)]
pub struct SubagentSendMessageOptions {
    pub signal: AbortSignal,
}

#[derive(Clone, Debug)]
pub struct SubagentRunInfo {
    pub run_id: SubagentRunId,
    pub provider: String,
    pub parent_session_id: SessionId,
    pub id: SessionId,
    pub local: bool,
}

#[derive(Clone, Debug)]
pub struct SubagentRunEndInfo {
    pub run_id: SubagentRunId,
    pub provider: String,
    pub parent_session_id: SessionId,
    pub id: SessionId,
    pub local: bool,
    pub stop_reason: SubagentStopReason,
    pub last_assistant_message: Option<Vec<ContentBlock>>,
}

impl SubagentRunEndInfo {
    pub fn new(
        run: SubagentRunInfo,
        stop_reason: SubagentStopReason,
        last_assistant_message: Option<Vec<ContentBlock>>,
    ) -> Self {
        Self {
            run_id: run.run_id,
            provider: run.provider,
            parent_session_id: run.parent_session_id,
            id: run.id,
            local: run.local,
            stop_reason,
            last_assistant_message,
        }
    }
}
