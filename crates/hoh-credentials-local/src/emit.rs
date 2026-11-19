//! YAML text for values the provider writes.
//!
//! The source renders with `Document.toString()` of the `yaml` package: plain scalars where the
//! text stays a string, double quotes otherwise, block literals for multi-line text, block
//! mappings and indented block sequences. This module writes the same shapes for the values the
//! provider stores (strings, and the JSON a grant payload may hold).

use serde_json::Value;

use crate::syntax::{Yaml, resolve_plain};

/// How one string is written after `key: `.
pub(crate) enum StringForm {
    /// A single-line scalar (plain, single- or double-quoted).
    Inline(String),
    /// A literal block: the header after the key (`|-`, `|`, `|+`) and the content lines.
    Block {
        header: &'static str,
        lines: Vec<String>,
    },
}

fn is_unprintable(c: char) -> bool {
    matches!(c, '\u{0000}'..='\u{001f}' | '\u{007f}'..='\u{009f}' | '\u{2028}' | '\u{2029}' | '\u{feff}')
}

/// Whether the single-line `value` may stand unquoted and still read back as this string.
fn plain_allowed(value: &str, implicit_key: bool) -> bool {
    let Some(first) = value.chars().next() else {
        return false;
    };
    if value.chars().any(is_unprintable) {
        return false;
    }
    if ",[]{}#&*!|>'\"%@` \t".contains(first) {
        return false;
    }
    if value == "?" || value == "-" || value.starts_with("? ") || value.starts_with("- ") {
        return false;
    }
    if value.starts_with("?\t") || value.starts_with("-\t") {
        return false;
    }
    if value.contains(": ")
        || value.contains(":\t")
        || value.contains(" #")
        || value.contains("\t#")
    {
        return false;
    }
    if value.ends_with([' ', '\t', ':']) {
        return false;
    }
    // A document marker at the start of a line is not a scalar.
    if implicit_key
        && (value.starts_with("---") || value.starts_with("..."))
        && value.chars().nth(3).is_none_or(|c| c == ' ' || c == '\t')
    {
        return false;
    }
    matches!(resolve_plain(value), Yaml::Str(_))
}

fn double_quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if is_unprintable(c) => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn quoted(value: &str) -> String {
    // `yaml` prefers single quotes only for text that holds a double quote and no single quote.
    if value.contains('"') && !value.contains('\'') && !value.chars().any(is_unprintable) {
        return format!("'{value}'");
    }
    double_quoted(value)
}

/// The form `value` takes after a mapping key or sequence dash. `implicit_key` asks for a form
/// that is also a valid simple key (single line).
pub(crate) fn string_form(value: &str, implicit_key: bool) -> StringForm {
    if value.contains('\n') && !implicit_key {
        return block_form(value).unwrap_or_else(|| StringForm::Inline(double_quoted(value)));
    }
    if value.contains('\n') || !plain_allowed(value, implicit_key) {
        return StringForm::Inline(quoted(value));
    }
    StringForm::Inline(value.to_owned())
}

fn block_form(value: &str) -> Option<StringForm> {
    let body = value.trim_end_matches('\n');
    let trailing = value.len() - body.len();
    if body.is_empty()
        || body.starts_with([' ', '\t', '\n'])
        || body
            .lines()
            .last()
            .is_some_and(|line| line.ends_with([' ', '\t']))
        || body
            .chars()
            .any(|c| is_unprintable(c) && c != '\n' && c != '\t')
    {
        return None;
    }
    let header = match trailing {
        0 => "|-",
        1 => "|",
        _ => "|+",
    };
    let mut lines: Vec<String> = body.split('\n').map(str::to_owned).collect();
    for _ in 2..=trailing {
        lines.push(String::new());
    }
    Some(StringForm::Block { header, lines })
}

fn pad(indent: usize) -> String {
    " ".repeat(indent)
}

fn scalar_text(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(number) => number.as_i64().map_or_else(
            || {
                number.as_u64().map_or_else(
                    || hoh_util_values::js_number_string(number.as_f64().unwrap_or(f64::NAN)),
                    |value| value.to_string(),
                )
            },
            |value| value.to_string(),
        ),
        Value::String(_) | Value::Array(_) | Value::Object(_) => unreachable!("not a scalar"),
    }
}

fn key_text(key: &str) -> String {
    match string_form(key, true) {
        StringForm::Inline(text) => text,
        StringForm::Block { .. } => double_quoted(key),
    }
}

