// Adapted from DSH's adaptation of OpenAI Codex rust-v0.158.0 seek_sequence.rs
// and file_update.rs. Copyright 2025 OpenAI; Apache-2.0. Modifications: Rust port.
use crate::types::{ApplyHunksResult, FailureStage, HunkApplyFailure, PatchHunk};

fn normalize(value: &str, pass: u8) -> String {
    match pass {
        0 => value.into(),
        1 => value.trim_end().into(),
        2 => value.trim().into(),
        _ => value
            .trim()
            .chars()
            .map(|c| match c {
                '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
                '\u{2018}'..='\u{201b}' => '\'',
                '\u{201c}'..='\u{201f}' => '"',
                '\u{a0}' | '\u{2002}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}' => ' ',
                _ => c,
            })
            .collect(),
    }
}

fn seek_sequence(lines: &[String], pattern: &[String], start: usize, eof: bool) -> Option<usize> {
    if pattern.is_empty() {
        return Some(start);
    }
    if pattern.len() > lines.len() {
        return None;
    }
    let last = lines.len() - pattern.len();
    let first = if eof { last } else { start };
    for pass in 0..4 {
        let expected: Vec<_> = pattern.iter().map(|s| normalize(s, pass)).collect();
        for index in first..=last {
            if expected
                .iter()
                .enumerate()
                .all(|(offset, line)| normalize(&lines[index + offset], pass) == *line)
            {
                return Some(index);
            }
        }
    }
    None
}

/// Apply hunks using only the original text as the search input.
pub fn apply_hunks(before: &str, hunks: &[PatchHunk]) -> ApplyHunksResult {
    let mut original: Vec<String> = before.split('\n').map(String::from).collect();
    if original.last().is_some_and(String::is_empty) {
        original.pop();
    }
    let mut replacements = Vec::new();
    let mut line_index = 0;
    for hunk in hunks {
        if let Some(context) = &hunk.context {
            let pattern = vec![context.clone()];
            let Some(found) = seek_sequence(&original, &pattern, line_index, false) else {
                return ApplyHunksResult::Failure {
                    failure: HunkApplyFailure {
                        hunk_line_number: hunk.line_number,
                        stage: FailureStage::Context,
                        search_start: line_index as u64,
                        pattern,
                    },
                };
            };
            line_index = found + 1;
        }
        if hunk.old_lines.is_empty() {
            let start = original.len() - usize::from(original.last().is_some_and(String::is_empty));
            replacements.push((start, 0, hunk.new_lines.as_slice()));
            continue;
        }
        let mut pattern = hunk.old_lines.as_slice();
        let mut new_lines = hunk.new_lines.as_slice();
        let mut found = seek_sequence(&original, pattern, line_index, hunk.is_end_of_file);
        if found.is_none() && pattern.last().is_some_and(String::is_empty) {
            pattern = &pattern[..pattern.len() - 1];
            if new_lines.last().is_some_and(String::is_empty) {
                new_lines = &new_lines[..new_lines.len() - 1];
            }
            found = seek_sequence(&original, pattern, line_index, hunk.is_end_of_file);
        }
        let Some(found) = found else {
            return ApplyHunksResult::Failure {
                failure: HunkApplyFailure {
                    hunk_line_number: hunk.line_number,
                    stage: FailureStage::OldLines,
                    search_start: line_index as u64,
                    pattern: hunk.old_lines.clone(),
                },
            };
        };
        replacements.push((found, pattern.len(), new_lines));
        line_index = found + pattern.len();
    }
    replacements.sort_by_key(|replacement| replacement.0);
    let mut lines = original;
    for (start, old_length, new_lines) in replacements.into_iter().rev() {
        // JS slice clamps indices after overlapping replacements shrink the text.
        let start_clamped = start.min(lines.len());
        let end = (start + old_length).min(lines.len());
        lines.splice(start_clamped..end, new_lines.iter().cloned());
    }
    if !lines.last().is_some_and(String::is_empty) {
        lines.push(String::new());
    }
    ApplyHunksResult::Success {
        after: lines.join("\n"),
    }
}
