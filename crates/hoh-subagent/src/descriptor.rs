//! Durable `subagent/descriptor` event vocabulary.

use std::sync::Arc;

use hoh_error::BoxError;
use hoh_llm::ReasoningEffortId;
use hoh_session::SessionEvent;
use hoh_tools::ToolRestriction;
use hoh_util_values::snapshot_json_value;
use serde_json::{Map, Value};

pub const SUBAGENT_DESCRIPTOR_VERSION: u64 = 3;

#[derive(Clone, Debug)]
pub struct OneShotSubagentDescriptorData {
    pub provider: String,
    pub label: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ContinuableSubagentDescriptorData {
    pub provider: String,
    pub label: String,
    pub agent_provider: Option<String>,
    pub agent_model: Option<String>,
    pub agent_reasoning_effort: Option<ReasoningEffortId>,
    pub persona: Option<String>,
    pub tool_filter: Option<ToolRestriction>,
}

/// Pre-snapshot descriptor fields; the Rust data type stores no wire version,
/// so it is also the source's `ContinuableSubagentDescriptorInput` shape.
pub type ContinuableSubagentDescriptorInput = ContinuableSubagentDescriptorData;

#[derive(Clone, Debug)]
pub enum SubagentDescriptorData {
    OneShot(OneShotSubagentDescriptorData),
    Continuable(ContinuableSubagentDescriptorData),
}

fn record(value: &Value) -> Result<&Map<String, Value>, BoxError> {
    value
        .as_object()
        .ok_or_else(|| "persisted subagent descriptor payload must be an object".into())
}

fn assert_known_keys(
    value: &Map<String, Value>,
    keys: &[&str],
    path: &str,
) -> Result<(), BoxError> {
    if let Some(unknown) = value.keys().find(|key| !keys.contains(&key.as_str())) {
        return Err(format!(
            "persisted subagent descriptor {path} has unknown field \"{unknown}\""
        )
        .into());
    }
    Ok(())
}

fn optional_string(value: &Map<String, Value>, key: &str) -> Result<Option<String>, BoxError> {
    let Some(field) = value.get(key) else {
        return Ok(None);
    };
    field
        .as_str()
        .map(|text| Some(text.into()))
        .ok_or_else(|| format!("persisted subagent descriptor {key} must be a string").into())
}

fn optional_string_array(
    value: &Map<String, Value>,
    key: &str,
) -> Result<Option<Vec<String>>, BoxError> {
    let Some(field) = value.get(key) else {
        return Ok(None);
    };
    let items = field.as_array().ok_or_else(|| -> BoxError {
        format!("persisted subagent descriptor toolFilter.{key} must be an array of strings").into()
    })?;
    items
        .iter()
        .map(|item| {
            item.as_str().map(str::to_owned).ok_or_else(|| -> BoxError {
                format!(
                    "persisted subagent descriptor toolFilter.{key} must be an array of strings"
                )
                .into()
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn parse_tool_filter(value: &Value) -> Result<ToolRestriction, BoxError> {
    let value = value.as_object().ok_or_else(|| -> BoxError {
        "persisted subagent descriptor toolFilter must be an object".into()
    })?;
    assert_known_keys(value, &["allow", "deny"], "toolFilter")?;
    let allow = optional_string_array(value, "allow")?;
    let deny = optional_string_array(value, "deny")?;
    if allow.is_none() && deny.is_none() {
        return Err(
            "persisted subagent descriptor toolFilter must declare allow and/or deny".into(),
        );
    }
    Ok(ToolRestriction { allow, deny })
}

fn parse(value: &Value) -> Result<Option<SubagentDescriptorData>, BoxError> {
    let value = record(value)?;
    let version = value
        .get("version")
        .and_then(Value::as_f64)
        .ok_or_else(|| -> BoxError {
            "persisted subagent descriptor version must be a number".into()
        })?;
    if version != SUBAGENT_DESCRIPTOR_VERSION as f64 {
        return Ok(None);
    }
    let mode = value
        .get("mode")
        .and_then(Value::as_str)
        .ok_or_else(|| -> BoxError {
            "persisted subagent descriptor mode must be \"one-shot\" or \"continuable\"".into()
        })?;
    if !matches!(mode, "one-shot" | "continuable") {
        return Err(
            "persisted subagent descriptor mode must be \"one-shot\" or \"continuable\"".into(),
        );
    }
    let keys = if mode == "one-shot" {
        &["version", "mode", "provider", "label"][..]
    } else {
        &[
            "version",
            "mode",
            "provider",
            "label",
            "agentProvider",
            "agentModel",
            "agentReasoningEffort",
            "persona",
            "toolFilter",
        ][..]
    };
    assert_known_keys(value, keys, "payload")?;
    let provider = value
        .get("provider")
        .and_then(Value::as_str)
        .ok_or_else(|| -> BoxError {
            "persisted subagent descriptor provider must be a string".into()
        })?
        .to_owned();
    if mode == "one-shot" {
        return Ok(Some(SubagentDescriptorData::OneShot(
            OneShotSubagentDescriptorData {
                provider,
                label: optional_string(value, "label")?,
            },
        )));
    }
    let label = value
        .get("label")
        .and_then(Value::as_str)
        .ok_or_else(|| -> BoxError {
            "persisted subagent descriptor label must be a string".into()
        })?
        .to_owned();
    Ok(Some(SubagentDescriptorData::Continuable(
        ContinuableSubagentDescriptorData {
            provider,
            label,
            agent_provider: optional_string(value, "agentProvider")?,
            agent_model: optional_string(value, "agentModel")?,
            agent_reasoning_effort: optional_string(value, "agentReasoningEffort")?
                .map(ReasoningEffortId::from),
            persona: optional_string(value, "persona")?,
            tool_filter: value.get("toolFilter").map(parse_tool_filter).transpose()?,
        },
    )))
}

pub fn descriptor_value(input: &SubagentDescriptorData) -> Value {
    let mut value = Map::new();
    value.insert("version".into(), SUBAGENT_DESCRIPTOR_VERSION.into());
    match input {
        SubagentDescriptorData::OneShot(input) => {
            value.insert("mode".into(), "one-shot".into());
            value.insert("provider".into(), input.provider.clone().into());
            if let Some(label) = &input.label {
                value.insert("label".into(), label.clone().into());
            }
        }
        SubagentDescriptorData::Continuable(input) => {
            value.insert("mode".into(), "continuable".into());
            value.insert("provider".into(), input.provider.clone().into());
            value.insert("label".into(), input.label.clone().into());
            for (key, field) in [
                ("agentProvider", input.agent_provider.as_deref()),
                ("agentModel", input.agent_model.as_deref()),
                (
                    "agentReasoningEffort",
                    input.agent_reasoning_effort.as_ref().map(AsRef::as_ref),
                ),
                ("persona", input.persona.as_deref()),
            ] {
                if let Some(field) = field {
                    value.insert(key.into(), field.into());
                }
            }
            if let Some(filter) = &input.tool_filter {
                let mut object = Map::new();
                if let Some(allow) = &filter.allow {
                    object.insert(
                        "allow".into(),
                        Value::Array(allow.iter().cloned().map(Value::String).collect()),
                    );
                }
                if let Some(deny) = &filter.deny {
                    object.insert(
                        "deny".into(),
                        Value::Array(deny.iter().cloned().map(Value::String).collect()),
                    );
                }
                value.insert("toolFilter".into(), Value::Object(object));
            }
        }
    }
    Value::Object(value)
}

pub fn snapshot_subagent_descriptor(
    input: &ContinuableSubagentDescriptorData,
) -> Result<ContinuableSubagentDescriptorData, BoxError> {
    let value = descriptor_value(&SubagentDescriptorData::Continuable(input.clone()));
    let snapshot = snapshot_json_value(&value).ok_or_else(|| -> BoxError {
        "subagent descriptor is not losslessly JSON-serializable".into()
    })?;
    match parse(&snapshot)? {
        Some(SubagentDescriptorData::Continuable(value)) => Ok(value),
        _ => Err("subagent descriptor is not losslessly JSON-serializable".into()),
    }
}

pub fn fold_subagent_descriptor(
    events: &[Arc<SessionEvent>],
) -> Result<Option<SubagentDescriptorData>, BoxError> {
    let Some(event) = events
        .iter()
        .find(|event| event.event_type() == "subagent/descriptor")
    else {
        return Ok(None);
    };
    parse(event.data())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(data: Value) -> Arc<SessionEvent> {
        Arc::new(SessionEvent::from_value(json!({
            "seq": 0, "time": 1, "type": "subagent/descriptor", "data": data
        })))
    }

    #[test]
    fn unknown_descriptor_version_is_ignored() {
        let events = [event(json!({ "version": 4, "mode": "continuable" }))];
        assert!(fold_subagent_descriptor(&events).unwrap().is_none());
    }

    #[test]
    fn continuable_round_trip_preserves_all_fields_and_ordered_shape() {
        let input = ContinuableSubagentDescriptorData {
            provider: "local".into(),
            label: "research".into(),
            agent_provider: Some("openai".into()),
            agent_model: Some("gpt".into()),
            agent_reasoning_effort: Some("high".into()),
            persona: Some("precise".into()),
            tool_filter: Some(ToolRestriction {
                allow: Some(vec!["read".into()]),
                deny: Some(vec!["bash".into()]),
            }),
        };
        let snapshot = snapshot_subagent_descriptor(&input).unwrap();
        let value = descriptor_value(&SubagentDescriptorData::Continuable(snapshot));
        assert_eq!(value["version"], 3);
        assert_eq!(value["mode"], "continuable");
        assert_eq!(value["toolFilter"]["allow"], json!(["read"]));
    }

    #[test]
    fn current_version_rejects_unknown_fields_and_empty_filter() {
        for data in [
            json!({ "version": 3, "mode": "continuable", "provider": "p", "label": "l", "extra": true }),
            json!({ "version": 3, "mode": "continuable", "provider": "p", "label": "l", "toolFilter": {} }),
        ] {
            assert!(fold_subagent_descriptor(&[event(data)]).is_err());
        }
    }

    #[test]
    fn first_descriptor_event_is_authoritative() {
        let events = [
            event(json!({ "version": 3, "mode": "one-shot", "provider": "first" })),
            event(json!({ "version": 3, "mode": "one-shot", "provider": "second" })),
        ];
        let Some(SubagentDescriptorData::OneShot(value)) =
            fold_subagent_descriptor(&events).unwrap()
        else {
            panic!("one-shot")
        };
        assert_eq!(value.provider, "first");
    }
}
