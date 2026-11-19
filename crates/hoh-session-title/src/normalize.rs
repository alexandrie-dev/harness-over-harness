//! Title-text cleaning and UTF-8-safe truncation.

use hoh_util_values::{is_js_white_space, js_trim, js_trim_end};

fn assert_positive(name: &str, value: usize) -> Result<(), String> {
    if value == 0 {
        return Err(format!("{name} must be a positive integer"));
    }
    Ok(())
}

fn is_directional_control(ch: char) -> bool {
    matches!(ch, '\u{200b}' | '\u{200e}' | '\u{200f}' | '\u{feff}')
        || ('\u{202a}'..='\u{202e}').contains(&ch)
        || ('\u{2060}'..='\u{2064}').contains(&ch)
        || ('\u{2066}'..='\u{206f}').contains(&ch)
}

fn is_removed_control(ch: char) -> bool {
    matches!(ch as u32, 0x00..=0x08 | 0x0b | 0x0c | 0x0e..=0x1f | 0x7f..=0x9f)
}

fn strip_osc(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut output = String::new();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        let osc_start =
            ch == '\u{009d}' || (ch == '\u{001b}' && chars.get(index + 1) == Some(&']'));
        if osc_start {
            index += if ch == '\u{001b}' { 2 } else { 1 };
            while index < chars.len() {
                if chars[index] == '\u{0007}' {
                    index += 1;
                    break;
                }
                if chars[index] == '\u{001b}' && chars.get(index + 1) == Some(&'\\') {
                    index += 2;
                    break;
                }
                index += 1;
            }
            continue;
        }
        output.push(ch);
        index += 1;
    }
    output
}

fn strip_csi(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut output = String::new();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        let csi_start =
            ch == '\u{009b}' || (ch == '\u{001b}' && chars.get(index + 1) == Some(&'['));
        if csi_start {
            let mut cursor = index + if ch == '\u{001b}' { 2 } else { 1 };
            while chars.get(cursor).is_some_and(|c| ('0'..='?').contains(c)) {
                cursor += 1;
            }
            while chars.get(cursor).is_some_and(|c| (' '..='/').contains(c)) {
                cursor += 1;
            }
            if chars.get(cursor).is_some_and(|c| ('@'..='~').contains(c)) {
                index = cursor + 1;
                continue;
            }
        }
        output.push(ch);
        index += 1;
    }
    output
}

fn strip_esc(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut output = String::new();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if ch == '\u{001b}'
            && chars
                .get(index + 1)
                .is_some_and(|next| ('@'..='_').contains(next))
        {
            index += 2;
            continue;
        }
        output.push(ch);
        index += 1;
    }
    output
}

fn clean_title_text(input: &str) -> String {
    // The source applies five global replacements in this exact order. Keeping
    // the passes separate matters when deleting one escape reveals another.
    let without_osc = strip_osc(input);
    let without_csi = strip_csi(&without_osc);
    let without_esc = strip_esc(&without_csi);
    let visible: String = without_esc
        .chars()
        .filter(|ch| !is_removed_control(*ch))
        .filter(|ch| !is_directional_control(*ch))
        .collect();

    let mut collapsed = String::new();
    let mut in_space = false;
    for ch in visible.chars() {
        if is_js_white_space(ch) {
            if !in_space {
                collapsed.push(' ');
                in_space = true;
            }
        } else {
            collapsed.push(ch);
            in_space = false;
        }
    }
    js_trim(&collapsed).to_owned()
}

/// Truncate a string to a UTF-8 byte budget without splitting a code point.
pub fn truncate_title_utf8(input: &str, max_bytes: usize) -> Result<String, String> {
    assert_positive("maxBytes", max_bytes)?;
    if input.len() <= max_bytes {
        return Ok(input.to_owned());
    }
    let end = input
        .char_indices()
        .map(|(offset, _)| offset)
        .take_while(|offset| *offset <= max_bytes)
        .last()
        .unwrap_or(0);
    let end = if input.is_char_boundary(max_bytes) {
        max_bytes
    } else {
        end
    };
    Ok(input[..end].to_owned())
}

/// Normalize an untrusted title into one terminal-safe line.
pub fn normalize_session_title(input: &str, max_bytes: usize) -> Result<String, String> {
    let clean = clean_title_text(input);
    let truncated = truncate_title_utf8(&clean, max_bytes)?;
    Ok(js_trim_end(&truncated).to_owned())
}

/// Derive the deterministic first-prompt fallback.
pub fn fallback_session_title(
    input: &str,
    max_words: usize,
    max_bytes: usize,
) -> Result<String, String> {
    assert_positive("maxWords", max_words)?;
    let clean = clean_title_text(input);
    let words = clean.split(' ').filter(|word| !word.is_empty());
    let leading = words.take(max_words).collect::<Vec<_>>().join(" ");
    let truncated = truncate_title_utf8(&leading, max_bytes)?;
    Ok(js_trim_end(&truncated).to_owned())
}
