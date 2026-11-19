//! Public workspace-change summaries and lazy per-file comparisons.

use futures_util::future::BoxFuture;
use hoh_error::BoxError;
use hoh_session::SessionId;
use hoh_timeout::AbortSignal;
use serde::{Deserialize, Serialize};

fn is_false(value: &bool) -> bool {
    !*value
}

/// One file changed during a turn.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChangedFile {
    /// Path relative to the Session working directory, or an absolute Host path outside it.
    pub path: String,
    /// Slash-separated sort key and user-facing label.
    pub display: String,
    /// Added lines, or zero for binary/oversized content.
    pub added: u64,
    /// Deleted lines, or zero for binary/oversized content.
    pub deleted: u64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub binary: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub oversized: bool,
}

/// Git trees bounding a turn.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceSnapshot {
    pub before: String,
    pub after: String,
}

/// Files changed during one top-level turn.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChangesSummary {
    pub turn: u64,
    pub cwd: String,
    pub files: Vec<WorkspaceChangedFile>,
    pub total: u64,
    pub added: u64,
    pub deleted: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<WorkspaceSnapshot>,
}

/// One unified-diff hunk with three context lines.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDiffHunk {
    pub old_start: u64,
    pub old_lines: u64,
    pub new_start: u64,
    pub new_lines: u64,
    pub lines: Vec<String>,
}

/// Comparison of one listed file's turn-start and turn-end contents.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WorkspaceFileDiff {
    Text {
        path: String,
        display: String,
        before: bool,
        after: bool,
        hunks: Vec<WorkspaceDiffHunk>,
        coarse: bool,
    },
    Binary {
        path: String,
        display: String,
    },
    Oversized {
        path: String,
        display: String,
    },
}

/// Process-local service for live Session summaries and comparisons.
pub trait WorkspaceChangesApi: Send + Sync {
    fn summary(&self, session_id: &SessionId, seq: u64) -> Option<WorkspaceChangesSummary>;
    fn diff<'a>(
        &'a self,
        session_id: &'a SessionId,
        seq: u64,
        index: usize,
        signal: AbortSignal,
    ) -> BoxFuture<'a, Result<Option<WorkspaceFileDiff>, BoxError>>;
}

hoh_kernel::service!(pub WorkspaceChanges: dyn WorkspaceChangesApi = "workspaceChanges");
