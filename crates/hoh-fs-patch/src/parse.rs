// Adapted from DSH's adaptation of OpenAI Codex rust-v0.158.0 parser.rs and
// streaming_parser.rs. Copyright 2025 OpenAI; Apache-2.0. Modifications: Rust port.
use crate::types::{PatchHunk, PatchOperation};

const BEGIN: &str = "*** Begin Patch";
const END: &str = "*** End Patch";
const EOF: &str = "*** End of File";
const MOVE: &str = "*** Move to: ";
const ADD: &str = "*** Add File: ";
const DELETE: &str = "*** Delete File: ";
const UPDATE: &str = "*** Update File: ";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Invalid patch at line {line_number}: {reason}")]
pub struct PatchParseError {
    pub line_number: u64,
    pub reason: String,
}

impl PatchParseError {
    pub fn new(line_number: u64, reason: impl Into<String>) -> Self {
        Self {
            line_number,
            reason: reason.into(),
        }
    }
}

struct InputLine<'a> {
    text: &'a str,
    number: u64,
}

fn patch_lines(patch: &str) -> Result<Vec<InputLine<'_>>, PatchParseError> {
    let leading = &patch[..patch.len() - patch.trim_start().len()];
    let offset = leading.bytes().filter(|b| *b == b'\n').count() as u64;
    let mut lines: Vec<_> = patch
        .trim()
        .split('\n')
        .enumerate()
        .map(|(index, text)| InputLine {
            text: text.strip_suffix('\r').unwrap_or(text),
            number: offset + index as u64 + 1,
        })
        .collect();
    if lines.len() >= 4
        && matches!(lines[0].text, "<<EOF" | "<<'EOF'" | "<<\"EOF\"")
        && lines.last().is_some_and(|line| line.text.ends_with("EOF"))
    {
        lines.pop();
        lines.remove(0);
    }
    // split always yields a line, and wrapper removal retains at least two.
    let first = &lines[0];
    if first.text.trim() != BEGIN {
        return Err(PatchParseError::new(
            first.number,
            format!("Expected '{BEGIN}'"),
        ));
    }
    let last = &lines[lines.len() - 1];
    if last.text.trim() != END {
        return Err(PatchParseError::new(
            last.number,
            format!("Expected '{END}'"),
        ));
    }
    let len = lines.len();
    for line in &mut lines[..len - 1] {
        line.text = line.text.strip_suffix('\r').unwrap_or(line.text);
    }
    Ok(lines)
}

fn empty_hunk(hunk: &PatchHunk) -> bool {
    hunk.old_lines.is_empty() && hunk.new_lines.is_empty()
}

fn ensure_update(
    operation: Option<&PatchOperation>,
    line_number: u64,
) -> Result<(), PatchParseError> {
    if let Some(PatchOperation::Update {
        path,
        line_number: origin,
        hunks,
        ..
    }) = operation
    {
        let last = hunks.last().ok_or_else(|| {
            PatchParseError::new(
                *origin,
                format!("Update for '{path}' requires at least one hunk"),
            )
        })?;
        if empty_hunk(last) {
            return Err(PatchParseError::new(
                line_number,
                "Update hunk must contain context, added, or removed lines",
            ));
        }
    }
    Ok(())
}

fn new_hunk(line_number: u64) -> PatchHunk {
    PatchHunk {
        line_number,
        context: None,
        old_lines: vec![],
        new_lines: vec![],
        is_end_of_file: false,
    }
}

