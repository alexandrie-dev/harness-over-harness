//! Public vocabulary for the spill storage service.

use hoh_llm::ToolCallId;
use hoh_session::SessionId;
use serde::{Deserialize, Serialize};

hoh_brand::branded_string!(pub SpillLocator);

/// Brand a backend-produced locator string.
pub fn spill_locator(locator: impl Into<String>) -> SpillLocator {
    locator.into().into()
}

/// Save-time storage namespace for a spilled artifact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpillOwner {
    pub session_id: SessionId,
}

/// Descriptive source of a spilled artifact; it does not grant access.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum SpillSource {
    Tool {
        tool_name: String,
        call_id: ToolCallId,
        label: String,
    },
    SessionReference {
        session_id: SessionId,
        label: String,
    },
}

/// One request to persist text to a spill artifact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveTextSpill {
    pub owner: SpillOwner,
    pub source: SpillSource,
    pub suggested_name: String,
    pub content: String,
}

/// A saved spill artifact and backend-specific retrieval guidance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpillRef {
    pub locator: SpillLocator,
    pub bytes: u64,
    pub retrieval_hint: String,
}
