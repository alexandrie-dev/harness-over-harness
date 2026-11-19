//! Result-time contextual diff presentation for write and edit. Storage returns before/after text;
//! this model-facing layer derives one three-line-context card per applied hunk.

use hoh_jsdiff::{PatchOptions, structured_patch};
use hoh_tools::FileDiff;
use serde_json::Value;

/// Context lines shown on each side of an applied hunk.
pub const DIFF_CONTEXT: usize = 3;

/// Compute one [`FileDiff`] per hunk between `before` and `after`, each carrying the applied change
/// plus [`DIFF_CONTEXT`] context lines. Pure insertions use `old_text: None`, patch-only no-newline
/// markers are omitted, and scattered replacements remain separate hunks.
///
/// `path` is stamped on every produced diff (the model-facing `file_path`; the bridge relativizes
/// it); `before` and `after` are the file texts on the backend's LF-normalized diff basis. Returns
/// one diff per applied hunk, in file order; empty when the texts are identical.
pub fn compute_hunk_diffs(path: &str, before: &str, after: &str) -> Vec<FileDiff> {
    let options = PatchOptions {
        context: DIFF_CONTEXT,
        ..PatchOptions::default()
    };
    // No timeout and no edit limit are set, so a patch is always produced.
    let Some(patch) = structured_patch("", "", before, after, None, None, &options) else {
        return Vec::new();
    };
    let mut diffs = Vec::new();
    for hunk in &patch.hunks {
        let mut old_lines: Vec<&str> = Vec::new();
        let mut new_lines: Vec<&str> = Vec::new();
        for line in &hunk.lines {
            // The unified-diff marker for a missing trailing newline annotates the patch, not the
            // content — skip it so it never leaks into a diff block.
            if line.starts_with('\\') {
                continue;
            }
            let mut chars = line.chars();
            chars.next();
            let text = chars.as_str();
            if line.starts_with('-') {
                old_lines.push(text);
            } else if line.starts_with('+') {
                new_lines.push(text);
            } else {
                // A context (unchanged) line appears on both sides.
                old_lines.push(text);
                new_lines.push(text);
            }
        }
        diffs.push(FileDiff {
            path: path.to_owned(),
            old_text: (!old_lines.is_empty()).then(|| old_lines.join("\n")),
            new_text: new_lines.join("\n"),
        });
    }
    diffs
}

/// Whether `value` is a valid [`FileDiff`] (defensive narrowing from opaque `meta`).
fn file_diff(value: &Value) -> Option<FileDiff> {
    let object = value.as_object()?;
    let path = object.get("path")?.as_str()?;
    let old_text = match object.get("oldText")? {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        _ => return None,
    };
    let new_text = object.get("newText")?.as_str()?;
    Some(FileDiff {
        path: path.to_owned(),
        old_text,
        new_text: new_text.to_owned(),
    })
}

/// Narrow opaque live or replayed result metadata to non-empty file diffs. Malformed metadata
/// returns `None` so presentation can fall back instead of failing during replay.
pub fn diffs_from_meta(meta: Option<&Value>) -> Option<Vec<FileDiff>> {
    let diffs = meta?.as_object()?.get("diffs")?.as_array()?;
    if diffs.is_empty() {
        return None;
    }
    diffs.iter().map(file_diff).collect()
}

/// Narrow the patch tool's optional stopped flag from persisted metadata: true only for an explicit
/// stopped flag.
pub fn stopped_from_meta(meta: Option<&Value>) -> bool {
    meta.and_then(Value::as_object)
        .and_then(|object| object.get("stopped"))
        .is_some_and(|stopped| stopped == &Value::Bool(true))
}

/// Read the operation's file path even when the applied diff is empty. `None` for old or malformed
/// metadata.
pub fn path_from_meta(meta: Option<&Value>) -> Option<String> {
    let path = meta?.as_object()?.get("path")?.as_str()?;
    (!path.is_empty()).then(|| path.to_owned())
}
