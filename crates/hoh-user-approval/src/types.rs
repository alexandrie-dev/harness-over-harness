//! The source's client-safe `./types` entry point and log-only audit payloads.
use hoh_agent::Agent;
use hoh_kernel::{Event, Waterfall};
use hoh_llm::ToolCallId;
use hoh_timeout::AbortSignal;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

pub use hoh_brand::ids::ApprovalRequestId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalOutcome {
    AllowedOnce,
    Rejected,
    Cancelled,
    Unavailable,
}
impl ApprovalOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AllowedOnce => "allowed-once",
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
            Self::Unavailable => "unavailable",
        }
    }
    pub(crate) fn normalize(value: Value) -> Self {
        match value.as_str() {
            Some("allowed-once") => Self::AllowedOnce,
            Some("rejected") => Self::Rejected,
            Some("cancelled") => Self::Cancelled,
            _ => Self::Unavailable,
        }
    }
}
impl From<ApprovalOutcome> for Value {
    fn from(outcome: ApprovalOutcome) -> Self {
        Value::String(outcome.as_str().into())
    }
}

/// Same-process readonly request. Arc preserves exact request and agent identities.
pub struct ApprovalRequestEvent {
    pub agent: Arc<dyn Agent>,
    pub tool_name: String,
    pub call_id: Option<ToolCallId>,
    pub reason: Option<String>,
    pub display_reason: Option<DisplayReason>,
    pub signal: Option<AbortSignal>,
}
impl ApprovalRequestEvent {
    pub fn new(agent: Arc<dyn Agent>, tool_name: impl Into<String>) -> Self {
        Self {
            agent,
            tool_name: tool_name.into(),
            call_id: None,
            reason: None,
            display_reason: None,
            signal: None,
        }
    }
}
impl Event for ApprovalRequestEvent {
    const NAME: &'static str = "approval/request";
    // JS answerers are untrusted; normalize before returning the closed outcome.
    type Mode = Waterfall<Value>;
}
impl hoh_agent::AgentSubjectEvent for ApprovalRequestEvent {}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct DisplayReason(IndexMap<String, String>);
impl DisplayReason {
    pub fn new(en: impl Into<String>) -> Self {
        Self(IndexMap::from([("en".into(), en.into())]))
    }
    pub fn insert(&mut self, locale: impl Into<String>, text: impl Into<String>) {
        self.0.insert(locale.into(), text.into());
    }
    pub fn get(&self, locale: &str) -> Option<&str> {
        self.0.get(locale).map(String::as_str)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalAsked {
    pub id: ApprovalRequestId,
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<ToolCallId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
impl ApprovalAsked {
    pub const TYPE: &'static str = "approval/asked";
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApprovalDecided {
    pub id: ApprovalRequestId,
    pub outcome: ApprovalOutcome,
}
impl ApprovalDecided {
    pub const TYPE: &'static str = "approval/decided";
}