/// `key: value` lines (one or more) at `indent`, for a mapping entry.
fn write_entry(out: &mut String, indent: usize, key: &str, value: &Value) {
    let head = format!("{}{}:", pad(indent), key_text(key));
    match value {
        Value::Object(map) if map.is_empty() => out.push_str(&format!("{head} {{}}\n")),
        Value::Array(items) if items.is_empty() => out.push_str(&format!("{head} []\n")),
        Value::Object(map) => {
            out.push_str(&format!("{head}\n"));
            write_map(out, indent + 2, map);
        }
        Value::Array(items) => {
            out.push_str(&format!("{head}\n"));
            write_seq(out, indent + 2, items);
        }
        Value::String(text) => match string_form(text, false) {
            StringForm::Inline(text) => out.push_str(&format!("{head} {text}\n")),
            StringForm::Block { header, lines } => {
                out.push_str(&format!("{head} {header}\n"));
                for line in lines {
                    push_block_line(out, indent + 2, &line);
                }
            }
        },
        scalar => out.push_str(&format!("{head} {}\n", scalar_text(scalar))),
    }
}

fn push_block_line(out: &mut String, indent: usize, line: &str) {
    if !line.is_empty() {
        out.push_str(&pad(indent));
        out.push_str(line);
    }
    out.push('\n');
}

/// The entries of a non-empty mapping, as block lines at `indent`.
pub(crate) fn write_map(out: &mut String, indent: usize, map: &serde_json::Map<String, Value>) {
    for (key, value) in map {
        write_entry(out, indent, key, value);
    }
}

fn write_seq(out: &mut String, indent: usize, items: &[Value]) {
    for item in items {
        let dash = format!("{}- ", pad(indent));
        match item {
            Value::Object(map) if map.is_empty() => out.push_str(&format!("{dash}{{}}\n")),
            Value::Array(items) if items.is_empty() => out.push_str(&format!("{dash}[]\n")),
            Value::Object(_) | Value::Array(_) => {
                // The nested block starts on the dash's line: render it two columns deeper and
                // swap its first line's indentation for the dash.
                let mut nested = String::new();
                match item {
                    Value::Object(map) => write_map(&mut nested, indent + 2, map),
                    Value::Array(items) => write_seq(&mut nested, indent + 2, items),
                    _ => unreachable!("matched a collection"),
                }
                out.push_str(&dash);
                out.push_str(&nested[indent + 2..]);
            }
            Value::String(text) => match string_form(text, false) {
                StringForm::Inline(text) => out.push_str(&format!("{dash}{text}\n")),
                StringForm::Block { header, lines } => {
                    out.push_str(&format!("{dash}{header}\n"));
                    for line in lines {
                        push_block_line(out, indent + 2, &line);
                    }
                }
            },
            scalar => out.push_str(&format!("{dash}{}\n", scalar_text(scalar))),
        }
    }
}

/// One mapping entry `key: value` at `indent` (a string value may be a block literal).
pub(crate) fn entry_text(indent: usize, key: &str, value: &Value) -> String {
    let mut out = String::new();
    write_entry(&mut out, indent, key, value);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(key: &str, value: Value) -> String {
        entry_text(2, key, &value)
    }

    #[test]
    fn plain_where_the_text_stays_a_string() {
        assert_eq!(entry("K", json!("new value!")), "  K: new value!\n");
        assert_eq!(entry("K", json!("sk-fresh")), "  K: sk-fresh\n");
        assert_eq!(entry("K", json!("both ' and \"")), "  K: both ' and \"\n");
    }

    #[test]
    fn quotes_text_that_would_change_meaning() {
        assert_eq!(entry("K", json!("123")), "  K: \"123\"\n");
        assert_eq!(entry("K", json!("true")), "  K: \"true\"\n");
        assert_eq!(entry("K", json!("null")), "  K: \"null\"\n");
        assert_eq!(entry("K", json!("a: b")), "  K: \"a: b\"\n");
        assert_eq!(entry("K", json!("# x")), "  K: \"# x\"\n");
        assert_eq!(entry("K", json!(" lead")), "  K: \" lead\"\n");
        assert_eq!(entry("K", json!("x ")), "  K: \"x \"\n");
        assert_eq!(entry("K", json!("")), "  K: \"\"\n");
        assert_eq!(entry("K", json!("\"hi\"")), "  K: '\"hi\"'\n");
    }

    #[test]
    fn multi_line_text_is_a_literal_block() {
        assert_eq!(
            entry("K", json!("line one\nline two")),
            "  K: |-\n    line one\n    line two\n"
        );
        assert_eq!(entry("K", json!("a\nb\n")), "  K: |\n    a\n    b\n");
        assert_eq!(entry("K", json!("a\n\nb")), "  K: |-\n    a\n\n    b\n");
        assert_eq!(entry("K", json!("a \nb ")), "  K: \"a \\nb \"\n");
    }

    #[test]
    fn nested_values_use_indented_block_collections() {
        let text = entry_text(
            4,
            "payload",
            &json!({"type": "oauth", "scopes": ["a", "b"], "n": {"x": 1}, "e": {}, "l": [], "z": [[1, 2], {"k": "v"}], "d": 1.5, "t": true, "u": null}),
        );
        assert_eq!(
            text,
            "    payload:\n      type: oauth\n      scopes:\n        - a\n        - b\n      n:\n        x: 1\n      e: {}\n      l: []\n      z:\n        - - 1\n          - 2\n        - k: v\n      d: 1.5\n      t: true\n      u: null\n"
        );
    }
}
