//! Types of `src/types.ts`: the pruning policy, its resolved form and the pruning outcome.

use hoh_llm::ToolCallId;
use hoh_session::SessionSeq;
use indexmap::IndexMap;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Character-budget policy for deterministic tool-result pruning.
///
/// The raw plugin object is kept as written (`thresholdChars`, `headChars`, `tailChars`, each
/// optional) so that a stale key reaches `resolve_config` and is rejected there, as in the source.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolResultPruneConfig(pub IndexMap<String, Value>);

impl ToolResultPruneConfig {
    /// Raw value of a key; `null` counts as absent like `undefined`.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key).filter(|value| !value.is_null())
    }
}

impl From<IndexMap<String, Value>> for ToolResultPruneConfig {
    fn from(map: IndexMap<String, Value>) -> Self {
        Self(map)
    }
}

impl JsonSchema for ToolResultPruneConfig {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ToolResultPruneConfig".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        Schema::try_from(json!({
            "type": "object",
            "properties": {
                "thresholdChars": {
                    "type": "number", "multipleOf": 1, "minimum": 1, "default": 8192
                },
                "headChars": {
                    "type": "number", "multipleOf": 1, "minimum": 0, "default": 4096
                },
                "tailChars": {
                    "type": "number", "multipleOf": 1, "minimum": 0, "default": 1024
                },
            },
        }))
        .unwrap_or_else(|_| Schema::from(true))
    }
}

/// Validated, detached, immutable pruning configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedConfig {
    pub threshold_chars: u64,
    pub head_chars: u64,
    pub tail_chars: u64,
}

/// Cited source event and size accounting for one landed surface replacement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrunedEntry {
    /// Full-fidelity tool-result event shadowed by the replacement.
    pub original_seq: SessionSeq,
    /// Newly appended pruned tool-result event.
    pub replacement_seq: SessionSeq,
    /// Tool call shared by the original and replacement.
    pub call_id: ToolCallId,
    /// Original text size in Unicode code points.
    pub chars_before: u64,
    /// Replacement text size in Unicode code points.
    pub chars_after: u64,
}

/// Aggregate outcome of one stable-surface pruning pass.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneResult {
    /// Replacements in the snapshotted surface order.
    pub pruned: Vec<PrunedEntry>,
    /// Total Unicode code points removed across replacements.
    pub chars_removed: u64,
}
