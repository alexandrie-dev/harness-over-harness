//! Canonical patch results, typed refusals, and compact model output.

use hoh_error::{BoxError, HarnessError};
use hoh_fs::FsError;
use serde_json::{Map, Value, json};

use crate::diff::compute_hunk_diffs;
use crate::patch::{PatchExecutionReport, PatchOperationOutcome, PatchRefusal};

/// Patch syntax and symbolic-link refusals retain their own stable codes.
pub struct PatchToolError;

impl PatchToolError {
    /// The error name carried by [`HarnessError`].
    pub const NAME: &str = "PatchToolError";

    /// A patch refusal with its stable code (`PATCH_SYNTAX` or `PATCH_SYMLINK_SOURCE`).
    #[allow(clippy::new_ret_no_self)]
    pub fn new(message: impl Into<String>, code: impl Into<String>) -> HarnessError {
        HarnessError::named(Self::NAME, message, code)
    }
}

/// Strict canonical output; full file texts feed rendering and are excluded from presentation
/// metadata.
pub fn patch_output_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false,
        "properties": {
            "phase": { "type": "string", "const": "commit", "required": true },
            "cancelled": { "type": "boolean", "required": true },
            "operations": { "type": "array", "required": true, "items": {
                "type": "object", "additionalProperties": false,
                "properties": {
                    "index": { "type": "integer", "required": true },
                    "kind": { "type": "string", "enum": ["add", "update", "delete", "move"], "required": true },
                    "path": { "type": "string", "required": true },
                    "destination": { "type": "string" },
                    "patchLine": { "type": "integer", "required": true },
                    "status": { "type": "string", "enum": ["completed", "failed", "unknown", "skipped"], "required": true },
                    "code": { "type": "string" },
                    "message": { "type": "string" },
                    "hunkLine": { "type": "integer" },
                    "changes": { "type": "array", "items": {
                        "type": "object", "additionalProperties": false,
                        "properties": {
                            "path": { "type": "string", "required": true },
                            "before": { "required": true, "oneOf": [{ "type": "string" }, { "type": "null" }] },
                            "after": { "required": true, "oneOf": [{ "type": "string" }, { "type": "null" }] },
                        },
                    } },
                },
            } },
            "affectedPaths": { "type": "array", "items": { "type": "string" }, "required": true },
        },
    })
}

fn rejection(code: &str, message: String) -> HarnessError {
    if code == "PATCH_SYNTAX" || code == "PATCH_SYMLINK_SOURCE" {
        PatchToolError::new(message, code)
    } else {
        HarnessError::named(FsError::NAME, message, code)
    }
}

fn refusal_error(refusals: &[PatchRefusal]) -> HarnessError {
    let lines: Vec<String> = refusals
        .iter()
        .map(|PatchRefusal { operation, failure }| {
            format!(
                "- operation {} ({} {}, patch line {}): {}",
                operation.index + 1,
                operation.kind.as_str(),
                operation.path,
                operation.patch_line,
                failure.message
            )
        })
        .collect();
    rejection(
        &refusals[0].failure.code,
        format!(
            "Patch rejected before any change ({} problems):\n{}",
            refusals.len(),
            lines.join("\n")
        ),
    )
}

/// Convert a report into a commit value, or reject while no disk change exists.
/// A commit whose first operation failed without any change, such as a provider sandbox
/// denial, rejects like a preflight refusal so the error code reaches the same retry paths.
///
/// Returns a flat, lossless JSON commit value, including partial failures. Fails with an
/// `FsError` or [`PatchToolError`] for syntax, preflight, or first-operation refusals.
pub fn patch_tool_value(report: &PatchExecutionReport) -> Result<Value, BoxError> {
    match report {
        PatchExecutionReport::Syntax { failure } => {
            Err(rejection(&failure.code, failure.message.clone()).into())
        }
        PatchExecutionReport::Preflight { refusals } => Err(refusal_error(refusals).into()),
        PatchExecutionReport::Commit {
            outcomes,
            affected_paths,
            cancelled,
        } => {
            if let Some((first, rest)) = outcomes.split_first()
                && let PatchOperationOutcome::Failed {
                    operation,
                    failure,
                    changes,
                } = first
                && changes.is_empty()
                && rest.iter().all(|outcome| outcome.status() == "skipped")
            {
                return Err(refusal_error(&[PatchRefusal {
                    operation: operation.clone(),
                    failure: failure.clone(),
                }])
                .into());
            }
            let operations: Vec<Value> = outcomes
                .iter()
                .map(|outcome| {
                    let operation = outcome.operation();
                    let mut entry = Map::new();
                    entry.insert("index".into(), json!(operation.index));
                    entry.insert("kind".into(), json!(operation.kind.as_str()));
                    entry.insert("path".into(), json!(operation.path));
                    entry.insert("patchLine".into(), json!(operation.patch_line));
                    if let Some(destination) = &operation.destination {
                        entry.insert("destination".into(), json!(destination));
                    }
                    entry.insert("status".into(), json!(outcome.status()));
                    if let Some(failure) = outcome.failure() {
                        entry.insert("code".into(), json!(failure.code));
                        entry.insert("message".into(), json!(failure.message));
                        if let Some(line) = failure.hunk_line {
                            entry.insert("hunkLine".into(), json!(line));
                        }
                    }
                    if let Some(changes) = outcome.changes() {
                        entry.insert(
                            "changes".into(),
                            Value::Array(
                                changes
                                    .iter()
                                    .map(|change| {
                                        json!({
                                            "path": change.path,
                                            "before": change.before,
                                            "after": change.after,
                                        })
                                    })
                                    .collect(),
                            ),
                        );
                    }
                    Value::Object(entry)
                })
                .collect();
            Ok(json!({
                "phase": "commit",
                "cancelled": cancelled,
                "affectedPaths": affected_paths,
                "operations": operations,
            }))
        }
    }
}

