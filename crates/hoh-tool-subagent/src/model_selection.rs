use hoh_agent::{AgentOptions, Live};
use hoh_error::BoxError;
use hoh_llm::{LlmCallConfig, LlmCallControls, LlmRuntime, ReasoningEffortId};
use hoh_subagent::{SubagentError, SubagentErrorCode};
use hoh_timeout::AbortSignal;
use hoh_tools::{ToolArgsError, ToolContextUnavailableError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "AllowedModelRouteWire")]
pub struct AllowedModelRoute {
    pub provider: String,
    pub model: String,
}

#[derive(Deserialize, JsonSchema)]
#[schemars(rename = "AllowedModelRoute")]
struct AllowedModelRouteWire {
    provider: String,
    model: String,
}

/// `AllowedModelRouteSchema` of the source: `z.string().min(1).required()` for both ids, checked
/// when the configuration loads.
impl TryFrom<AllowedModelRouteWire> for AllowedModelRoute {
    type Error = String;
    fn try_from(wire: AllowedModelRouteWire) -> Result<Self, String> {
        if wire.provider.is_empty() || wire.model.is_empty() {
            return Err(
                "subagent model selection requires non-empty provider and model ids".into(),
            );
        }
        Ok(Self {
            provider: wire.provider,
            model: wire.model,
        })
    }
}

