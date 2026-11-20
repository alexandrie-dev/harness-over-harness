use crate::{
    model_selection::{AllowedModelRoute, ModelSelectionPolicy},
    model_selection_state::{ModelSelectionPolicyState, subagent_model_selection_policy},
};
use hoh_error::BoxError;
use hoh_kernel::{Context, Service};
use hoh_llm::{ContentBlock, Llm, LlmModelInfo, LlmRuntime};
use hoh_session_projection::ProjectionHandle;
use hoh_subagent::{SubagentError, SubagentErrorCode, parent_agent_options_for_delegation};
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, ToolArgsError, ToolContextUnavailableError, ToolExecution,
    ToolRegistrationOptions, Tools, define_tool,
};
use serde_json::{Value, json};
use std::sync::Arc;

fn model_line(provider: &str, model: &LlmModelInfo) -> String {
    format!(
        "{provider}/{} — {}{}",
        model.id,
        model.name,
        model
            .description
            .as_ref()
            .map(|description| format!(": {description}"))
            .unwrap_or_default()
    )
}

fn registered_provider(
    llm: &LlmRuntime,
    policy: &ModelSelectionPolicy,
    provider_id: &str,
) -> Result<hoh_llm::LlmProviderInfo, BoxError> {
    let providers = llm.list_providers();
    if let Some(provider) = providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .cloned()
    {
        return Ok(provider);
    }
    let available = providers
        .iter()
        .filter(|provider| {
            policy
                .routes
                .iter()
                .any(|route| route.provider == provider.id)
        })
        .map(|provider| provider.id.as_str())
        .collect::<Vec<_>>();
    Err(Box::new(SubagentError::error(
        format!(
            "LLM provider \"{provider_id}\" is not registered; available providers: {}",
            if available.is_empty() {
                "(none)".into()
            } else {
                available.join(", ")
            }
        ),
        SubagentErrorCode::LlmProviderNotFound,
    )))
}

