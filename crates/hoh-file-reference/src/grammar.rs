//! Browser-safe `@file` token grammar shared by terminal and web clients.

use hoh_util_values::is_js_white_space;

use crate::{FileReferenceCandidate, FileReferenceKind};

/// Extract an `@path` or `@"path with spaces` token at the cursor.
///
/// `cursor_col` is measured in UTF-16 code units, like JavaScript string
/// offsets. A cursor inside a surrogate pair is not a representable Rust string
/// boundary and therefore has no active token.
pub fn active_at_token(line: &str, cursor_col: usize) -> Option<crate::ActiveAtToken> {
    let before_cursor = prefix_at_utf16_offset(line, cursor_col)?;

    if let Some(token) = quoted_token(before_cursor) {
        return Some(crate::ActiveAtToken {
            prefix: token.to_owned(),
            query: token[2..].to_owned(),
            quoted: true,
        });
    }

    let token_start = before_cursor
        .char_indices()
        .rev()
        .find_map(|(index, character)| {
            is_js_white_space(character).then_some(index + character.len_utf8())
        })
        .unwrap_or(0);
    let token = &before_cursor[token_start..];

    token.strip_prefix('@').map(|query| crate::ActiveAtToken {
        prefix: token.to_owned(),
        query: query.to_owned(),
        quoted: false,
    })
}

fn quoted_token(text: &str) -> Option<&str> {
    std::iter::once(0)
        .chain(text.char_indices().filter_map(|(index, character)| {
            is_js_white_space(character).then_some(index + character.len_utf8())
        }))
        .map(|start| &text[start..])
        .find(|token| {
            token
                .strip_prefix("@\"")
                .is_some_and(|query| !query.contains('"'))
        })
}

/// Format a selected path as prompt text.
///
/// Whitespace uses the quoted `@"path"` grammar. A quoted directory keeps
/// that quote open after its trailing slash so completion can descend another
/// level. Paths containing controls or a quote cannot be represented safely.
pub fn format_file_mention(
    candidate: &FileReferenceCandidate,
    preserve_quote: bool,
) -> Option<String> {
    let path = match candidate.kind {
        FileReferenceKind::File => candidate.path.clone(),
        FileReferenceKind::Directory => format!("{}/", candidate.path),
    };
    if path.chars().any(
        |character| matches!(character, '\u{0000}'..='\u{001f}' | '\u{007f}'..='\u{009f}' | '"'),
    ) {
        return None;
    }

    let quoted = preserve_quote || path.chars().any(is_js_white_space);
    if !quoted {
        return Some(format!("@{path}"));
    }
    if candidate.kind == FileReferenceKind::Directory {
        Some(format!("@\"{path}"))
    } else {
        Some(format!("@\"{path}\""))
    }
}

fn prefix_at_utf16_offset(line: &str, cursor_col: usize) -> Option<&str> {
    let mut units = 0;
    for (byte_index, character) in line.char_indices() {
        if units == cursor_col {
            return Some(&line[..byte_index]);
        }
        units += character.len_utf16();
        if units > cursor_col {
            return None;
        }
    }
    Some(line)
}
