//! Fixed interfaces for Loader functionality that is not public yet.

use hoh_loader::{Entry, EvalEnv, PluginRegistry};
use serde_json::{Map, Value};
use std::{fmt::Write as _, path::PathBuf, sync::Arc};

/// SHIM(hoh-loader): the source registry inherits the active Loader's registry,
/// evaluator and declaring base. Replace this explicit runtime when Loader
/// exposes a read-only child-tree mount API.
#[derive(Clone)]
pub struct PresetRuntime {
    pub registry: Arc<PluginRegistry>,
    pub env: EvalEnv,
}

impl PresetRuntime {
    pub fn new(registry: Arc<PluginRegistry>, env: EvalEnv) -> Self {
        Self { registry, env }
    }

    pub(crate) fn fallback() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let exe = std::env::current_exe().unwrap_or_else(|_| cwd.join("hoh"));
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| cwd.clone());
        Self {
            registry: Arc::new(PluginRegistry::default()),
            env: EvalEnv {
                home,
                cwd,
                exe,
                platform: match std::env::consts::OS {
                    "macos" => "darwin",
                    "windows" => "win32",
                    _ => "linux",
                },
                var: Arc::new(|name| std::env::var(name).ok()),
            },
        }
    }
}

pub(crate) fn decode_entries(value: &Value) -> Result<Vec<Entry>, crate::RegistryError> {
    serde_json::from_value(value.clone())
        .map_err(|error| crate::RegistryError(format!("invalid preset composition: {error}")))
}

fn expression(value: &Value) -> Option<&str> {
    let object = value.as_object()?;
    object.get("__jsExpr")?.as_str()
}

fn yaml_number(value: &serde_json::Number) -> String {
    if let Some(value) = value.as_i64() {
        return value.to_string();
    }
    if let Some(value) = value.as_u64() {
        return value.to_string();
    }
    let value = value.as_f64().unwrap_or(f64::NAN);
    if value == 0.0 && value.is_sign_negative() {
        return "-0.0".into();
    }
    let mut rendered = hoh_util_values::js_number_string(value);
    if value.fract() != 0.0
        && let Some(exponent) = rendered.find('e')
        && !rendered[..exponent].contains('.')
    {
        rendered.insert(exponent, '.');
    }
    rendered
}