/// `assertAllowedModelRoutes` on an untrusted value, with the source's two messages.
pub fn routes_from_value(
    value: Option<&serde_json::Value>,
) -> Result<Vec<AllowedModelRoute>, BoxError> {
    let Some(serde_json::Value::Array(items)) = value else {
        return Err("subagent model selection requires an array of routes".into());
    };
    let mut routes = Vec::with_capacity(items.len());
    for item in items {
        let field = |name| match item.get(name) {
            Some(serde_json::Value::String(text)) if !text.is_empty() => Some(text.clone()),
            _ => None,
        };
        let (Some(provider), Some(model), true) =
            (field("provider"), field("model"), item.is_object())
        else {
            return Err(
                "subagent model selection requires non-empty provider and model ids".into(),
            );
        };
        routes.push(AllowedModelRoute { provider, model });
    }
    assert_allowed_model_routes(&routes)?;
    Ok(routes)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelSelectionPolicy {
    pub routes: Vec<AllowedModelRoute>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DelegationModelRequest {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}

pub fn model_route_key(route: &AllowedModelRoute) -> String {
    format!("{}\0{}", route.provider, route.model)
}

pub fn assert_allowed_model_routes(routes: &[AllowedModelRoute]) -> Result<(), BoxError> {
    let mut seen = HashSet::new();
    for route in routes {
        if route.provider.is_empty() || route.model.is_empty() {
            return Err(
                "subagent model selection requires non-empty provider and model ids".into(),
            );
        }
        if !seen.insert(model_route_key(route)) {
            return Err(format!(
                "subagent model selection repeats route \"{}/{}\"",
                route.provider, route.model
            )
            .into());
        }
    }
    Ok(())
}

pub fn has_delegation_model_request(request: &DelegationModelRequest) -> bool {
    request.provider.is_some() || request.model.is_some() || request.reasoning_effort.is_some()
}

fn validate_non_empty(value: Option<&str>, field: &str) -> Result<(), BoxError> {
    if value == Some("") {
        return Err(Box::new(ToolArgsError::new(vec![format!(
            "child LLM `{field}` must be non-empty"
        )])));
    }
    Ok(())
}

pub fn assert_allowed_model_selection(
    policy: Option<&ModelSelectionPolicy>,
    parent: &AgentOptions,
    route: &AllowedModelRoute,
) -> Result<(), BoxError> {
    let allowed = match policy {
        Some(policy) => policy.routes.iter().any(|candidate| candidate == route),
        None => {
            parent.provider().as_deref() == Some(route.provider.as_str())
                && parent.model().as_deref() == Some(route.model.as_str())
        }
    };
    if allowed {
        return Ok(());
    }
    let suffix = if policy.is_none() {
        "; without a recorded model-selection policy, explicit selection is limited to the parent route"
    } else {
        ""
    };
    Err(Box::new(SubagentError::error(
        format!(
            "child LLM route \"{}/{}\" is not allowed for this Session{suffix}",
            route.provider, route.model
        ),
        SubagentErrorCode::ModelRouteNotAllowed,
    )))
}

pub fn requested_agent_options(
    configured: Option<&AgentOptions>,
    request: &DelegationModelRequest,
    enabled: bool,
    policy: Option<&ModelSelectionPolicy>,
    parent: &AgentOptions,
) -> Result<Option<AgentOptions>, BoxError> {
    if !has_delegation_model_request(request) {
        return Ok(configured.cloned());
    }
    if !enabled {
        return Err(Box::new(SubagentError::error(
            "child model selection is disabled for this tool instance",
            SubagentErrorCode::ModelSelectionDisabled,
        )));
    }
    validate_non_empty(request.provider.as_deref(), "provider")?;
    validate_non_empty(request.model.as_deref(), "model")?;
    validate_non_empty(request.reasoning_effort.as_deref(), "reasoning_effort")?;
    let (Some(provider), Some(model)) = (&request.provider, &request.model) else {
        let violation = if request.reasoning_effort.is_none() {
            "child LLM `provider` and `model` must be supplied together"
        } else {
            "child LLM `reasoning_effort` requires both `provider` and `model`; supply both or omit all three fields"
        };
        return Err(Box::new(ToolArgsError::new(vec![violation.into()])));
    };
    assert_allowed_model_selection(
        policy,
        parent,
        &AllowedModelRoute {
            provider: provider.clone(),
            model: model.clone(),
        },
    )?;
    let configured = configured.cloned().unwrap_or_default();
    Ok(Some(AgentOptions {
        provider: Some(Live::Value(provider.clone())),
        model: Some(Live::Value(model.clone())),
        reasoning_effort: request
            .reasoning_effort
            .as_deref()
            .map(ReasoningEffortId::from)
            .map(Live::Value),
        max_tokens: configured.max_tokens,
    }))
}

pub fn has_configured_llm_selection(options: Option<&AgentOptions>) -> bool {
    options.is_some_and(|options| {
        options.provider.is_some() || options.model.is_some() || options.reasoning_effort.is_some()
    })
}

pub async fn preflight_child_llm_route(
    llm: &LlmRuntime,
    parent: &AgentOptions,
    requested: Option<&AgentOptions>,
    signal: &AbortSignal,
    inherit_parent_reasoning_effort: bool,
) -> Result<(), BoxError> {
    let provider = requested
        .and_then(AgentOptions::provider)
        .or_else(|| parent.provider())
        .ok_or_else(|| {
            Box::new(ToolContextUnavailableError::new(
                "cannot select child LLM values without an effective provider and model",
            )) as BoxError
        })?;
    let model = requested
        .and_then(AgentOptions::model)
        .or_else(|| parent.model())
        .ok_or_else(|| {
            Box::new(ToolContextUnavailableError::new(
                "cannot select child LLM values without an effective provider and model",
            )) as BoxError
        })?;
    let route_changed = parent.provider().as_deref() != Some(provider.as_str())
        || parent.model().as_deref() != Some(model.as_str());
    let reasoning_effort = requested
        .and_then(AgentOptions::reasoning_effort)
        .or_else(|| {
            (inherit_parent_reasoning_effort && !route_changed)
                .then(|| parent.reasoning_effort())
                .flatten()
        });
    llm.resolve_call_config(
        LlmCallConfig {
            provider,
            model,
            controls: LlmCallControls {
                reasoning_effort,
                ..LlmCallControls::default()
            },
        },
        Some(signal),
    )
    .await?;
    Ok(())
}
