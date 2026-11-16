//! Port of `@deepseek-ai/dsh-util-values` (`packages/util/values`).
//!
//! JSON helpers over [`serde_json::Value`] and the lazy streamed-argument reader
//! [`PartialArguments`]. `assertNever`, `deepFreeze` and `WeakMapWithValues` have no
//! Rust counterpart; see `PORT_NOTES.md`.

mod js_json;
pub mod partial_json;

use serde_json::Number;

pub use js_json::{
    JS_JSON_MAX_DEPTH, JsonParseError, js_json_parse, js_json_stringify, js_json_stringify_pretty,
};
pub use partial_json::{LengthReadOptions, PartialArguments, SealedViewError};
/// A value that round-trips through JSON without loss.
pub use serde_json::Value as JsonValue;

/// Test whether a value survives a JSON round trip without loss.
///
/// A `serde_json::Value` is already acyclic, dense and string-keyed, so the only
/// remaining violation of the lossless rules is a negative-zero number. The walk is
/// iterative, so deep nesting does not consume the call stack.
pub fn is_json_value(value: &JsonValue) -> bool {
    let mut pending = vec![value];
    while let Some(node) = pending.pop() {
        match node {
            JsonValue::Number(number) => {
                if number.is_f64()
                    && number
                        .as_f64()
                        .is_some_and(|float| float == 0.0 && float.is_sign_negative())
                {
                    return false;
                }
            }
            JsonValue::Array(items) => pending.extend(items.iter()),
            JsonValue::Object(map) => pending.extend(map.values()),
            JsonValue::Null | JsonValue::Bool(_) | JsonValue::String(_) => {}
        }
    }
    true
}

/// Validate and detach lossless JSON: a copy of the value, or `None` when it is not lossless.
pub fn snapshot_json_value(value: &JsonValue) -> Option<JsonValue> {
    is_json_value(value).then(|| value.clone())
}

fn numbers_equal(left: &Number, right: &Number) -> bool {
    if let (Some(a), Some(b)) = (left.as_i64(), right.as_i64()) {
        return a == b;
    }
    if let (Some(a), Some(b)) = (left.as_u64(), right.as_u64()) {
        return a == b;
    }
    left.as_f64() == right.as_f64()
}

/// Compare JSON-compatible values structurally. Numbers compare by value, so `1` equals `1.0`
/// (JavaScript has one number type); object key order does not matter.
pub fn deep_equal_json(a: &JsonValue, b: &JsonValue) -> bool {
    match (a, b) {
        (JsonValue::Number(x), JsonValue::Number(y)) => numbers_equal(x, y),
        (JsonValue::Array(x), JsonValue::Array(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|(left, right)| deep_equal_json(left, right))
        }
        (JsonValue::Object(x), JsonValue::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(key, left)| y.get(key).is_some_and(|right| deep_equal_json(left, right)))
        }
        (JsonValue::Null, JsonValue::Null) => true,
        (JsonValue::Bool(x), JsonValue::Bool(y)) => x == y,
        (JsonValue::String(x), JsonValue::String(y)) => x == y,
        _ => false,
    }
}

/// ECMAScript `Number::toString(10)`: `NaN`, `Infinity`, `-Infinity`, `0` for both zeros, decimal
/// notation for decimal exponents from -6 up to 20, otherwise exponent notation (`1e+21`, `1.5e-7`),
/// with the shortest digits that round-trip.
pub fn js_number_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value == 0.0 {
        return "0".to_owned();
    }
    if value.is_infinite() {
        return if value < 0.0 { "-Infinity" } else { "Infinity" }.to_owned();
    }
    let sign = if value < 0.0 { "-" } else { "" };
    // Rust's `{:e}` prints the shortest round-trip digits as `d.ddde<exp>`.
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let mut digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    digits = prefer_even_tie(value.abs(), digits, exponent);
    // `n` of the specification: the value is 0.<digits> * 10^n.
    let n = exponent.parse::<i32>().unwrap_or(0) + 1;
    let k = digits.len() as i32;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let exponent = format!("e{}{}", if e < 0 { '-' } else { '+' }, e.abs());
        if k == 1 {
            format!("{digits}{exponent}")
        } else {
            format!("{}.{}{exponent}", &digits[..1], &digits[1..])
        }
    };
    format!("{sign}{body}")
}

/// ECMAScript `String(value)` for a JSON value: strings unquoted, numbers through
/// [`js_number_string`], arrays joined with `,` (a `null` element is empty), objects `[object Object]`.
pub fn js_string(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => "null".to_owned(),
        JsonValue::Bool(flag) => flag.to_string(),
        JsonValue::Number(number) => js_number_string(number.as_f64().unwrap_or(f64::NAN)),
        JsonValue::String(text) => text.clone(),
        JsonValue::Array(items) => items
            .iter()
            .map(|item| {
                if item.is_null() {
                    String::new()
                } else {
                    js_string(item)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        JsonValue::Object(_) => "[object Object]".to_owned(),
    }
}

/// Among shortest digit strings that round-trip ECMAScript takes the closest to the value and, of two equally
/// close, the one with an even last digit; Rust's shortest formatter rounds such a tie up. When the exact
/// expansion of `value` is the shortest digits `D` followed by a single `5`, choose between `D` and `D + 1`.
fn prefer_even_tie(value: f64, digits: String, exponent: &str) -> String {
    let count = digits.len();
    let digits_of = |text: &str| -> (String, String) {
        let (mantissa, exp) = text.split_once('e').unwrap_or((text, "0"));
        (
            mantissa.chars().filter(char::is_ascii_digit).collect(),
            exp.to_owned(),
        )
    };
    // Cheap filter: a tie has `count + 1` significant digits ending in 5.
    let (probe, _) = digits_of(&format!("{:.*e}", count, value));
    if !probe.ends_with('5') {
        return digits;
    }
    let (exact, exact_exponent) = digits_of(&format!("{:.800e}", value));
    if exact_exponent != exponent || exact.len() <= count || exact.as_bytes()[count] != b'5' {
        return digits;
    }
    if !exact[count + 1..].bytes().all(|byte| byte == b'0') {
        return digits;
    }
    let lower = &exact[..count];
    let last = lower.as_bytes()[count - 1];
    let candidate = if (last - b'0').is_multiple_of(2) {
        lower.to_owned()
    } else if last == b'9' {
        return digits;
    } else {
        format!("{}{}", &lower[..count - 1], char::from(last + 1))
    };
    let text = format!("{}.{}e{}", &candidate[..1], &candidate[1..], exponent);
    if text.parse::<f64>().is_ok_and(|parsed| parsed == value) {
        candidate
    } else {
        digits
    }
}
