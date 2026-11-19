//! Temporary boundary for the not-yet-ported `hoh-llm-deepseek` crate.
// SHIM(hoh-llm-deepseek): credential resolution, catalog metadata and adapter transport boundary;
// replace when hoh-llm-deepseek provides them.

use crate::config::{DeepSeekCatalogModel, ReasoningEffort, ResolvedDeepSeekOptions, Thinking};
use hoh_error::BoxError;
use hoh_kernel::Context;
use hoh_llm::{
    BuiltinLlmFailureCode, LlmError, LlmModelBadge, LlmModelContext, LlmModelInfo,
    LlmModelReasoningInfo, LlmReasoningEffortInfo, LlmResolvedModelInfo, ReasoningEffortId,
};

pub(crate) async fn resolve_deepseek_api_key(
    ctx: &Context,
    reference: &hoh_credentials::CredentialRef,
) -> Result<Option<String>, BoxError> {
    let raw = if let Some(credentials) = ctx.get::<hoh_credentials::Credentials>() {
        credentials
            .resolve(reference)
            .await?
            .map(|credential| credential.value)
    } else {
        hoh_launch_environment::launch_environment_of(ctx)
            .get(reference.as_str())
            .map(|entry| entry.value)
            .filter(|value| !value.is_empty())
    };
    raw.map(|value| {
        hoh_llm::assert_usable_api_key(&value, "llm-deepseek", reference.as_str())
            .map_err(|error| Box::new(error) as BoxError)
    })
    .transpose()
}

pub(crate) fn catalog_model_info(
    provider: &str,
    model: &DeepSeekCatalogModel,
    badge: Option<&LlmModelBadge>,
) -> LlmModelInfo {
    LlmModelInfo {
        provider: provider.into(),
        id: model.id.clone(),
        name: model.name.clone().unwrap_or_else(|| model.id.clone()),
        description: model.description.clone(),
        input_modalities: model
            .input_modalities
            .clone()
            .or_else(|| Some(vec!["text".into()])),
        badge: badge.cloned(),
    }
}

fn effort(id: &str, name: &str, description: &str) -> LlmReasoningEffortInfo {
    LlmReasoningEffortInfo {
        id: ReasoningEffortId::from(id),
        name: name.into(),
        description: Some(description.into()),
    }
}

pub(crate) fn model_info(
    connection: &ResolvedDeepSeekOptions,
    provider: &str,
    model_id: &str,
) -> LlmResolvedModelInfo {
    let configured = connection.models.iter().find(|model| model.id == model_id);
    let model = configured.map_or_else(
        || LlmModelInfo {
            provider: provider.into(),
            id: model_id.into(),
            name: model_id.into(),
            input_modalities: Some(vec!["text".into()]),
            ..Default::default()
        },
        |model| catalog_model_info(provider, model, None),
    );
    let off = effort(
        "off",
        "Off",
        "Use for simple tasks that do not need reasoning.",
    );
    let efforts = if connection.defaults.thinking == Some(Thinking::Disabled) {
        vec![off.clone()]
    } else {
        vec![
            off.clone(),
            effort(
                "low",
                "Low",
                "Prefer for routine or latency-sensitive tasks.",
            ),
            effort("high", "High", "The default balance for most tasks."),
            effort("max", "Max", "Reserve for the hardest quality-first tasks."),
        ]
    };
    let default_effort = if connection.defaults.thinking == Some(Thinking::Disabled) {
        "off"
    } else {
        match connection.defaults.reasoning_effort {
            Some(ReasoningEffort::Off) => "off",
            Some(ReasoningEffort::Low) => "low",
            Some(ReasoningEffort::Max) => "max",
            Some(ReasoningEffort::High) | None => "high",
        }
    };
    LlmResolvedModelInfo {
        model,
        context: Some(LlmModelContext {
            context_window: configured
                .and_then(|model| model.context_window)
                .unwrap_or(connection.default_context_window),
        }),
        default_max_tokens: Some(
            configured
                .and_then(|model| model.max_tokens)
                .unwrap_or(connection.max_tokens),
        ),
        system_prompt_update: configured.and_then(|model| model.system_prompt_update.clone()),
        tool_update: configured.and_then(|model| model.tool_update.clone()),
        reasoning: Some(LlmModelReasoningInfo {
            efforts,
            default_effort: Some(ReasoningEffortId::from(default_effort)),
        }),
    }
}

pub(crate) fn unavailable_transport() -> BoxError {
    Box::new(
        LlmError::new(
            "llm-deepseek-api-key: Messages transport awaits hoh-llm-deepseek integration",
            BuiltinLlmFailureCode::Transport,
        )
        .expect("valid LLM error"),
    )
}
