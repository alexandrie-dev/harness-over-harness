use hoh_session_format::{SessionFormatError, SessionFormatJsonObject};
use hoh_util_values::{js_json_stringify, js_string};
use serde_json::Value;

pub fn released_v0_record<'a>(
    value: &'a Value,
    label: &str,
) -> Result<&'a SessionFormatJsonObject, SessionFormatError> {
    value
        .as_object()
        .ok_or_else(|| SessionFormatError::new(format!("{label} must be a JSON object")))
}

pub fn assert_released_v0_keys(
    record: &SessionFormatJsonObject,
    required: &[&str],
    optional: &[&str],
    label: &str,
) -> Result<(), SessionFormatError> {
    if let Some(key) = js_keys(record)
        .into_iter()
        .find(|key| !required.contains(&key.as_str()) && !optional.contains(&key.as_str()))
    {
        return Err(SessionFormatError::new(format!(
            "{label} has unexpected member {}",
            js_json_stringify(&Value::String(key.clone()))
        )));
    }
    if let Some(key) = required.iter().find(|key| !record.contains_key(**key)) {
        return Err(SessionFormatError::new(format!(
            "{label} lacks required member {}",
            js_json_stringify(&Value::String((*key).to_owned()))
        )));
    }
    Ok(())
}

pub(crate) fn field<'a>(record: &'a SessionFormatJsonObject, key: &str) -> &'a Value {
    record.get(key).unwrap_or(&Value::Null)
}

pub(crate) fn record<'a>(value: &'a Value, label: &str) -> Result<&'a Value, SessionFormatError> {
    released_v0_record(value, label)?;
    Ok(value)
}

pub(crate) fn keys(
    value: &Value,
    required: &[&str],
    optional: &[&str],
    label: &str,
) -> Result<(), SessionFormatError> {
    assert_released_v0_keys(released_v0_record(value, label)?, required, optional, label)
}

pub(crate) fn js_display(value: Option<&Value>) -> String {
    value.map(js_string).unwrap_or_else(|| "undefined".into())
}

fn array_index(key: &str) -> Option<u32> {
    key.parse::<u32>()
        .ok()
        .filter(|n| *n != u32::MAX && n.to_string() == key)
}
fn js_keys(object: &SessionFormatJsonObject) -> Vec<&String> {
    let mut numeric: Vec<_> = object
        .keys()
        .filter_map(|key| array_index(key).map(|n| (n, key)))
        .collect();
    numeric.sort_by_key(|(n, _)| *n);
    numeric
        .into_iter()
        .map(|(_, key)| key)
        .chain(object.keys().filter(|key| array_index(key).is_none()))
        .collect()
}

pub(crate) fn event_type(event: &SessionFormatJsonObject) -> &str {
    field(event, "type").as_str().unwrap_or("")
}
/// Exact JSON coordinate conversion for lookups; invalid values never alias event 0.
pub(crate) fn exact_u64(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        let number = value.as_f64()?;
        if number.fract() != 0.0 || !(0.0..=9_007_199_254_740_992.0).contains(&number) {
            return None;
        }
        // The checked integral range makes this conversion exact, including -0 -> 0.
        Some(number as u64)
    })
}
pub(crate) fn exact_index(value: &Value) -> Option<usize> {
    exact_u64(value).and_then(|number| usize::try_from(number).ok())
}
pub(crate) fn event_seq(event: &SessionFormatJsonObject) -> Option<u64> {
    event.get("seq").and_then(exact_u64)
}
pub(crate) fn event_seq_text(event: &SessionFormatJsonObject) -> String {
    js_display(event.get("seq"))
}
pub(crate) fn event_type_text(event: &SessionFormatJsonObject) -> String {
    js_display(event.get("type"))
}
pub(crate) fn event_label(event: &SessionFormatJsonObject) -> String {
    format!("{} {}", event_type_text(event), event_seq_text(event))
}
pub(crate) fn has(value: &Value, key: &str) -> bool {
    value.as_object().is_some_and(|o| o.contains_key(key))
}
