//! Public file-reference discovery and mention-token records.

use serde::{Deserialize, Serialize};

/// Active `@` token ending at the editor cursor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveAtToken {
    /// Complete token replaced when the user accepts a completion.
    pub prefix: String,
    /// Path query after `@` or `@"`.
    pub query: String,
    /// Whether the user opened a quoted path.
    pub quoted: bool,
}

/// Whether one path-only completion candidate is a file or directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileReferenceKind {
    File,
    Directory,
}

/// One path-only completion candidate inside the target session working directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReferenceCandidate {
    /// User-facing path accepted by normal prompts and filesystem tools.
    pub path: String,
    /// Directories keep completion open; files finish the mention.
    pub kind: FileReferenceKind,
}
