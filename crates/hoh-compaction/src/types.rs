use crate::CompactionId;
use hoh_commands::CommandId;
use hoh_llm::{ContentBlock, TokenUsage};
use hoh_session::SessionSeq;
use serde::{Deserialize, Serialize};

/// Inclusive surface-position range shadowed by a compaction or prune.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowedRange {
    pub start: SessionSeq,
    pub end: SessionSeq,
}

/// `compaction/start` payload. This event is log-only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionStartEvent {
    pub compaction_id: CompactionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_command_id: Option<CommandId>,
    pub turn: Option<u64>,
}

impl CompactionStartEvent {
    pub const TYPE: &'static str = "compaction/start";
}

/// Summarizer-specific fields on `compaction/summary`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CompactionSummaryOutput {
    /// Complete output from one marked call through the LLM service.
    LlmStream {
        #[serde(rename = "rawOutput")]
        raw_output: Vec<ContentBlock>,
        #[serde(rename = "llmStreamCall")]
        llm_stream_call: True,
    },
    /// Optional output from an unmarked template, remote, or other summarizer.
    Other {
        #[serde(rename = "rawOutput", default, skip_serializing_if = "Option::is_none")]
        raw_output: Option<Vec<ContentBlock>>,
    },
}

impl CompactionSummaryOutput {
    pub fn llm_stream(raw_output: Vec<ContentBlock>) -> Self {
        Self::LlmStream {
            raw_output,
            llm_stream_call: True,
        }
    }

    pub fn other(raw_output: Option<Vec<ContentBlock>>) -> Self {
        Self::Other { raw_output }
    }
}

/// Boolean marker which only admits and writes `true`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct True;

impl Serialize for True {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for True {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if bool::deserialize(deserializer)? {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom("llmStreamCall must be true"))
        }
    }
}

/// `compaction/summary` payload. This event is log-only and immediately precedes
/// the replacement user message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSummaryEvent {
    pub compaction_id: CompactionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_command_id: Option<CommandId>,
    pub summary: Vec<ContentBlock>,
    #[serde(flatten)]
    pub output: CompactionSummaryOutput,
    pub shadowed_range: ShadowedRange,
    pub shadowed_seqs: Vec<SessionSeq>,
    pub shadowed_token_count: u64,
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
}

impl CompactionSummaryEvent {
    pub const TYPE: &'static str = "compaction/summary";
}

/// `compaction/end` payload. This event is log-only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionEndEvent {
    pub compaction_id: CompactionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_command_id: Option<CommandId>,
    pub turn: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl CompactionEndEvent {
    pub const TYPE: &'static str = "compaction/end";
}

/// `compaction/prune` shadow-price payload. This log-only event immediately
/// precedes its replacement surface event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionPruneEvent {
    pub shadowed_range: ShadowedRange,
    pub shadowed_seqs: Vec<SessionSeq>,
    pub shadowed_token_count: u64,
}

impl CompactionPruneEvent {
    pub const TYPE: &'static str = "compaction/prune";
}

/// Result of a successful compaction operation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionResult {
    pub compaction_id: CompactionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_command_id: Option<CommandId>,
    pub start_seq: SessionSeq,
    pub summary_seq: SessionSeq,
    pub end_seq: SessionSeq,
    pub summary: Vec<ContentBlock>,
    pub shadowed_range: ShadowedRange,
    pub shadowed_seqs: Vec<SessionSeq>,
    pub shadowed_token_count: u64,
}
