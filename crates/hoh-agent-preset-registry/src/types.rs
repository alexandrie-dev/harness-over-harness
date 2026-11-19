//! Client-safe payloads and the event owned by the preset domain.

use hoh_session::SessionId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BuiltInPresetCopyKey {
    PresetStandardName,
    PresetStandardDescription,
    PresetPtcName,
    PresetPtcDescription,
    PresetMinimalName,
    PresetMinimalDescription,
    PresetCordisName,
    PresetCordisDescription,
}

impl BuiltInPresetCopyKey {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PresetStandardName => "presetStandardName",
            Self::PresetStandardDescription => "presetStandardDescription",
            Self::PresetPtcName => "presetPtcName",
            Self::PresetPtcDescription => "presetPtcDescription",
            Self::PresetMinimalName => "presetMinimalName",
            Self::PresetMinimalDescription => "presetMinimalDescription",
            Self::PresetCordisName => "presetCordisName",
            Self::PresetCordisDescription => "presetCordisDescription",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetDisplaySource {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetDisplayText {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresetRow {
    pub id: String,
    pub is_default: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broken: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentPresetRoster {
    pub presets: Vec<AgentPresetRow>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresetDocument {
    pub agent_preset: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One session committed a different preset to its durable log.
pub struct AgentPresetSelected {
    pub session_id: SessionId,
    pub agent_preset: String,
}
hoh_kernel::event!(
    AgentPresetSelected = "agent-preset/selected",
    hoh_kernel::Emit
);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresetNotFoundDetails {
    pub agent_preset: String,
    pub available: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresetInvalidDetails {
    pub agent_preset: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPresetLockedDetails {
    pub session_id: SessionId,
    pub agent_preset: String,
}
