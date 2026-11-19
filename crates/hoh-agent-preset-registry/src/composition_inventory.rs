//! Plugin inventory from a declaration or an activated revision.

use crate::{definition::entry_list_problem, mount::PresetMount};
use hoh_error::BoxError;
use hoh_kernel::FiberState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CompositionRowEnablement {
    Enabled(bool),
    Conditional(ConditionalEnablement),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConditionalEnablement {
    #[serde(rename = "conditional")]
    Conditional,
}

impl CompositionRowEnablement {
    pub const fn conditional() -> Self {
        Self::Conditional(ConditionalEnablement::Conditional)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentPresetCompositionRow {
    pub entry_id: Option<String>,
    pub module_name: String,
    pub enabled: CompositionRowEnablement,
    pub condition: Option<String>,
    pub fiber_state: Option<FiberState>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentPresetComposition {
    pub id: String,
    pub name: Option<String>,
    /// Runtime `compositionInventory()` includes this field although the source interface omitted it.
    pub description: Option<String>,
    pub is_default: bool,
    pub broken: Option<String>,
    pub rows: Vec<AgentPresetCompositionRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentPresetModule {
    pub module_name: String,
    pub base_url: Option<String>,
    pub use_host_base: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentPresetInspection {
    pub id: String,
    pub modules: Vec<AgentPresetModule>,
    pub leaked_services: Vec<String>,
}

pub type DisabledExpressionEvaluator =
    dyn Fn(&str) -> Result<Value, BoxError> + Send + Sync + 'static;

fn js_expression(value: &Value) -> Option<&str> {
    let object = value.as_object()?;
    (object.len() == 1)
        .then(|| object.get("__jsExpr")?.as_str())
        .flatten()
}

fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn disabled_contribution(
    value: Option<&Value>,
    evaluate: &DisabledExpressionEvaluator,
) -> CompositionRowEnablement {
    let value = value.unwrap_or(&Value::Null);
    if let Some(expression) = js_expression(value) {
        return evaluate(expression)
            .map(|value| CompositionRowEnablement::Enabled(js_truthy(&value)))
            .unwrap_or_else(|_| CompositionRowEnablement::conditional());
    }
    CompositionRowEnablement::Enabled(js_truthy(value))
}

fn combine_disabled(
    outer: CompositionRowEnablement,
    own: CompositionRowEnablement,
) -> CompositionRowEnablement {
    match (outer, own) {
        (CompositionRowEnablement::Enabled(true), _)
        | (_, CompositionRowEnablement::Enabled(true)) => CompositionRowEnablement::Enabled(true),
        (CompositionRowEnablement::Conditional(_), _)
        | (_, CompositionRowEnablement::Conditional(_)) => CompositionRowEnablement::conditional(),
        _ => CompositionRowEnablement::Enabled(false),
    }
}

fn flatten_rows(
    rows: &[Value],
    outer_disabled: CompositionRowEnablement,
    evaluate: &DisabledExpressionEvaluator,
    found: &mut Vec<AgentPresetCompositionRow>,
) {
    for value in rows {
        let row = value
            .as_object()
            .expect("entry_list_problem accepted every row as an object");
        let disabled = combine_disabled(
            outer_disabled.clone(),
            disabled_contribution(row.get("disabled"), evaluate),
        );
        if row.get("group") == Some(&Value::Bool(true)) {
            flatten_rows(
                row.get("config")
                    .and_then(Value::as_array)
                    .expect("entry_list_problem accepted group children"),
                disabled,
                evaluate,
                found,
            );
            continue;
        }
        let enabled = match disabled {
            CompositionRowEnablement::Enabled(disabled) => {
                CompositionRowEnablement::Enabled(!disabled)
            }
            CompositionRowEnablement::Conditional(_) => CompositionRowEnablement::conditional(),
        };
        found.push(AgentPresetCompositionRow {
            entry_id: row
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_owned),
            module_name: row["name"]
                .as_str()
                .expect("entry_list_problem accepted the name")
                .to_owned(),
            enabled,
            condition: row
                .get("disabled")
                .and_then(js_expression)
                .map(str::to_owned),
            fiber_state: None,
        });
    }
}

pub fn definition_composition(
    rows: &Value,
    evaluate: &DisabledExpressionEvaluator,
) -> Result<Vec<AgentPresetCompositionRow>, String> {
    if let Some(problem) = entry_list_problem(rows) {
        return Err(problem);
    }
    let mut found = Vec::new();
    flatten_rows(
        rows.as_array().expect("entry list was validated"),
        CompositionRowEnablement::Enabled(false),
        evaluate,
        &mut found,
    );
    Ok(found)
}

pub fn mounted_composition_rows(mount: &PresetMount) -> Vec<AgentPresetCompositionRow> {
    mount
        .tree
        .entries()
        .into_iter()
        .filter(|entry| !entry.options.group)
        .map(|entry| AgentPresetCompositionRow {
            entry_id: Some(entry.options.id.clone()),
            module_name: entry.options.name.clone(),
            enabled: CompositionRowEnablement::Enabled(!entry.disabled),
            condition: js_expression(&entry.options.disabled).map(str::to_owned),
            fiber_state: entry.fiber.map(|fiber| fiber.state()),
        })
        .collect()
}

pub fn active_composition_modules(mount: &PresetMount) -> Vec<AgentPresetModule> {
    mount
        .tree
        .entries()
        .into_iter()
        .filter(|entry| {
            !entry.options.group
                && !entry.disabled
                && entry
                    .fiber
                    .as_ref()
                    .is_some_and(|fiber| fiber.state() == FiberState::Active)
        })
        .map(|entry| AgentPresetModule {
            module_name: entry.options.name,
            // SHIM(hoh-loader): EntryInfo does not expose the declaring base URL.
            base_url: None,
            // All rows mounted by this private Group use the runtime's host base.
            use_host_base: true,
        })
        .collect()
}
