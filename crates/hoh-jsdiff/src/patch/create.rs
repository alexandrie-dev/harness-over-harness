use crate::{Hunk, PatchOptions, StructuredPatch, diff_lines};

struct PatchChange {
    added: bool,
    removed: bool,
    lines: Vec<String>,
}

/// Computes a structured unified patch, returning `None` when a diff limit is hit.
pub fn structured_patch(
    old_file_name: &str,
    new_file_name: &str,
    old_str: &str,
    new_str: &str,
    old_header: Option<&str>,
    new_header: Option<&str>,
    options: &PatchOptions,
) -> Option<StructuredPatch> {
    let diff = diff_lines(old_str, new_str, options)?;
    let mut changes: Vec<PatchChange> = diff
        .into_iter()
        .map(|change| PatchChange {
            added: change.added,
            removed: change.removed,
            lines: split_lines(&change.value),
        })
        .collect();
    changes.push(PatchChange {
        added: false,
        removed: false,
        lines: Vec::new(),
    });

    let mut hunks = Vec::new();
    let mut old_range_start = 0;
    let mut new_range_start = 0;
    let mut cur_range: Vec<String> = Vec::new();
    let mut old_line = 1;
    let mut new_line = 1;

    for (i, current) in changes.iter().enumerate() {
        let lines = &current.lines;
        if current.added || current.removed {
            if old_range_start == 0 {
                old_range_start = old_line;
                new_range_start = new_line;

                if let Some(previous) = i.checked_sub(1).and_then(|index| changes.get(index))
                    && options.context > 0
                {
                    let first = previous.lines.len().saturating_sub(options.context);
                    cur_range = previous.lines[first..]
                        .iter()
                        .map(|entry| format!(" {entry}"))
                        .collect();
                    old_range_start -= cur_range.len();
                    new_range_start -= cur_range.len();
                }
            }

            for line in lines {
                cur_range.push(format!("{}{line}", if current.added { '+' } else { '-' }));
            }

            if current.added {
                new_line += lines.len();
            } else {
                old_line += lines.len();
            }
        } else {
            if old_range_start != 0 {
                if lines.len() <= options.context.saturating_mul(2) && i + 2 < changes.len() {
                    for line in lines {
                        cur_range.push(format!(" {line}"));
                    }
                } else {
                    let context_size = lines.len().min(options.context);
                    for line in &lines[..context_size] {
                        cur_range.push(format!(" {line}"));
                    }

                    hunks.push(Hunk {
                        old_start: old_range_start,
                        old_lines: old_line - old_range_start + context_size,
                        new_start: new_range_start,
                        new_lines: new_line - new_range_start + context_size,
                        lines: std::mem::take(&mut cur_range),
                    });

                    old_range_start = 0;
                    new_range_start = 0;
                }
            }
            old_line += lines.len();
            new_line += lines.len();
        }
    }

    for hunk in &mut hunks {
        let mut i = 0;
        while i < hunk.lines.len() {
            if hunk.lines[i].ends_with('\n') {
                hunk.lines[i].pop();
            } else {
                hunk.lines
                    .insert(i + 1, "\\ No newline at end of file".to_owned());
                i += 1;
            }
            i += 1;
        }
    }

    Some(StructuredPatch {
        old_file_name: old_file_name.to_owned(),
        new_file_name: new_file_name.to_owned(),
        old_header: old_header.map(str::to_owned),
        new_header: new_header.map(str::to_owned),
        hunks,
    })
}

/// Serializes a structured patch using jsdiff's default header form.
pub fn format_patch(patch: &StructuredPatch) -> String {
    let mut ret = Vec::new();

    if patch.old_file_name == patch.new_file_name {
        ret.push(format!("Index: {}", patch.old_file_name));
    }
    ret.push("===================================================================".to_owned());
    ret.push(format!(
        "--- {}{}",
        quote_file_name_if_needed(&patch.old_file_name),
        patch
            .old_header
            .as_deref()
            .filter(|header| !header.is_empty())
            .map_or(String::new(), |header| format!("\t{header}"))
    ));
    ret.push(format!(
        "+++ {}{}",
        quote_file_name_if_needed(&patch.new_file_name),
        patch
            .new_header
            .as_deref()
            .filter(|header| !header.is_empty())
            .map_or(String::new(), |header| format!("\t{header}"))
    ));

    for hunk in &patch.hunks {
        let old_start = if hunk.old_lines == 0 {
            hunk.old_start.saturating_sub(1)
        } else {
            hunk.old_start
        };
        let new_start = if hunk.new_lines == 0 {
            hunk.new_start.saturating_sub(1)
        } else {
            hunk.new_start
        };
        ret.push(format!(
            "@@ -{old_start},{} +{new_start},{} @@",
            hunk.old_lines, hunk.new_lines
        ));
        ret.extend(hunk.lines.iter().cloned());
    }

    let mut output = ret.join("\n");
    output.push('\n');
    output
}

#[allow(clippy::too_many_arguments)]
pub fn create_two_files_patch(
    old_file_name: &str,
    new_file_name: &str,
    old_str: &str,
    new_str: &str,
    old_header: Option<&str>,
    new_header: Option<&str>,
    options: &PatchOptions,
) -> Option<String> {
    structured_patch(
        old_file_name,
        new_file_name,
        old_str,
        new_str,
        old_header,
        new_header,
        options,
    )
    .map(|patch| format_patch(&patch))
}

pub fn create_patch(
    file_name: &str,
    old_str: &str,
    new_str: &str,
    old_header: Option<&str>,
    new_header: Option<&str>,
    options: &PatchOptions,
) -> Option<String> {
    create_two_files_patch(
        file_name, file_name, old_str, new_str, old_header, new_header, options,
    )
}

fn split_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    text.split_inclusive('\n').map(str::to_owned).collect()
}

fn needs_quoting(value: &str) -> bool {
    value.chars().any(|character| {
        !('\x20'..='\x7e').contains(&character) || character == '"' || character == '\\'
    })
}

fn quote_file_name_if_needed(value: &str) -> String {
    if !needs_quoting(value) {
        return value.to_owned();
    }

    let mut result = String::from("\"");
    for byte in value.bytes() {
        match byte {
            0x07 => result.push_str("\\a"),
            0x08 => result.push_str("\\b"),
            0x09 => result.push_str("\\t"),
            0x0a => result.push_str("\\n"),
            0x0b => result.push_str("\\v"),
            0x0c => result.push_str("\\f"),
            0x0d => result.push_str("\\r"),
            0x22 => result.push_str("\\\""),
            0x5c => result.push_str("\\\\"),
            0x20..=0x7e => result.push(char::from(byte)),
            _ => result.push_str(&format!("\\{byte:03o}")),
        }
    }
    result.push('"');
    result
}
