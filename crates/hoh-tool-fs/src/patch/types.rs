//! Internal patch outcomes, failure diagnostics, and operation-local plans.

use hoh_fs::FsTarget;
use hoh_fs_patch::PatchHunk;
use hoh_sandbox::SandboxExecutionPolicy;

/// One request after the caller resolves its per-call sandbox policy.
#[derive(Clone, Debug)]
pub struct PatchExecutionRequest {
    /// Raw patch text from the model.
    pub patch: String,
    /// Resolved per-call policy; `None` under an unconfined backend.
    pub sandbox_policy: Option<SandboxExecutionPolicy>,
}

/// File effect represented by one operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchOperationKind {
    Add,
    Update,
    Delete,
    Move,
}

impl PatchOperationKind {
    /// The wire spelling (`add`, `update`, `delete`, `move`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Update => "update",
            Self::Delete => "delete",
            Self::Move => "move",
        }
    }
}

/// One file operation; unresolved paths retain their original patch spelling on resolution failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchOperationRecord {
    /// Zero-based position in the patch.
    pub index: usize,
    /// File effect represented by this operation.
    pub kind: PatchOperationKind,
    /// Source display path (created path for add), or raw patch path if resolution failed.
    pub path: String,
    /// Move destination display path, or raw patch path if it could not be resolved.
    pub destination: Option<String>,
    /// One-based patch line of the operation header.
    pub patch_line: u64,
}

/// LF text before and after one effect; `None` means absence or an unavailable provider basis.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchFileChange {
    /// Canonical absolute process path that changed.
    pub path: String,
    /// Previous text, or `None` for absence or an unavailable basis.
    pub before: Option<String>,
    /// Published text, or `None` after deletion.
    pub after: Option<String>,
}

/// Actionable diagnostic ready for model output. `code` is a filesystem error code or one of
/// `PATCH_SYNTAX` / `PATCH_SYMLINK_SOURCE`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchFailure {
    /// Stable machine-readable reason.
    pub code: String,
    /// Message after sandbox denial mapping and filesystem remediation.
    pub message: String,
    /// Patch line of an unmatched hunk.
    pub hunk_line: Option<u64>,
}

impl PatchFailure {
    /// A failure without a hunk line.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            hunk_line: None,
        }
    }
}

/// One operation refused before any write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchRefusal {
    pub operation: PatchOperationRecord,
    pub failure: PatchFailure,
}

/// Each operation retains completed effects even when a move fails halfway.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatchOperationOutcome {
    Completed {
        operation: PatchOperationRecord,
        changes: Vec<PatchFileChange>,
    },
    Failed {
        operation: PatchOperationRecord,
        failure: PatchFailure,
        changes: Vec<PatchFileChange>,
    },
    Unknown {
        operation: PatchOperationRecord,
        failure: PatchFailure,
    },
    Skipped {
        operation: PatchOperationRecord,
    },
}

impl PatchOperationOutcome {
    /// The operation record of any outcome.
    pub fn operation(&self) -> &PatchOperationRecord {
        match self {
            Self::Completed { operation, .. }
            | Self::Failed { operation, .. }
            | Self::Unknown { operation, .. }
            | Self::Skipped { operation } => operation,
        }
    }

    /// The wire status (`completed`, `failed`, `unknown`, `skipped`).
    pub fn status(&self) -> &'static str {
        match self {
            Self::Completed { .. } => "completed",
            Self::Failed { .. } => "failed",
            Self::Unknown { .. } => "unknown",
            Self::Skipped { .. } => "skipped",
        }
    }

    /// The failure of a failed or unknown outcome.
    pub fn failure(&self) -> Option<&PatchFailure> {
        match self {
            Self::Failed { failure, .. } | Self::Unknown { failure, .. } => Some(failure),
            _ => None,
        }
    }

    /// The known changes of a completed or failed outcome (`None` for unknown/skipped).
    pub fn changes(&self) -> Option<&[PatchFileChange]> {
        match self {
            Self::Completed { changes, .. } | Self::Failed { changes, .. } => Some(changes),
            _ => None,
        }
    }
}

/// Syntax and preflight refusals prevent all writes; a commit failure stops later operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatchExecutionReport {
    Syntax {
        failure: PatchFailure,
    },
    /// Always non-empty.
    Preflight {
        refusals: Vec<PatchRefusal>,
    },
    Commit {
        outcomes: Vec<PatchOperationOutcome>,
        affected_paths: Vec<String>,
        cancelled: bool,
    },
}

/// What a resolved operation does, with its operation-local payload.
#[derive(Clone, Debug)]
pub enum PlannedKind {
    Add {
        content: String,
    },
    Update {
        hunks: Vec<PatchHunk>,
    },
    Delete,
    Move {
        destination: FsTarget,
        hunks: Vec<PatchHunk>,
    },
}

/// Resolved operation with raw source spelling retained for no-follow inspection.
#[derive(Clone, Debug)]
pub struct PlannedOperation {
    pub record: PatchOperationRecord,
    pub target: FsTarget,
    pub raw_path: String,
    pub kind: PlannedKind,
}

/// Resolved operations and path-resolution refusals in one parsed patch.
#[derive(Clone, Debug, Default)]
pub struct PatchPlan {
    /// Resolved operations in patch order.
    pub operations: Vec<PlannedOperation>,
    /// Resolution refusals; unresolvable paths retain their raw patch spelling.
    pub refusals: Vec<PatchRefusal>,
}
