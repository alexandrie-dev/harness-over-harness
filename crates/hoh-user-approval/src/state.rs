//! Private host-only approvalState projection from src/index.ts:90–119.
use crate::{ApprovalError, ApprovalPolicy};
use hoh_error::BoxError;
use hoh_session_projection::ProjectionDefinition;
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalState {
    pub policy: Option<ApprovalPolicy>,
    pub turn_open: bool,
}

fn received(value: Option<&Value>) -> &'static str {
    match value {
        None => "undefined",
        Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}
fn invalid_type(expected: &str, value: Option<&Value>, path: Value) -> Value {
    json!({"expected": expected,"code":"invalid_type","path":path,"message":format!("Invalid input: expected {expected}, received {}",received(value))})
}
pub(crate) fn parse(value: Option<&Value>) -> Result<Arc<ApprovalState>, BoxError> {
    let mut issues = vec![];
    let Some(object) = value.and_then(Value::as_object) else {
        issues.push(invalid_type("object", value, json!([])));
        return Err(
            ApprovalError::InvalidState(hoh_util_values::js_json_stringify_pretty(
                &issues.into(),
                2,
            ))
            .into(),
        );
    };
    let policy = match object.get("policy") {
        Some(Value::Null) => None,
        Some(Value::String(policy)) if policy == "ask" || policy == "never" => {
            Some(if policy == "ask" {
                ApprovalPolicy::Ask
            } else {
                ApprovalPolicy::Never
            })
        }
        _ => {
            issues.push(json!({"code":"invalid_value","values":["ask","never"],"path":["policy"],"message":"Invalid option: expected one of \"ask\"|\"never\""}));
            None
        }
    };
    let turn_open = match object.get("turnOpen").and_then(Value::as_bool) {
        Some(value) => value,
        None => {
            issues.push(invalid_type(
                "boolean",
                object.get("turnOpen"),
                json!(["turnOpen"]),
            ));
            false
        }
    };
    if !issues.is_empty() {
        return Err(
            ApprovalError::InvalidState(hoh_util_values::js_json_stringify_pretty(
                &issues.into(),
                2,
            ))
            .into(),
        );
    }
    // z.object strips unknown fields, requires policy (nullable) and turnOpen.
    Ok(Arc::new(ApprovalState { policy, turn_open }))
}
pub(crate) fn definition() -> ProjectionDefinition<ApprovalState> {
    ProjectionDefinition {
        key: "approvalState".into(),
        state_version: 1,
        state_schema: Arc::new(parse),
        init: Arc::new(|_, _| Ok(Arc::new(ApprovalState::default()))),
        apply: Arc::new(|state, event| {
            let mut next = (**state).clone();
            match event.event_type() {
                "approval/policy" => {
                    next.policy = Some(ApprovalPolicy::try_from(
                        event
                            .data()
                            .get("policy")
                            .and_then(Value::as_str)
                            .unwrap_or(""),
                    )?)
                }
                "turn/start" => next.turn_open = true,
                "turn/end" => next.turn_open = false,
                _ => return Ok(state.clone()),
            }
            if next.policy == state.policy && next.turn_open == state.turn_open {
                Ok(state.clone())
            } else {
                Ok(Arc::new(next))
            }
        }),
        wire: None,
    }
}
