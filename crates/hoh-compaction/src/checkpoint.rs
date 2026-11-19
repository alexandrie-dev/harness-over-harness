use crate::CompactionId;
use hoh_commands::CommandId;
use hoh_llm::MessageSource;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Message source carried by a concrete compaction checkpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionCheckpointSource {
    pub kind: CompactCheckpointKind,
    pub compaction_id: CompactionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_command_id: Option<CommandId>,
}

/// The fixed `"compact-checkpoint"` source marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompactCheckpointKind {
    #[serde(rename = "compact-checkpoint")]
    CompactCheckpoint,
}

/// Create a checkpoint source correlated with one compaction transaction.
pub fn compact_checkpoint_source(
    compaction_id: CompactionId,
    source_command_id: Option<CommandId>,
) -> CompactionCheckpointSource {
    CompactionCheckpointSource {
        kind: CompactCheckpointKind::CompactCheckpoint,
        compaction_id,
        source_command_id,
    }
}

/// Test whether a persisted message source identifies a compaction checkpoint.
pub fn is_compact_checkpoint_source(source: &MessageSource) -> bool {
    source.kind == "compact-checkpoint"
}

impl From<CompactionCheckpointSource> for MessageSource {
    fn from(source: CompactionCheckpointSource) -> Self {
        let mut data = Map::new();
        data.insert(
            "compactionId".into(),
            Value::String(source.compaction_id.into()),
        );
        if let Some(command_id) = source.source_command_id {
            data.insert("sourceCommandId".into(), Value::String(command_id.into()));
        }
        Self {
            kind: "compact-checkpoint".into(),
            data,
        }
    }
}
