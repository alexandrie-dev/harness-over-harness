//! File-reference discovery seam and shared `@file` mention grammar.
//!
//! This is the port of `@deepseek-ai/dsh-file-reference`. It deliberately owns
//! no filesystem access; provider crates publish an implementation of
//! [`FileReferenceService`] under the [`FileReferences`] service key.

pub mod grammar;
pub mod types;

pub use grammar::{active_at_token, format_file_mention};
pub use types::{ActiveAtToken, FileReferenceCandidate, FileReferenceKind};

use std::sync::Arc;

use futures_util::future::BoxFuture;
use hoh_agent::Agent;
use hoh_error::BoxError;
use hoh_timeout::AbortSignal;

/// Model guidance for path-only references selected by a user interface.
pub const FILE_REFERENCE_PROMPT: &str = "Tokens prefixed with @ are paths the user explicitly referenced. Relative paths resolve from the workspace root; absolute paths identify files or directories on the host. A trailing slash marks a directory: list it when its contents matter. Anything else is a file: use the read tool when its contents are needed, and do not claim to have inspected it before reading. @\"...\" quotes a path containing spaces.";

/// Host capability for cancellable file-reference discovery.
pub trait FileReferenceService: Send + Sync + 'static {
    /// List file and directory candidates for one agent's working directory.
    fn list<'a>(
        &'a self,
        agent: &'a Arc<dyn Agent>,
        query: &'a str,
        signal: &'a AbortSignal,
    ) -> BoxFuture<'a, Result<Vec<FileReferenceCandidate>, BoxError>>;
}

hoh_kernel::service!(pub FileReferences: dyn FileReferenceService = "fileReferences");
