//! Port of structuredPatch and its line diff from jsdiff 9.0.0 (npm package diff).

mod diff;
mod patch;

pub use diff::line::diff_lines;
pub use patch::create::{create_patch, create_two_files_patch, format_patch, structured_patch};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredPatch {
    pub old_file_name: String,
    pub new_file_name: String,
    pub old_header: Option<String>,
    pub new_header: Option<String>,
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchOptions {
    pub context: usize,
    pub timeout_ms: Option<u64>,
    pub max_edit_length: Option<usize>,
}

impl Default for PatchOptions {
    fn default() -> Self {
        Self {
            context: 4,
            timeout_ms: None,
            max_edit_length: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub value: String,
    pub added: bool,
    pub removed: bool,
    pub count: usize,
}
