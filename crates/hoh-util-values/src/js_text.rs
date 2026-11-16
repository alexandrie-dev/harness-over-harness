/// What `\s` matches and `trim()` removes in JavaScript.
pub fn is_js_white_space(ch: char) -> bool {
    matches!(ch, '\u{9}'..='\u{d}' | ' ' | '\u{a0}' | '\u{1680}'
        | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}'
        | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

/// JavaScript's `String.prototype.trim`, borrowing the retained text.
pub fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_white_space)
}

/// JavaScript's `String.prototype.trimStart`, borrowing the retained text.
pub fn js_trim_start(text: &str) -> &str {
    text.trim_start_matches(is_js_white_space)
}

/// JavaScript's `String.prototype.trimEnd`, borrowing the retained text.
pub fn js_trim_end(text: &str) -> &str {
    text.trim_end_matches(is_js_white_space)
}

/// The inside of a `regex` character class matching JavaScript white space.
/// Use `format!("[{JS_WHITE_SPACE_SET}]+")` for `\s+`,
/// or `format!("[^{JS_WHITE_SPACE_SET}]")` for `\S`.
pub const JS_WHITE_SPACE_SET: &str = r"\x{9}-\x{d}\x{20}\x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}";