fn read_update(
    move_to: &mut Option<String>,
    hunks: &mut Vec<PatchHunk>,
    line: &InputLine<'_>,
) -> Result<(), PatchParseError> {
    let text = line.text.trim_end();
    let marker = text == "@@" || text.starts_with("@@ ");
    if hunks.last().is_some_and(|hunk| hunk.is_end_of_file) {
        if text.is_empty() {
            return Ok(());
        }
        if !marker {
            return Err(PatchParseError::new(
                line.number,
                "Expected '@@' after '*** End of File'",
            ));
        }
    }
    if hunks.is_empty()
        && move_to.is_none()
        && let Some(path) = text.strip_prefix(MOVE)
    {
        *move_to = Some(path.into());
        return Ok(());
    }
    if marker {
        if hunks.last().is_some_and(empty_hunk) {
            return Err(PatchParseError::new(
                line.number,
                "Update hunk must contain context, added, or removed lines",
            ));
        }
        let mut hunk = new_hunk(line.number);
        hunk.context = text.strip_prefix("@@ ").map(String::from);
        hunks.push(hunk);
        return Ok(());
    }
    if text == EOF {
        if let Some(previous) = hunks.last_mut() {
            if empty_hunk(previous) {
                return Err(PatchParseError::new(
                    line.number,
                    "Update hunk must contain context, added, or removed lines",
                ));
            }
            previous.is_end_of_file = true;
        }
        return Ok(());
    }
    let prefix = line.text.as_bytes().first().copied();
    if !line.text.is_empty() && !matches!(prefix, Some(b' ' | b'+' | b'-')) {
        return Err(PatchParseError::new(
            line.number,
            "Expected a context (' '), added ('+'), removed ('-') line, or '@@'",
        ));
    }
    if hunks.is_empty() {
        hunks.push(new_hunk(line.number));
    }
    let index = hunks.len() - 1;
    let hunk = &mut hunks[index];
    let content = line.text.get(1..).unwrap_or("").to_owned();
    match prefix {
        Some(b'+') => hunk.new_lines.push(content),
        Some(b'-') => hunk.old_lines.push(content),
        _ => {
            hunk.old_lines.push(content.clone());
            hunk.new_lines.push(content);
        }
    }
    Ok(())
}

/// Parse a complete Codex-style patch without filesystem access.
pub fn parse_patch(patch: &str) -> Result<Vec<PatchOperation>, PatchParseError> {
    let lines = patch_lines(patch)?;
    let mut operations = Vec::new();
    let mut ended = false;
    for (index, line) in lines.iter().enumerate().skip(1) {
        let last = index == lines.len() - 1;
        let header = if matches!(operations.last(), Some(PatchOperation::Update { .. })) && !last {
            line.text.trim_end()
        } else {
            line.text.trim()
        };
        if ended && !last {
            if !line.text.trim().is_empty() {
                return Err(PatchParseError::new(
                    line.number,
                    format!("Unexpected content after '{END}'"),
                ));
            }
            continue;
        }
        if header == END {
            ensure_update(operations.last(), line.number)?;
            ended = true;
        } else if let Some(path) = header.strip_prefix(ADD) {
            ensure_update(operations.last(), line.number)?;
            operations.push(PatchOperation::Add {
                path: path.into(),
                line_number: line.number,
                content: String::new(),
            });
        } else if let Some(path) = header.strip_prefix(DELETE) {
            ensure_update(operations.last(), line.number)?;
            operations.push(PatchOperation::Delete {
                path: path.into(),
                line_number: line.number,
            });
        } else if let Some(path) = header.strip_prefix(UPDATE) {
            ensure_update(operations.last(), line.number)?;
            operations.push(PatchOperation::Update {
                path: path.into(),
                line_number: line.number,
                move_to: None,
                hunks: vec![],
            });
        } else if operations.is_empty() && header.starts_with("*** Environment ID:") {
            return Err(PatchParseError::new(
                line.number,
                "Environment ID headers are unsupported; the caller selects the filesystem provider",
            ));
        } else {
            match operations.last_mut() {
                Some(PatchOperation::Add { content, .. }) if line.text.starts_with('+') => {
                    content.push_str(&line.text[1..]);
                    content.push('\n');
                }
                Some(PatchOperation::Update { move_to, hunks, .. }) => {
                    read_update(move_to, hunks, line)?
                }
                _ => {
                    return Err(PatchParseError::new(
                        line.number,
                        "Expected an Add File, Delete File, or Update File header; added content must start with +",
                    ));
                }
            }
        }
    }
    Ok(operations)
}
