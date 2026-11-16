use hoh_session_format::{SessionFormatError, SessionFormatJsonObject};
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
            js_stringify(&Value::String(key.clone()))
        )));
    }
    if let Some(key) = required.iter().find(|key| !record.contains_key(**key)) {
        return Err(SessionFormatError::new(format!(
            "{label} lacks required member {}",
            js_stringify(&Value::String((*key).to_owned()))
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

/// JSON.stringify-compatible member ordering and integral-number spelling for framed payloads.
pub(crate) fn js_stringify(value: &Value) -> String {
    match value {
        Value::Object(object) => {
            let ordered = js_keys(object);
            format!(
                "{{{}}}",
                ordered
                    .into_iter()
                    .map(|key| format!(
                        "{}:{}",
                        Value::String(key.clone()),
                        js_stringify(&object[key])
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        Value::Array(array) => format!(
            "[{}]",
            array.iter().map(js_stringify).collect::<Vec<_>>().join(",")
        ),
        Value::Number(n) if n.is_f64() => {
            let f = n.as_f64().unwrap_or(0.0);
            if f == 0.0 {
                return "0".into();
            }
            if f.fract() == 0.0 && f.abs() < 1e21 {
                format!("{f:.0}")
            } else {
                value.to_string()
            }
        }
        _ => value.to_string(),
    }
}

pub(crate) fn js_display(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(_)) => "[object Object]".into(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| {
                if v.is_null() {
                    String::new()
                } else {
                    js_display(Some(v))
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(v) => js_stringify(v),
    }
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

/// Represent JS object enumeration and safe integral numbers in serde's JSON representation.
/// Negative zero stays intact until its owning validator can reject it.
pub(crate) fn normalize_json_layout(value: &mut Value) {
    match value {
        Value::Number(number) if number.is_f64() => {
            if let Some(f) = number.as_f64()
                && f.fract() == 0.0
                && !(f == 0.0 && f.is_sign_negative())
                && f.abs() <= 9_007_199_254_740_991.0
            {
                *value = Value::from(f as i64);
            }
        }
        Value::Array(members) => members.iter_mut().for_each(normalize_json_layout),
        Value::Object(object) => {
            let ordered: Vec<_> = js_keys(object).into_iter().cloned().collect();
            let mut output = SessionFormatJsonObject::new();
            for key in ordered {
                if let Some(mut member) = object.shift_remove(&key) {
                    normalize_json_layout(&mut member);
                    output.insert(key, member);
                }
            }
            *object = output;
        }
        Value::Null | Value::Bool(_) | Value::String(_) | Value::Number(_) => {}
    }
}

pub(crate) fn event_type(event: &SessionFormatJsonObject) -> &str {
    field(event, "type").as_str().unwrap_or("")
}
pub(crate) fn event_seq(event: &SessionFormatJsonObject) -> u64 {
    field(event, "seq")
        .as_u64()
        .or_else(|| field(event, "seq").as_f64().map(|v| v as u64))
        .unwrap_or(0)
}
pub(crate) fn has(value: &Value, key: &str) -> bool {
    value.as_object().is_some_and(|o| o.contains_key(key))
}
