//! Explicit Markdown blockquote recognition and decoding.

use hoh_util_values::js_trim;

/// One blockquote span in original UTF-16 text, excluding its trailing line break.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MessageQuoteRange {
    pub start: usize,
    pub end: usize,
}

fn is_js_line_terminator(character: char) -> bool {
    matches!(character, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

fn marker_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = start;
    let mut spaces = 0;
    while bytes.get(index) == Some(&b' ') && spaces < 3 {
        spaces += 1;
        index += 1;
    }
    if bytes.get(index) != Some(&b'>') {
        return None;
    }
    index += 1;
    if matches!(bytes.get(index), Some(b' ' | b'\t')) {
        index += 1;
    }
    Some(index)
}

fn explicit_quote_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut content = marker_end(text, start)?;
    loop {
        while content < bytes.len() && !matches!(bytes[content], b'\r' | b'\n') {
            content += 1;
        }
        let line_end = content;
        let next_line = match bytes.get(content) {
            Some(b'\n') => content + 1,
            Some(b'\r') if bytes.get(content + 1) == Some(&b'\n') => content + 2,
            _ => return Some(line_end),
        };
        if marker_end(text, next_line).is_none() {
            return Some(line_end);
        }
        content = marker_end(text, next_line).expect("continued quote marker was checked");
    }
}

fn utf16_offset(text: &str, byte: usize) -> usize {
    text[..byte].encode_utf16().count()
}

/// Remove one blockquote marker from each explicitly quoted line.
pub fn quote_text_of(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\r' {
            if characters.peek() == Some(&'\n') {
                characters.next();
            }
            normalized.push('\n');
        } else {
            normalized.push(character);
        }
    }

    let mut decoded = String::with_capacity(normalized.len());
    let mut index = 0;
    let mut line_start = true;
    while index < normalized.len() {
        if line_start && let Some(after_marker) = marker_end(&normalized, index) {
            index = after_marker;
        }
        if index >= normalized.len() {
            break;
        }
        let character = normalized[index..]
            .chars()
            .next()
            .expect("index is on a character boundary");
        decoded.push(character);
        index += character.len_utf8();
        line_start = is_js_line_terminator(character);
    }
    decoded
}

/// Recognize consecutive explicitly marked Markdown blockquote lines.
pub fn message_quote_ranges(text: &str) -> Vec<MessageQuoteRange> {
    let mut candidates = vec![0usize];
    for (byte, character) in text.char_indices() {
        if is_js_line_terminator(character) {
            candidates.push(byte + character.len_utf8());
        }
    }

    let mut ranges = Vec::new();
    let mut search_from = 0;
    for start in candidates {
        if start < search_from {
            continue;
        }
        let Some(end) = explicit_quote_end(text, start) else {
            continue;
        };
        // matchAll advances even across a blank quote which is filtered out.
        search_from = end;
        if !js_trim(&quote_text_of(&text[start..end])).is_empty() {
            ranges.push(MessageQuoteRange {
                start: utf16_offset(text, start),
                end: utf16_offset(text, end),
            });
        }
    }
    ranges
}

pub(crate) fn byte_index_at_utf16(text: &str, offset: usize) -> Option<usize> {
    if offset == 0 {
        return Some(0);
    }
    let mut units = 0;
    for (byte, character) in text.char_indices() {
        units += character.len_utf16();
        if units == offset {
            return Some(byte + character.len_utf8());
        }
        if units > offset {
            return None;
        }
    }
    (units == offset).then_some(text.len())
}

pub(crate) fn slice_utf16(text: &str, start: usize, end: usize) -> Option<&str> {
    let start = byte_index_at_utf16(text, start)?;
    let end = byte_index_at_utf16(text, end)?;
    text.get(start..end)
}
