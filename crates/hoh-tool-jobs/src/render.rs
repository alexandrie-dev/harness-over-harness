//! Model-facing rendering of registry reads: the consuming delta as the shell tools have always
//! shown it (stdout, then one marked stderr section), the `[status: …]` line, and the public job
//! projection the tool schemas expose.
//!
//! Port of `src/render.ts` (`@deepseek-ai/dsh-tool-jobs/render`).

use hoh_jobs::{JobChannel, JobChunk, JobStatus, JobView};
use serde_json::{Map, Value};

/// Job state safe for model-authored programs; ownership and offsets are omitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicJobSnapshot {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub status: JobStatus,
    /// The live progress line while running, the terminal reason once settled.
    pub detail: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

impl PublicJobSnapshot {
    /// The JSON object the tool output carries, keys in the order the source builds them.
    pub fn to_map(&self) -> Map<String, Value> {
        let mut map = Map::new();
        map.insert("id".into(), Value::String(self.id.clone()));
        map.insert("kind".into(), Value::String(self.kind.clone()));
        map.insert("label".into(), Value::String(self.label.clone()));
        map.insert(
            "status".into(),
            Value::String(status_name(self.status).into()),
        );
        if let Some(detail) = &self.detail {
            map.insert("detail".into(), Value::String(detail.clone()));
        }
        map.insert("startedAt".into(), Value::from(self.started_at));
        if let Some(finished_at) = self.finished_at {
            map.insert("finishedAt".into(), Value::from(finished_at));
        }
        map
    }
}

/// The wire name of a lifecycle status.
pub fn status_name(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Running => "running",
        JobStatus::Stopping => "stopping",
        JobStatus::Completed => "completed",
        JobStatus::Killed => "killed",
        JobStatus::Failed => "failed",
    }
}

/// The one-line qualifier the model reads beside a status: live progress while the job runs, the
/// terminal reason once it settled.
pub fn job_detail(job: &JobView) -> Option<&str> {
    job.progress.as_deref().or(job.detail.as_deref())
}

/// Remove ownership, offsets, and limits from a registry projection.
pub fn public_job(job: &JobView) -> PublicJobSnapshot {
    PublicJobSnapshot {
        id: job.id.to_string(),
        kind: job.kind.clone(),
        label: job.label.clone(),
        status: job.status,
        detail: job_detail(job).map(str::to_owned),
        started_at: job.started_at,
        finished_at: job.finished_at,
    }
}

/// Render generic status with optional detail.
pub fn status_line(status: &str, detail: Option<&str>) -> String {
    match detail {
        Some(detail) => format!("[status: {status}, {detail}]"),
        None => format!("[status: {status}]"),
    }
}

/// `status_line` of a snapshot.
pub fn snapshot_status_line(snapshot: &PublicJobSnapshot) -> String {
    status_line(status_name(snapshot.status), snapshot.detail.as_deref())
}

/// Render one consuming read for the model: stdout and unlabeled chunks in order, then every
/// stderr chunk in one `[stderr]` section, exactly as the shell tools render a foreground result.
/// `log` chunks are producer narration for observers and never reach the model. Lost bytes — the
/// cursor fell behind the ring's retention, or a model-visible chunk carries a producer-side gap —
/// end the read with the shell tools' dropped-output notice, naming the spill files the job's
/// sources keep.
pub fn render_model_delta(chunks: &[JobChunk], lossy: bool, spill_paths: &[String]) -> String {
    let visible: Vec<&JobChunk> = chunks
        .iter()
        .filter(|chunk| chunk.channel != Some(JobChannel::Log))
        .collect();
    let out: String = visible
        .iter()
        .filter(|chunk| chunk.channel != Some(JobChannel::Stderr))
        .map(|chunk| chunk.text.as_str())
        .collect();
    let err: String = visible
        .iter()
        .filter(|chunk| chunk.channel == Some(JobChannel::Stderr))
        .map(|chunk| chunk.text.as_str())
        .collect();
    let separator = if !out.is_empty() && !out.ends_with('\n') {
        "\n"
    } else {
        ""
    };
    let body = if err.is_empty() {
        out
    } else {
        format!("{out}{separator}[stderr]\n{err}")
    };
    if !lossy && !visible.iter().any(|chunk| chunk.gap_before) {
        return body;
    }
    let files = if spill_paths.is_empty() {
        "(unavailable)".to_owned()
    } else {
        spill_paths.join(", ")
    };
    let notice = format!("[some output was dropped from memory; full output: {files}]");
    let gap = if !body.is_empty() && !body.ends_with('\n') {
        "\n"
    } else {
        ""
    };
    format!("{body}{gap}{notice}")
}
