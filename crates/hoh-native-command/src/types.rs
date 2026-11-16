use crate::runner::NativeCommandError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeFileApplication {
    pub id: String,
    pub name: String,
    pub default: bool,
    pub icon: Option<String>,
}

pub fn parse_native_file_applications(
    value: &Value,
) -> Result<Vec<NativeFileApplication>, NativeCommandError> {
    let entries = value
        .as_array()
        .ok_or_else(|| NativeCommandError::message("Invalid native application list"))?;
    entries
        .iter()
        .map(|entry| {
            let invalid = || NativeCommandError::message("Invalid native application entry");
            let id = entry
                .get("id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(invalid)?;
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(invalid)?;
            let default = entry
                .get("default")
                .and_then(Value::as_bool)
                .ok_or_else(invalid)?;
            let icon = match entry.get("icon") {
                Some(Value::Null) => None,
                Some(Value::String(icon)) if valid_icon(icon) => Some(icon.clone()),
                _ => return Err(invalid()),
            };
            Ok(NativeFileApplication {
                id: id.into(),
                name: name.into(),
                default,
                icon,
            })
        })
        .collect()
}

fn valid_icon(icon: &str) -> bool {
    let data = icon
        .strip_prefix("data:image/png;base64,")
        .or_else(|| icon.strip_prefix("data:image/svg+xml;base64,"));
    data.is_some_and(|data| {
        !data.is_empty()
            && data
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
    })
}
