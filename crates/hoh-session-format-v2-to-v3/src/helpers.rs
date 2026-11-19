use hoh_session_format::{SessionFormatError, SessionFormatEvent, SessionFormatJsonObject};
use hoh_util_values::{js_json_stringify, js_string};
use serde_json::Value;
pub(crate) type Result<T = ()> = std::result::Result<T, SessionFormatError>;
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
pub(crate) fn error(value: impl Into<String>) -> SessionFormatError {
    SessionFormatError::new(value)
}
pub(crate) fn refusal(value: impl Into<String>) -> SessionFormatError {
    SessionFormatError::unsupported_migration(value)
}
pub(crate) fn number_eq(value: &Value, expected: u64) -> bool {
    value.as_f64() == Some(expected as f64)
}
pub(crate) fn object(value: Value) -> SessionFormatJsonObject {
    // Every caller constructs an object literal with json!.
    value.as_object().expect("object literal").clone()
}
pub(crate) fn subject(event: &SessionFormatEvent) -> String {
    format!(
        "format v3 {} at seq {}",
        text(event.get("type")),
        text(event.get("seq"))
    )
}
