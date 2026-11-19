//! Session projection recording the preset that the session actually runs.

use hoh_error::BoxError;
use hoh_session::SessionLogOffset;
use hoh_session_projection::{ProjectionDefinition, ProjectionWire};
use serde_json::Value;
use std::sync::Arc;

fn projection_error(message: impl Into<String>) -> BoxError {
    Box::new(crate::RegistryError(message.into()))
}

fn parse_state(value: Option<&Value>) -> Result<Arc<Option<String>>, BoxError> {
    match value {
        Some(Value::Null) => Ok(Arc::new(None)),
        Some(Value::String(value)) => Ok(Arc::new(Some(value.clone()))),
        _ => Err(projection_error(
            "agentPreset projection state must be a string or null",
        )),
    }
}

/// Current preset, initialized from the creation header and advanced by selection events.
pub fn agent_preset_projection_definition() -> ProjectionDefinition<Option<String>> {
    ProjectionDefinition {
        key: "agentPreset".into(),
        state_schema: Arc::new(parse_state),
        init: Arc::new(|header, _inherited: SessionLogOffset| {
            Ok(Arc::new(header.agent_preset.clone()))
        }),
        apply: Arc::new(|state, event| {
            if event.event_type() != "agent-preset/selected" {
                return Ok(state.clone());
            }
            let selected = event
                .data()
                .get("agentPreset")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    projection_error("agent-preset/selected requires a string agentPreset")
                })?;
            Ok(Arc::new(Some(selected.to_owned())))
        }),
        wire: Some(ProjectionWire::new(
            Arc::new(|value| match value {
                Some(Value::Null | Value::String(_)) => {
                    Ok(Arc::new(value.cloned().unwrap_or(Value::Null)))
                }
                _ => Err(projection_error(
                    "agentPreset projection view must be a string or null",
                )),
            }),
            Arc::new(|state| {
                Ok(Some(Arc::new(
                    state.as_ref().clone().map_or(Value::Null, Value::String),
                )))
            }),
        )),
        state_version: 1,
    }
}
