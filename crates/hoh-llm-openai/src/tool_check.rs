//! Strict tool calling: a finished tool call is checked before the agent loop sees it.
use hoh_llm::ToolSchema;
use serde_json::Value;

/// Why a tool call cannot be executed; `None` arguments are normalized to `{}` by the caller.
pub fn check_tool_call(
    name: &str,
    arguments: &str,
    tools: Option<&[ToolSchema]>,
) -> Result<(), String> {
    if name.is_empty() {
        return Err("no tool name".into());
    }
    let declared = tools.and_then(|tools| tools.iter().find(|tool| tool.name == name));
    let Some(tool) = declared else {
        return Err(format!("the name {name:?}, which is not a declared tool"));
    };
    let value: Value = serde_json::from_str(arguments)
        .map_err(|e| format!("arguments that are not valid JSON ({e})"))?;
    let Value::Object(object) = value else {
        return Err("arguments that are not a JSON object".into());
    };
    let missing: Vec<&str> = tool
        .parameters
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|key| !object.contains_key(*key))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "no value for required parameter(s): {}",
            missing.join(", ")
        ))
    }
}

/// Empty arguments are what servers send for a tool without parameters.
pub fn normalize_arguments(arguments: &str) -> String {
    if arguments.trim().is_empty() {
        "{}".into()
    } else {
        arguments.to_owned()
    }
}
