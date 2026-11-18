use hoh_session_format::{
    SessionFormatError, SessionFormatEvent, SessionFormatJsonObject, session_format_count,
};
use hoh_util_values::{js_json_stringify, js_string};
use serde_json::{Map, Value};
pub(crate) type Result<T = ()> = std::result::Result<T, SessionFormatError>;
pub(crate) fn field<'a>(object: &'a Map<String, Value>, key: &str) -> &'a Value {
    object.get(key).unwrap_or(&Value::Null)
}
pub(crate) fn kind(event: &SessionFormatEvent) -> &str {
    field(event, "type").as_str().unwrap_or("")
}
pub(crate) fn text(value: Option<&Value>) -> String {
    value.map(js_string).unwrap_or_else(|| "undefined".into())
}
pub(crate) fn label(event: &SessionFormatEvent) -> String {
    format!("{} {}", text(event.get("type")), text(event.get("seq")))
}
pub(crate) fn seq(event: &SessionFormatEvent) -> Result<u64> {
    session_format_count(field(event, "seq"), "Session event seq")
}
pub(crate) fn refusal(message: impl Into<String>) -> SessionFormatError {
    SessionFormatError::unsupported_migration(message)
}
pub(crate) fn error(message: impl Into<String>) -> SessionFormatError {
    SessionFormatError::new(message)
}
pub(crate) fn object(value: Value) -> Result<SessionFormatJsonObject> {
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(error("Session event must be an object")),
    }
}
pub(crate) fn exact_coordinate(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        let f = value.as_f64()?;
        if f.fract() != 0.0 || !(0.0..=9_007_199_254_740_992.0).contains(&f) {
            return None;
        }
        Some(f as u64)
    })
}
pub(crate) fn number_eq(value: &Value, expected: u64) -> bool {
    value.as_f64() == Some(expected as f64)
}
pub(crate) fn ordered_keys(object: &Map<String, Value>) -> Vec<&String> {
    let index = |key: &str| {
        key.parse::<u32>()
            .ok()
            .filter(|n| *n != u32::MAX && n.to_string() == key)
    };
    let mut keys: Vec<_> = object.keys().collect();
    keys.sort_by_key(|key| index(key).map_or((1, 0), |i| (0, i)));
    keys
}
pub(crate) fn quoted(value: Option<&Value>) -> String {
    value
        .map(js_json_stringify)
        .unwrap_or_else(|| "undefined".into())
}