fn yaml_scalar(value: &Value) -> Option<String> {
    if let Some(expression) = expression(value) {
        return Some(format!("!!js {}", yaml_string(expression)));
    }
    match value {
        Value::Null => Some("null".into()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(yaml_number(value)),
        Value::String(value) => Some(yaml_string(value)),
        Value::Array(value) if value.is_empty() => Some("[]".into()),
        Value::Object(value) if value.is_empty() => Some("{}".into()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

fn yaml_string(value: &str) -> String {
    if value.is_empty() {
        return "''".into();
    }
    if value.chars().any(|ch| !is_js_yaml_printable(ch)) {
        return yaml_double_string(value);
    }
    if !is_plain_string(value)
        || is_json_schema_scalar(value)
        || is_deprecated_boolean(value)
        || is_deprecated_base60(value)
    {
        format!("'{}'", value.replace('\'', "''"))
    } else {
        value.to_owned()
    }
}

fn is_js_yaml_printable(ch: char) -> bool {
    let code = u32::from(ch);
    (0x20..=0x7e).contains(&code)
        || ((0xa1..=0xd7ff).contains(&code) && !matches!(code, 0x2028 | 0x2029))
        || ((0xe000..=0xfffd).contains(&code) && code != 0xfeff)
        || (0x10000..=0x10ffff).contains(&code)
}

fn is_plain_string(value: &str) -> bool {
    let mut chars = value.chars();
    let first = chars.next().expect("non-empty YAML string");
    let last = value.chars().next_back().expect("non-empty YAML string");
    if !is_plain_safe_first(first) || is_js_whitespace(last) || last == ':' {
        return false;
    }
    let mut previous = None;
    for ch in value.chars() {
        let ns_or_whitespace = is_ns_char_or_whitespace(ch);
        let ns = ns_or_whitespace && !is_js_whitespace(ch);
        let normally_safe = ns_or_whitespace && ch != '#' && !(previous == Some(':') && !ns);
        let hash_after_ns = ch == '#'
            && previous.is_some_and(|previous| {
                is_ns_char_or_whitespace(previous) && !is_js_whitespace(previous)
            });
        let colon_before_ns = previous == Some(':') && ns;
        if !(normally_safe || hash_after_ns || colon_before_ns) {
            return false;
        }
        previous = Some(ch);
    }
    true
}

fn is_plain_safe_first(ch: char) -> bool {
    is_js_yaml_printable(ch)
        && !is_js_whitespace(ch)
        && !matches!(
            ch,
            '\u{feff}'
                | '-'
                | '?'
                | ':'
                | ','
                | '['
                | ']'
                | '{'
                | '}'
                | '#'
                | '&'
                | '*'
                | '!'
                | '|'
                | '='
                | '>'
                | '\''
                | '"'
                | '%'
                | '@'
                | '`'
        )
}

fn is_js_whitespace(ch: char) -> bool {
    matches!(ch, ' ' | '\t')
}

fn is_ns_char_or_whitespace(ch: char) -> bool {
    is_js_yaml_printable(ch) && !matches!(ch, '\u{feff}' | '\r' | '\n')
}

fn is_json_schema_scalar(value: &str) -> bool {
    matches!(
        value,
        "~" | "null" | "Null" | "NULL" | "true" | "True" | "TRUE" | "false" | "False" | "FALSE"
    ) || is_yaml_integer(value)
        || is_yaml_float(value)
}

fn unsigned_yaml_number(value: &str) -> &str {
    value.strip_prefix(['+', '-']).unwrap_or(value)
}

fn finite_radix(digits: &str, radix: u32) -> bool {
    let mut number = 0.0_f64;
    for digit in digits.chars().filter_map(|ch| ch.to_digit(radix)) {
        number = number.mul_add(f64::from(radix), f64::from(digit));
        if !number.is_finite() {
            return false;
        }
    }
    true
}

fn is_yaml_integer(value: &str) -> bool {
    let unsigned = unsigned_yaml_number(value);
    if unsigned == "0" {
        return true;
    }
    for (prefix, radix) in [("0b", 2), ("0x", 16), ("0o", 8)] {
        if let Some(digits) = unsigned.strip_prefix(prefix) {
            return !digits.is_empty()
                && digits.chars().all(|ch| ch.is_digit(radix))
                && finite_radix(digits, radix);
        }
    }
    !unsigned.is_empty()
        && unsigned.bytes().all(|byte| byte.is_ascii_digit())
        && value.parse::<f64>().is_ok_and(f64::is_finite)
}

fn is_yaml_float(value: &str) -> bool {
    let unsigned = unsigned_yaml_number(value);
    if matches!(unsigned, ".inf" | ".Inf" | ".INF") || matches!(value, ".nan" | ".NaN" | ".NAN") {
        return true;
    }
    let (mantissa, exponent) = value.find(['e', 'E']).map_or((value, None), |index| {
        (&value[..index], Some(&value[index + 1..]))
    });
    if exponent.is_some_and(|exponent| {
        let digits = unsigned_yaml_number(exponent);
        digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit())
    }) {
        return false;
    }
    let unsigned_mantissa = unsigned_yaml_number(mantissa);
    let signed_mantissa = unsigned_mantissa.len() != mantissa.len();
    let valid_mantissa = if let Some((whole, fraction)) = unsigned_mantissa.split_once('.') {
        (!whole.is_empty()
            && whole.bytes().all(|byte| byte.is_ascii_digit())
            && fraction.bytes().all(|byte| byte.is_ascii_digit()))
            || (!signed_mantissa
                && whole.is_empty()
                && !fraction.is_empty()
                && fraction.bytes().all(|byte| byte.is_ascii_digit()))
    } else {
        !unsigned_mantissa.is_empty() && unsigned_mantissa.bytes().all(|byte| byte.is_ascii_digit())
    };
    valid_mantissa && value.parse::<f64>().is_ok_and(f64::is_finite)
}

fn is_deprecated_boolean(value: &str) -> bool {
    matches!(
        value,
        "y" | "Y"
            | "yes"
            | "Yes"
            | "YES"
            | "on"
            | "On"
            | "ON"
            | "n"
            | "N"
            | "no"
            | "No"
            | "NO"
            | "off"
            | "Off"
            | "OFF"
    )
}

fn is_deprecated_base60(value: &str) -> bool {
    let unsigned = unsigned_yaml_number(value);
    let mut components = unsigned.split(':');
    let Some(first) = components.next() else {
        return false;
    };
    if first.is_empty()
        || !first
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'_')
    {
        return false;
    }
    let rest: Vec<_> = components.collect();
    if rest.is_empty() {
        return false;
    }
    rest.iter().enumerate().all(|(index, component)| {
        if index + 1 == rest.len() {
            let mut parts = component.split('.');
            let integer = parts.next().unwrap_or_default();
            let fraction = parts.next();
            parts.next().is_none()
                && !integer.is_empty()
                && integer
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'_')
                && fraction.is_none_or(|fraction| {
                    fraction
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || byte == b'_')
                })
        } else {
            !component.is_empty()
                && component
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'_')
        }
    })
}

