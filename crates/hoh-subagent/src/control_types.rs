//! Client-safe catalog and continuation control vocabulary.

use hoh_attachment::PromptContentPart;
use hoh_llm::MessageId;
use hoh_session::SessionId;
use serde::{Deserialize, Serialize};

hoh_brand::branded_string!(pub SubagentPromptRequestId);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubagentActivity {
    Running,
    Inactive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubagentDiagnosticReason {
    Corrupt,
    Unsupported,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SubagentListEntry {
    Child {
        id: SessionId,
        activity: SubagentActivity,
        #[serde(flatten)]
        fields: crate::projection_types::SubagentCatalogEntryFields,
        #[serde(rename = "hasChildren")]
        has_children: bool,
    },
    Diagnostic {
        id: SessionId,
        reason: SubagentDiagnosticReason,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentDescendantListEntry {
    #[serde(flatten)]
    pub entry: SubagentListEntry,
    pub parent_id: SessionId,
    pub depth: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubagentAddressMode {
    OneShot,
    Continuable,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentAddress {
    pub parent_session_id: SessionId,
    pub child_session_id: SessionId,
    pub mode: SubagentAddressMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubagentDelivery {
    Queue,
    Steer,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentPromptRequest {
    pub request_id: SubagentPromptRequestId,
    pub child_session_id: SessionId,
    pub delivery: SubagentDelivery,
    pub content: Vec<PromptContentPart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_time_zone: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentPromptReceipt {
    pub message_id: MessageId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentInterruptReceipt {
    pub accepted: bool,
}
