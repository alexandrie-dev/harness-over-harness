use crate::model_selection::{AllowedModelRoute, routes_from_value};
use hoh_error::BoxError;
use hoh_session::{Session, SessionEvent, SessionLogOffset};
use hoh_session_projection::{ProjectionDefinition, ProjectionHandle};
use serde_json::Value;
use std::sync::Arc;

pub type ModelSelectionPolicyState = Option<Vec<AllowedModelRoute>>;

/// `z.array(z.object({provider, model}).strict()).min(1).nullable()` of the source: strict route
/// objects with non-empty ids; duplicates are caught only by `apply`.
fn parse_state(value: Option<&Value>) -> Result<Arc<ModelSelectionPolicyState>, BoxError> {
    match value {
        Some(Value::Null) => Ok(Arc::new(None)),
        Some(Value::Array(items)) if !items.is_empty() => {
            let mut routes = Vec::with_capacity(items.len());
            for item in items {
                let object = item
                    .as_object()
                    .filter(|object| object.len() == 2)
                    .ok_or("subagent model selection projection state must hold strict routes")?;
                let text = |name: &str| match object.get(name) {
                    Some(Value::String(text)) if !text.is_empty() => Some(text.clone()),
                    _ => None,
                };
                let (Some(provider), Some(model)) = (text("provider"), text("model")) else {
                    return Err(
                        "subagent model selection projection state must hold strict routes".into(),
                    );
                };
                routes.push(AllowedModelRoute { provider, model });
            }
            Ok(Arc::new(Some(routes)))
        }
        _ => Err(
            "subagent model selection projection state must be a non-empty array or null".into(),
        ),
    }
}

pub fn subagent_model_selection_projection_definition()
-> ProjectionDefinition<ModelSelectionPolicyState> {
    ProjectionDefinition {
        key: "subagentModelSelectionPolicy".into(),
        state_schema: Arc::new(parse_state),
        init: Arc::new(|_, _: SessionLogOffset| Ok(Arc::new(None))),
        apply: Arc::new(|state, event: &SessionEvent| {
            if state.is_some() || event.event_type() != "subagent/model-selection-policy" {
                return Ok(state.clone());
            }
            let routes = routes_from_value(event.data().get("allowedModels"))?;
            if routes.is_empty() {
                return Err("subagent/model-selection-policy requires at least one route".into());
            }
            Ok(Arc::new(Some(routes)))
        }),
        wire: None,
        state_version: 1,
    }
}

pub fn subagent_model_selection_policy(
    handle: &ProjectionHandle<ModelSelectionPolicyState>,
    session: &Session,
) -> Result<Option<Vec<AllowedModelRoute>>, BoxError> {
    Ok(handle.state_of(session)?.as_ref().clone())
}
