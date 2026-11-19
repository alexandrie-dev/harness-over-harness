//! Client-safe, read-only job projections and output chunks.

use crate::JobId;
use hoh_session::SessionId;
use serde::{Deserialize, Deserializer, Serialize};

/// Job lifecycle: `running`, optionally `stopping`, then one terminal status.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Running,
    Stopping,
    Completed,
    Killed,
    Failed,
}

impl JobStatus {
    /// Whether the job is still running or stopping.
    pub const fn is_live(self) -> bool {
        matches!(self, Self::Running | Self::Stopping)
    }

    /// Whether the job has reached a terminal status.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Killed | Self::Failed)
    }
}

/// An open producer kind. Other crates add kind names without changing this crate.
pub type JobKind = String;

/// Stream label of one output chunk.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobChannel {
    Stdout,
    Stderr,
    Log,
}

fn deserialize_channel<'de, D>(deserializer: D) -> Result<Option<JobChannel>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value.as_ref().and_then(serde_json::Value::as_str) {
        Some("stdout") => Some(JobChannel::Stdout),
        Some("stderr") => Some(JobChannel::Stderr),
        Some("log") => Some(JobChannel::Log),
        _ => None,
    })
}

const fn is_false(value: &bool) -> bool {
    !*value
}

/// One chunk of a job's output ring.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobChunk {
    /// Absolute UTF-8 byte offset of the chunk's first byte.
    pub at: u64,
    /// Chunk text exactly as appended.
    pub text: String,
    /// Unknown labels decode as absent, matching the client-safe source seam.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_channel"
    )]
    pub channel: Option<JobChannel>,
    /// Whether bytes immediately before this chunk were lost.
    #[serde(default, skip_serializing_if = "is_false")]
    pub gap_before: bool,
}

/// Absolute output-ring coordinates and retained complete-stream files.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobOutputView {
    pub total: u64,
    pub earliest: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spill_paths: Option<Vec<String>>,
}

/// A fresh, read-only projection of one job.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobView {
    pub id: JobId,
    /// Open string so clients compiled with fewer producer kinds remain compatible.
    pub kind: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_limit_bytes: Option<u64>,
    pub status: JobStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    pub output: JobOutputView,
}
