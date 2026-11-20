//! Client-safe subagent projection vocabulary.

use hoh_session::{SessionId, SessionSeq};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubagentCatalogEntry {
    pub id: SessionId,
    pub created_at: i64,
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Catalog fields shared by recursive child rows. The id and creation timestamp
/// stay on the parent-owned catalog entry itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubagentCatalogEntryFields {
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubagentTimingProjection {
    pub settled_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<ActiveInterval>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_turn_completed: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActiveInterval {
    pub since: i64,
    pub through: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum SubagentIdentityProjection {
    OneShot {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        seq: SessionSeq,
    },
    Continuable {
        label: String,
        seq: SessionSeq,
    },
}

impl SubagentIdentityProjection {
    pub fn one_shot(label: Option<String>, seq: SessionSeq) -> Self {
        Self::OneShot { label, seq }
    }

    pub fn continuable(label: String, seq: SessionSeq) -> Self {
        Self::Continuable { label, seq }
    }
}