pub async fn list_subagent_models(
    llm: Option<&LlmRuntime>,
    handle: &ProjectionHandle<ModelSelectionPolicyState>,
    request: &Value,
    exec: &ToolExecution,
) -> Result<String, BoxError> {
    let agent = exec.agent.as_ref().ok_or_else(|| -> BoxError {
        Box::new(ToolContextUnavailableError::new(
            "list_subagent_models requires a calling agent",
        ))
    })?;
    let routes = subagent_model_selection_policy(handle, agent.session())?;
    let selection_enabled = routes.is_some();
    let policy = match routes {
        Some(routes) => ModelSelectionPolicy { routes },
        None => {
            let options = parent_agent_options_for_delegation(agent.as_ref());
            let provider = options.provider().ok_or_else(|| -> BoxError {
                Box::new(ToolContextUnavailableError::new(
                    "cannot discover child LLM routes without a parent provider and model",
                ))
            })?;
            let model = options.model().ok_or_else(|| -> BoxError {
                Box::new(ToolContextUnavailableError::new(
                    "cannot discover child LLM routes without a parent provider and model",
                ))
            })?;
            ModelSelectionPolicy {
                routes: vec![AllowedModelRoute { provider, model }],
            }
        }
    };
    let llm = llm.ok_or_else(|| -> BoxError {
        Box::new(ToolContextUnavailableError::new(
            "cannot discover child LLM routes because the `llm` service is unavailable",
        ))
    })?;
    let provider_id = request.get("provider").and_then(Value::as_str);
    let model_id = request.get("model").and_then(Value::as_str);
    if model_id.is_some() && provider_id.is_none() {
        return Err(Box::new(ToolArgsError::new(vec![
            "`model` requires `provider`".into(),
        ])));
    }
    if provider_id.is_none() {
        let providers = llm
            .list_providers()
            .into_iter()
            .filter(|provider| {
                policy
                    .routes
                    .iter()
                    .any(|route| route.provider == provider.id)
            })
            .map(|provider| format!("{} — {}", provider.id, provider.name))
            .collect::<Vec<_>>();
        return Ok(if providers.is_empty() {
            "(no LLM providers)".into()
        } else {
            providers.join("\n")
        });
    }
    let provider_id = provider_id.unwrap_or_default();
    if provider_id.is_empty() {
        return Err(Box::new(ToolArgsError::new(vec![
            "`provider` must be non-empty".into(),
        ])));
    }
    let allowed = policy
        .routes
        .iter()
        .filter(|route| route.provider == provider_id)
        .collect::<Vec<_>>();
    let Some(first) = allowed.first() else {
        return Err(Box::new(SubagentError::error(
            format!("LLM provider \"{provider_id}\" is not allowed for this Session"),
            SubagentErrorCode::ModelRouteNotAllowed,
        )));
    };
    let provider = registered_provider(llm, &policy, provider_id)?;
    if model_id.is_none() {
        let models = if selection_enabled {
            llm.list_models(&provider.id)
                .await?
                .into_iter()
                .filter(|model| allowed.iter().any(|route| route.model == model.id))
                .collect::<Vec<_>>()
        } else {
            vec![
                llm.resolve_model_info(&provider.id, &first.model, Some(&exec.signal()))
                    .await?
                    .model,
            ]
        };
        exec.signal().throw_if_aborted()?;
        return Ok(if models.is_empty() {
            format!("(no advertised models for {})", provider.id)
        } else {
            models
                .iter()
                .map(|model| model_line(&provider.id, model))
                .collect::<Vec<_>>()
                .join("\n")
        });
    }
    let model_id = model_id.unwrap_or_default();
    if model_id.is_empty() {
        return Err(Box::new(ToolArgsError::new(vec![
            "`model` must be non-empty".into(),
        ])));
    }
    if !allowed.iter().any(|route| route.model == model_id) {
        return Err(Box::new(SubagentError::error(
            format!(
                "child LLM route \"{}/{model_id}\" is not allowed for this Session",
                provider.id
            ),
            SubagentErrorCode::ModelRouteNotAllowed,
        )));
    }
    let model = llm
        .resolve_model_info(&provider.id, model_id, Some(&exec.signal()))
        .await?;
    let efforts = model
        .reasoning
        .as_ref()
        .map(|reasoning| {
            reasoning
                .efforts
                .iter()
                .map(|effort| {
                    format!(
                        "{}{} — {}{}",
                        effort.id,
                        if reasoning.default_effort.as_ref() == Some(&effort.id) {
                            " (default)"
                        } else {
                            ""
                        },
                        effort.name,
                        effort
                            .description
                            .as_ref()
                            .map(|description| format!(": {description}"))
                            .unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|efforts| !efforts.is_empty())
        .unwrap_or_else(|| "(no advertised reasoning efforts)".into());
    Ok(format!(
        "{}\nReasoning efforts:\n{efforts}",
        model_line(&provider.id, &model.model)
    ))
}

pub fn register_list_subagent_models(
    ctx: &Context,
    tools: &Service<Tools>,
    handle: ProjectionHandle<ModelSelectionPolicyState>,
) -> Result<(), BoxError> {
    let context = ctx.clone();
    let mut options = DefineToolOptions::new(
        "list_subagent_models",
        "Inspect the routes available for explicit subagent model selection. No arguments: authorized providers. `provider`: that provider's authorized models. `provider` + `model`: that model and its reasoning efforts. Query when choosing an explicit child route or reasoning effort. Omit optional fields rather than sending empty strings.",
        json!({
            "provider": { "type": "string", "description": "Registered LLM provider id." },
            "model": { "type": "string", "description": "Exact model id; requires provider." }
        }),
        DefineToolOutput {
            schema: json!({ "type": "string" }),
            render: Arc::new(|_, result| {
                vec![ContentBlock::text(result.as_str().unwrap_or_default())]
            }),
            presentation_meta: None,
        },
        move |args, exec| {
            let llm = context.get::<Llm>();
            let handle = handle.clone();
            Box::pin(async move {
                Ok(Value::String(
                    list_subagent_models(llm.as_deref(), &handle, &args, &exec).await?,
                ))
            })
        },
    );
    options.is_concurrency_safe = Some(Arc::new(|_| true));
    tools.register(
        ctx,
        define_tool(options)?,
        ToolRegistrationOptions::default(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoh_error::HarnessError;
    use hoh_llm::LlmModelInfo;

    fn route(provider: &str, model: &str) -> AllowedModelRoute {
        AllowedModelRoute {
            provider: provider.into(),
            model: model.into(),
        }
    }

    #[test]
    fn model_lines_preserve_optional_descriptions_byte_for_byte() {
        let mut model = LlmModelInfo {
            provider: "p".into(),
            id: "m".into(),
            name: "Model".into(),
            description: None,
            input_modalities: None,
            badge: None,
        };
        assert_eq!(model_line("p", &model), "p/m — Model");
        model.description = Some("Description".into());
        assert_eq!(model_line("p", &model), "p/m — Model: Description");
    }

    #[test]
    fn missing_provider_reports_only_registered_authorized_alternatives() {
        let llm = LlmRuntime::new();
        let policy = ModelSelectionPolicy {
            routes: vec![route("allowed", "model")],
        };
        let error = registered_provider(&llm, &policy, "missing").unwrap_err();
        let error = HarnessError::of(error.as_ref()).expect("SubagentError");
        assert_eq!(error.code(), "LLM_PROVIDER_NOT_FOUND");
        assert_eq!(
            error.message(),
            "LLM provider \"missing\" is not registered; available providers: (none)"
        );
    }
}
