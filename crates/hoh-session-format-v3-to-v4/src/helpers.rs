use hoh_session_format::{
    SessionFormatError, SessionFormatEvent, SessionFormatJsonObject, session_format_count,
};
use hoh_util_values::{js_json_stringify, js_string};
use serde_json::Value;
pub(crate) type Result<T = ()> = std::result::Result<T, SessionFormatError>;
pub(crate) fn error(message: impl Into<String>) -> SessionFormatError {
    SessionFormatError::new(message)
}
pub(crate) fn refusal(message: impl Into<String>) -> SessionFormatError {
    SessionFormatError::unsupported_migration(message)
}
pub(crate) fn field<'a>(object: &'a SessionFormatJsonObject, key: &str) -> &'a Value {
    object.get(key).unwrap_or(&Value::Null)
}
pub(crate) fn kind(event: &SessionFormatEvent) -> &str {
    field(event, "type").as_str().unwrap_or("")
}
pub(crate) fn text(value: Option<&Value>) -> String {
    value.map(js_string).unwrap_or_else(|| "undefined".into())
}
pub(crate) fn quoted(value: Option<&Value>) -> String {
    value
        .map(js_json_stringify)
        .unwrap_or_else(|| "undefined".into())
}
pub(crate) fn subject(event: &SessionFormatEvent, version: u64) -> String {
    format!(
        "format v{version} {} at seq {}",
        text(event.get("type")),
        text(event.get("seq"))
    )
}
pub(crate) fn count(value: Option<&Value>, label: &str) -> Result<u64> {
    session_format_count(value.unwrap_or(&Value::Null), label)
}
pub(crate) fn number_eq(value: Option<&Value>, expected: u64) -> bool {
    value.and_then(Value::as_f64) == Some(expected as f64)
}
pub(crate) fn record<'a>(
    value: Option<&'a Value>,
    subject: &str,
) -> Result<&'a SessionFormatJsonObject> {
    value
        .and_then(Value::as_object)
        .ok_or_else(|| error(format!("{subject} requires an object")))
}
pub(crate) fn array<'a>(value: Option<&'a Value>, subject: &str) -> Result<&'a Vec<Value>> {
    value
        .and_then(Value::as_array)
        .ok_or_else(|| error(format!("{subject} requires an array")))
}
pub(crate) fn nonempty(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str).filter(|s| !s.is_empty())
}
pub(crate) fn earlier(value: Option<&Value>, seq: u64, subject: &str) -> Result<u64> {
    let n = count(value, subject)?;
    if n >= seq {
        return Err(error(format!("{subject} must name an earlier event")));
    }
    Ok(n)
}
pub(crate) fn event_at(events: &[SessionFormatEvent], seq: u64) -> Option<&SessionFormatEvent> {
    usize::try_from(seq).ok().and_then(|n| events.get(n))
}
pub(crate) fn strict_eq(left: Option<&Value>, right: Option<&Value>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(Value::Null), Some(Value::Null)) => true,
        (Some(Value::Number(a)), Some(Value::Number(b))) => a.as_f64() == b.as_f64(),
        (Some(Value::String(a)), Some(Value::String(b))) => a == b,
        (Some(Value::Bool(a)), Some(Value::Bool(b))) => a == b,
        (Some(a @ (Value::Object(_) | Value::Array(_))), Some(b)) => std::ptr::eq(a, b),
        _ => false,
    }
}
pub(crate) fn canonical_suffix(value: &str) -> Option<u64> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    value
        .parse::<u64>()
        .ok()
        .filter(|n| *n <= 9_007_199_254_740_991)
}
// SHIM(hoh-util-workspace-path): the direct path dependency is awaiting permission.
// Reuse the previous generation's public validator, which already owns the
// shared Node isAbsolute helper; replace with node_path::is_absolute directly.
pub(crate) fn absolute_cwd(value: &Value) -> bool {
    let Some(_) = value.as_str() else {
        return false;
    };
    let mut probe = SessionFormatJsonObject::new();
    probe.insert("version".into(), 3.into());
    probe.insert("id".into(), "".into());
    probe.insert("createdAt".into(), 0.into());
    probe.insert("isSeeded".into(), false.into());
    probe.insert("delegationDepth".into(), 0.into());
    probe.insert("cwd".into(), value.clone());
    hoh_session_format_v2_to_v3::assert_released_v3_header(&probe).is_ok()
}
