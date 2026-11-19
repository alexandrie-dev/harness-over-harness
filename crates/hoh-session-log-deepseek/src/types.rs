use serde::{Deserialize, Serialize};
use serde_json::Value;

use hoh_error::BoxError;

/// Request identity inspected before preparing a Session-log contribution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionLogUploadRequest {
    pub session_id: String,
    pub purpose: Option<String>,
}

/// Synchronous upload exclusion owned by its registering plugin.
pub type SessionLogUploadExclusion =
    dyn Fn(&SessionLogUploadRequest) -> Result<bool, BoxError> + Send + Sync;

/// Raw-number surface mutation on the external request wire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DeepSeekSessionLogWireSurfaceOp {
    Append(AppendWireSurfaceOp),
    Replace {
        op: ReplaceWireSurfaceOp,
        #[serde(rename = "startSeq")]
        start_seq: u64,
        #[serde(rename = "endSeq")]
        end_seq: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppendWireSurfaceOp {
    Append,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplaceWireSurfaceOp {
    Replace,
}

/// Session header fields serialized as raw primitives on the external wire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepSeekSessionLogWireHeader {
    pub version: u64,
    pub id: String,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed_length: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delegation_depth: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_preset: Option<String>,
}

/// One complete canonical event translated to raw external wire values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepSeekSessionLogWireEvent {
    pub seq: u64,
    pub time: i64,
    pub data: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignorable: Option<bool>,
    #[serde(rename = "type")]
    pub r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface_op: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_event_seqs: Option<Value>,
}

/// Versioned incremental session-log field carried by an official DeepSeek request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepSeekSessionLogExtension {
    pub version: u64,
    pub session_format_version: u64,
    pub session: DeepSeekSessionLogWireHeader,
    pub after_seq: i64,
    pub through_seq: u64,
    pub events: Vec<DeepSeekSessionLogWireEvent>,
}
