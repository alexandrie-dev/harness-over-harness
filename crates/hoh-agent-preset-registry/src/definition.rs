//! Declarative preset configuration and entry-list validation.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Identity, display fields and child plugins of one preset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PresetDefinition {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<f64>,
    /// Raw Loader rows. Keeping the value raw preserves `!!js` nodes until mounting.
    pub plugins: Value,
}

/// Return the first malformed Loader row, including nested groups.
pub fn entry_list_problem(rows: &Value) -> Option<String> {
    entry_list_problem_at(rows, "")
}

fn entry_list_problem_at(rows: &Value, at: &str) -> Option<String> {
    let Some(rows) = rows.as_array() else {
        return Some(if at.is_empty() {
            "the composition must be a top-level list of plugin rows".into()
        } else {
            format!("group {at} must hold a list of plugin rows")
        });
    };
    for (index, row) in rows.iter().enumerate() {
        let label = if at.is_empty() {
            format!("row {}", index + 1)
        } else {
            format!("{at} row {}", index + 1)
        };
        let Some(row) = row.as_object() else {
            return Some(format!(
                "{label} is not a plugin row (expected a map with a \"name\")"
            ));
        };
        if row
            .get("name")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Some(format!(
                "{label} names no plugin (a \"name\" string is required)"
            ));
        }
        if row.get("group") == Some(&Value::Bool(true))
            && let Some(problem) =
                entry_list_problem_at(row.get("config").unwrap_or(&Value::Null), &label)
        {
            return Some(problem);
        }
    }
    None
}
