//! Pure patch parsing and text application, independent of filesystem providers.
pub mod apply;
pub mod parse;
pub mod types;

pub use apply::apply_hunks;
pub use parse::{PatchParseError, parse_patch};
pub use types::{ApplyHunksResult, HunkApplyFailure, PatchHunk, PatchOperation};