fn operations_of(value: &Value) -> &[Value] {
    value
        .get("operations")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn text_of<'a>(operation: &'a Value, key: &str) -> Option<&'a str> {
    operation.get(key).and_then(Value::as_str)
}

fn status_of(operation: &Value) -> &str {
    text_of(operation, "status").unwrap_or("")
}

/// Identify a stopped commit, including cancellation before the next operation.
///
/// `value` is the canonical patch result. Returns whether the patch did not finish every
/// operation.
pub fn patch_stopped(value: &Value) -> bool {
    value
        .get("cancelled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || operations_of(value)
            .iter()
            .any(|operation| status_of(operation) != "completed")
}

fn completed_lines(operations: &[Value]) -> Vec<String> {
    // Codex groups A/M/D, and records a completed move as M at its destination path.
    ["add", "update", "delete"]
        .into_iter()
        .flat_map(|kind| {
            operations
                .iter()
                .filter(move |operation| {
                    let operation_kind = text_of(operation, "kind").unwrap_or("");
                    status_of(operation) == "completed"
                        && (if operation_kind == "move" {
                            "update"
                        } else {
                            operation_kind
                        }) == kind
                })
                .map(move |operation| {
                    let letter = match kind {
                        "add" => "A",
                        "delete" => "D",
                        _ => "M",
                    };
                    let shown = text_of(operation, "destination")
                        .or_else(|| text_of(operation, "path"))
                        .unwrap_or("");
                    format!("{letter} {shown}")
                })
        })
        .collect()
}

/// Render completed paths and actionable stopped-operation diagnostics.
///
/// `value` is the canonical patch result; paths already use the executor's display spelling.
/// Returns concise model-visible text without whole-file contents.
pub fn render_patch(value: &Value) -> String {
    let operations = operations_of(value);
    if !patch_stopped(value) {
        return if operations.is_empty() {
            "Success. No operations in patch.".to_owned()
        } else {
            let mut lines = vec!["Success. Updated the following files:".to_owned()];
            lines.extend(completed_lines(operations));
            lines.join("\n")
        };
    }
    let completed = operations
        .iter()
        .filter(|operation| status_of(operation) == "completed")
        .count();
    let mut lines = vec![format!(
        "Patch stopped after {completed} of {} operations.",
        operations.len()
    )];
    if completed > 0 {
        lines.push("Completed:".to_owned());
        lines.extend(completed_lines(operations));
    }
    for (status, title) in [
        ("failed", "Failed:"),
        ("unknown", "Outcome unknown:"),
        ("skipped", "Not executed:"),
    ] {
        let matching: Vec<&Value> = operations
            .iter()
            .filter(|operation| status_of(operation) == status)
            .collect();
        if matching.is_empty() {
            continue;
        }
        lines.push(title.to_owned());
        for operation in matching {
            let index = operation.get("index").and_then(Value::as_u64).unwrap_or(0);
            let kind = text_of(operation, "kind").unwrap_or("");
            let path = text_of(operation, "path").unwrap_or("");
            let label = format!("- operation {} ({kind} {path}", index + 1);
            if status == "skipped" {
                lines.push(format!("{label})"));
                continue;
            }
            let destination = text_of(operation, "destination");
            let arrow =
                destination.map_or_else(String::new, |destination| format!(" -> {destination}"));
            let patch_line = operation
                .get("patchLine")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            // JS interpolates an absent message as "undefined".
            let message = text_of(operation, "message").unwrap_or("undefined");
            lines.push(format!(
                "{label}{arrow}, patch line {patch_line}): {message}"
            ));
            if status == "failed"
                && kind == "move"
                && operation
                    .get("changes")
                    .and_then(Value::as_array)
                    .is_some_and(|changes| !changes.is_empty())
            {
                lines.push(format!(
                    "  destination {} was written; source {path} remains",
                    destination.unwrap_or("undefined")
                ));
            }
        }
    }
    lines.join("\n")
}

/// Derive hunks only from known completed effects, including a partial move destination.
/// Creations store no diff, as for `write`; a removal without a bounded basis stores none either.
///
/// `value` is the canonical commit value containing the provider's bounded baselines. Returns
/// serializable diff hunks and an explicit stopped flag; no whole-file change records.
pub fn patch_presentation_meta(value: &Value) -> Value {
    let mut diffs: Vec<Value> = Vec::new();
    for operation in operations_of(value) {
        if !matches!(status_of(operation), "completed" | "failed") {
            continue;
        }
        let changes = operation
            .get("changes")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        for change in changes {
            let path = text_of(change, "path").unwrap_or("");
            let Some(before) = text_of(change, "before") else {
                continue;
            };
            let file_diffs = match text_of(change, "after") {
                None => vec![hoh_tools::FileDiff {
                    path: path.to_owned(),
                    old_text: Some(before.to_owned()),
                    new_text: String::new(),
                }],
                Some(after) => compute_hunk_diffs(path, before, after),
            };
            for diff in file_diffs {
                diffs.push(json!({ "path": diff.path, "oldText": diff.old_text, "newText": diff.new_text }));
            }
        }
    }
    json!({ "diffs": diffs, "stopped": patch_stopped(value) })
}
