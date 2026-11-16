use hoh_util_values::{is_json_value, snapshot_json_value};
use serde_json::Value;

use crate::error::SessionFormatError;
use crate::types::{SessionFormatHeader, SessionFormatJsonObject, SessionFormatJsonValue};

/// `Number.MAX_SAFE_INTEGER`.
pub(crate) const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// Test whether a value is a non-null, non-array object.
pub fn is_session_format_json_object(value: &Value) -> bool {
    value.is_object()
}

/// A JSON number that is a non-negative safe integer and not negative zero.
fn safe_count(value: &Value) -> Option<u64> {
    let Value::Number(number) = value else {
        return None;
    };
    if let Some(unsigned) = number.as_u64() {
        return (unsigned <= MAX_SAFE_INTEGER).then_some(unsigned);
    }
    if number.is_i64() {
        return None;
    }
    let float = number.as_f64()?;
    let integral = float.is_finite() && float.fract() == 0.0;
    let negative_zero = float == 0.0 && float.is_sign_negative();
    // The range check keeps the cast exact.
    (integral && !negative_zero && (0.0..=MAX_SAFE_INTEGER as f64).contains(&float))
        .then_some(float as u64)
}

/// A JSON number that is a safe integer and not negative zero.
fn safe_integer(value: &Value) -> Option<i64> {
    let Value::Number(number) = value else {
        return None;
    };
    let limit = MAX_SAFE_INTEGER as i64;
    if let Some(signed) = number.as_i64() {
        return (-limit..=limit).contains(&signed).then_some(signed);
    }
    if number.is_u64() {
        return None;
    }
    let float = number.as_f64()?;
    let integral = float.is_finite() && float.fract() == 0.0;
    let negative_zero = float == 0.0 && float.is_sign_negative();
    (integral && !negative_zero && (-(limit as f64)..=limit as f64).contains(&float))
        .then_some(float as i64)
}

/// Require a non-negative safe integer without the JSON-unstable negative zero.
pub fn session_format_count(value: &Value, label: &str) -> Result<u64, SessionFormatError> {
    safe_count(value).ok_or_else(|| {
        SessionFormatError::new(format!("{label} must be a non-negative safe integer"))
    })
}

/// [`session_format_count`] for a value that is already a native integer.
pub fn session_format_count_u64(value: u64, label: &str) -> Result<u64, SessionFormatError> {
    if value <= MAX_SAFE_INTEGER {
        Ok(value)
    } else {
        Err(SessionFormatError::new(format!(
            "{label} must be a non-negative safe integer"
        )))
    }
}

/// Require a safe integer without the JSON-unstable negative zero.
pub fn session_format_safe_integer(value: &Value, label: &str) -> Result<i64, SessionFormatError> {
    safe_integer(value)
        .ok_or_else(|| SessionFormatError::new(format!("{label} must be a safe integer")))
}

/// Require a non-negative integral format version, labelled `Session format version`.
pub fn session_format_version(value: &Value) -> Result<u64, SessionFormatError> {
    session_format_version_labeled(value, "Session format version")
}

/// Require a non-negative integral format version with an explicit diagnostic subject.
pub fn session_format_version_labeled(
    value: &Value,
    label: &str,
) -> Result<u64, SessionFormatError> {
    session_format_count(value, label)
}

/// Read only the version required for directional dispatch.
pub fn inspect_session_format_version(header_value: &Value) -> Result<u64, SessionFormatError> {
    let Value::Object(object) = header_value else {
        return Err(SessionFormatError::new(
            "Session header must be a JSON object",
        ));
    };
    inspect_header_version(object)
}

/// [`inspect_session_format_version`] for a value already known to be an object.
pub(crate) fn inspect_header_version(
    header: &SessionFormatJsonObject,
) -> Result<u64, SessionFormatError> {
    session_format_version(header.get("version").unwrap_or(&Value::Null))
}

/// Detach a caller-supplied lossless JSON value, labelled `Session value`. Values are immutable
/// unless borrowed mutably, so there is no freezing step.
pub fn snapshot_session_format_json(
    value: &Value,
) -> Result<SessionFormatJsonValue, SessionFormatError> {
    snapshot_session_format_json_labeled(value, "Session value")
}

/// [`snapshot_session_format_json`] with an explicit diagnostic subject.
pub fn snapshot_session_format_json_labeled(
    value: &Value,
    label: &str,
) -> Result<SessionFormatJsonValue, SessionFormatError> {
    snapshot_json_value(value)
        .ok_or_else(|| SessionFormatError::new(format!("{label} is not lossless JSON")))
}

/// Snapshot one logical header without inspecting an event body, labelled `Session header`.
pub fn snapshot_session_format_header(
    header: &SessionFormatHeader,
) -> Result<SessionFormatHeader, SessionFormatError> {
    snapshot_session_format_header_labeled(header, "Session header")
}

/// [`snapshot_session_format_header`] with an explicit diagnostic subject.
pub fn snapshot_session_format_header_labeled(
    header: &SessionFormatHeader,
    label: &str,
) -> Result<SessionFormatHeader, SessionFormatError> {
    if !header.values().all(is_json_value) {
        return Err(SessionFormatError::new(format!(
            "{label} is not lossless JSON"
        )));
    }
    let snapshot = header.clone();
    inspect_header_version(&snapshot)?;
    if !snapshot.get("id").is_some_and(Value::is_string) {
        return Err(SessionFormatError::new(format!(
            "{label} id must be a string"
        )));
    }
    session_format_count(
        snapshot.get("createdAt").unwrap_or(&Value::Null),
        &format!("{label} createdAt"),
    )?;
    if !snapshot.get("isSeeded").is_some_and(Value::is_boolean) {
        return Err(SessionFormatError::new(format!(
            "{label} isSeeded must be a boolean"
        )));
    }
    session_format_count(
        snapshot.get("delegationDepth").unwrap_or(&Value::Null),
        &format!("{label} delegationDepth"),
    )?;
    Ok(snapshot)
}