fn yaml_double_string(value: &str) -> String {
    let mut output = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\0' => output.push_str("\\0"),
            '\u{7}' => output.push_str("\\a"),
            '\u{8}' => output.push_str("\\b"),
            '\t' => output.push_str("\\t"),
            '\n' => output.push_str("\\n"),
            '\u{b}' => output.push_str("\\v"),
            '\u{c}' => output.push_str("\\f"),
            '\r' => output.push_str("\\r"),
            '\u{1b}' => output.push_str("\\e"),
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{85}' => output.push_str("\\N"),
            '\u{a0}' => output.push_str("\\_"),
            '\u{2028}' => output.push_str("\\L"),
            '\u{2029}' => output.push_str("\\P"),
            _ if is_js_yaml_printable(ch) => output.push(ch),
            _ => {
                let code = u32::from(ch);
                if code <= 0xff {
                    write!(output, "\\x{code:02X}").expect("write to string");
                } else if code <= 0xffff {
                    write!(output, "\\u{code:04X}").expect("write to string");
                } else {
                    write!(output, "\\U{code:08X}").expect("write to string");
                }
            }
        }
    }
    output.push('"');
    output
}

fn multiline(value: &Value) -> Option<(String, Vec<&str>)> {
    let (value, tag) = match value {
        Value::String(value) => (value.as_str(), ""),
        _ => (expression(value)?, "!!js "),
    };
    (value.contains('\n')
        && value
            .chars()
            .all(|ch| ch == '\n' || is_js_yaml_printable(ch)))
    .then(|| {
        let trailing = value.chars().rev().take_while(|ch| *ch == '\n').count();
        let indent = value.trim_start_matches('\n').starts_with(' ');
        let chomp = match trailing {
            0 => "-",
            1 if value != "\n" => "",
            _ => "+",
        };
        let indicator = format!("{tag}|{}{chomp}", if indent { "2" } else { "" });
        let mut lines: Vec<_> = value.split('\n').collect();
        if trailing > 0 {
            lines.pop();
        }
        (indicator, lines)
    })
}

fn emit_multiline(lines: &mut Vec<String>, prefix: String, indent: usize, value: &Value) -> bool {
    let Some((indicator, values)) = multiline(value) else {
        return false;
    };
    lines.push(format!("{prefix}{indicator}"));
    lines.extend(values.into_iter().map(|value| {
        if value.is_empty() {
            String::new()
        } else {
            format!("{}{value}", " ".repeat(indent))
        }
    }));
    true
}

fn yaml_key(value: &str) -> String {
    yaml_string(value)
}

fn emit_entry(
    lines: &mut Vec<String>,
    prefix: String,
    child_indent: usize,
    key: &str,
    value: &Value,
) {
    let key = yaml_key(key);
    if emit_multiline(lines, format!("{prefix}{key}: "), child_indent, value) {
        return;
    }
    if let Some(value) = yaml_scalar(value) {
        lines.push(format!("{prefix}{key}: {value}"));
    } else {
        lines.push(format!("{prefix}{key}:"));
        emit_yaml(lines, value, child_indent);
    }
}

fn emit_object(lines: &mut Vec<String>, object: &Map<String, Value>, indent: usize, list: bool) {
    let mut entries = object.iter();
    if list {
        if let Some((key, value)) = entries.next() {
            emit_entry(
                lines,
                format!("{}- ", " ".repeat(indent)),
                indent + 4,
                key,
                value,
            );
        }
        for (key, value) in entries {
            emit_entry(lines, " ".repeat(indent + 2), indent + 4, key, value);
        }
    } else {
        for (key, value) in entries {
            emit_entry(lines, " ".repeat(indent), indent + 2, key, value);
        }
    }
}

fn emit_yaml(lines: &mut Vec<String>, value: &Value, indent: usize) {
    match value {
        Value::Array(values) => {
            for value in values {
                if emit_multiline(
                    lines,
                    format!("{}- ", " ".repeat(indent)),
                    indent + 2,
                    value,
                ) {
                    continue;
                } else if let Some(value) = yaml_scalar(value) {
                    lines.push(format!("{}- {value}", " ".repeat(indent)));
                } else if let Value::Object(object) = value {
                    emit_object(lines, object, indent, true);
                } else {
                    lines.push(format!("{}-", " ".repeat(indent)));
                    emit_yaml(lines, value, indent + 2);
                }
            }
        }
        Value::Object(object) => emit_object(lines, object, indent, false),
        _ => lines.push(format!(
            "{}{}",
            " ".repeat(indent),
            yaml_scalar(value).expect("primitive YAML value")
        )),
    }
}

/// SHIM(hoh-loader): local entry-list emitter until `stringifyEntryYaml` is public.
pub(crate) fn stringify_entry_yaml(value: &Value) -> String {
    let mut lines = Vec::new();
    emit_yaml(&mut lines, value, 0);
    let mut output = lines.join("\n");
    output.push('\n');
    output
}
