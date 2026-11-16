//! Port of `@deepseek-ai/dsh-util-values` (`packages/util/values`).
//!
//! JSON helpers over [`serde_json::Value`] and the lazy streamed-argument reader
//! [`PartialArguments`]. `assertNever`, `deepFreeze` and `WeakMapWithValues` have no
//! Rust counterpart; see `PORT_NOTES.md`.

pub mod partial_json;

use serde_json::Number;

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
