//! Pure patch call previews and known applied diff metadata for Host and Web replay.

use std::collections::HashSet;

use hoh_fs_patch::{PatchOperation, parse_patch};
use hoh_tools::{DiffCallView, DiffResultView, FileDiff, FileLocation, ToolResult, ToolResultView};

use crate::diff::{diffs_from_meta, stopped_from_meta};

/// Preview the requested operations without resolving paths or reading the filesystem.
/// A deletion has no diff text to preview and appears only among the locations.
///
/// `patch` is the raw patch argument. Returns a diff call view, or `None` when syntax is invalid.
pub fn present_patch_call(patch: &str) -> Option<DiffCallView> {
    let operations = parse_patch(patch).ok()?;
    let mut diffs = Vec::new();
    let mut paths: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut add_path = |path: &str| {
        if seen.insert(path.to_owned()) {
            paths.push(path.to_owned());
        }
    };
    for operation in &operations {
        match operation {
            PatchOperation::Add { path, content, .. } => {
                diffs.push(FileDiff {
                    path: path.clone(),
                    old_text: None,
                    new_text: content.clone(),
                });
                add_path(path);
            }
            PatchOperation::Delete { path, .. } => add_path(path),
            PatchOperation::Update {
                path,
                hunks,
                move_to,
                ..
            } => {
                let old: Vec<&str> = hunks
                    .iter()
                    .flat_map(|hunk| hunk.old_lines.iter().map(String::as_str))
                    .collect();
                let new: Vec<&str> = hunks
                    .iter()
                    .flat_map(|hunk| hunk.new_lines.iter().map(String::as_str))
                    .collect();
                let old_text = old.join("\n");
                diffs.push(FileDiff {
                    path: path.clone(),
                    old_text: (!old_text.is_empty()).then_some(old_text),
                    new_text: new.join("\n"),
                });
                add_path(path);
                if let Some(move_to) = move_to {
                    add_path(move_to);
                }
            }
        }
    }
    Some(DiffCallView {
        title: format!(
            "Apply patch ({} {})",
            paths.len(),
            if paths.len() == 1 { "file" } else { "files" }
        ),
        diffs,
        locations: Some(
            paths
                .into_iter()
                .map(|path| FileLocation { path, line: None })
                .collect(),
        ),
    })
}

/// Present applied metadata; failed or missing metadata uses generic result rendering.
///
/// `result` is the live or replayed tool result. Returns the known applied hunks with a stopped
/// title where appropriate.
pub fn present_patch_result(result: &ToolResult) -> Option<ToolResultView> {
    if result.is_error {
        return None;
    }
    let diffs = diffs_from_meta(result.meta.as_ref())?;
    let title = if stopped_from_meta(result.meta.as_ref()) {
        "Apply patch (stopped)"
    } else {
        "Apply patch"
    };
    Some(ToolResultView::Diff(DiffResultView {
        title: Some(title.to_owned()),
        diffs,
    }))
}
