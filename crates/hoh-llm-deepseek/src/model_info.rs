//! Protocol-independent model capabilities and reasoning choices.

use crate::types::{DeepSeekCatalogModel, DeepSeekConnectionOptions};
use hoh_llm::{
    LlmModelBadge, LlmModelContext, LlmModelInfo, LlmModelReasoningInfo, LlmReasoningEffortInfo,
    LlmResolvedModelInfo, ReasoningEffortId,
};

fn effort(id: &str, name: &str, description: &str) -> LlmReasoningEffortInfo {
    LlmReasoningEffortInfo {
        id: id.into(),
        name: name.into(),
        description: Some(description.into()),
    }
}

pub fn catalog_model_info(
    provider: &str,
    model: &DeepSeekCatalogModel,
    badge: Option<LlmModelBadge>,
) -> LlmModelInfo {
    LlmModelInfo {
        provider: provider.into(),
        id: model.id.clone(),
        name: model.name.clone().unwrap_or_else(|| model.id.clone()),
        description: model.description.clone(),
        input_modalities: Some(
            model
                .input_modalities
                .clone()
                .unwrap_or_else(|| vec!["text".into()]),
        ),
        badge,
    }
}

pub fn model_info(
    connection: &DeepSeekConnectionOptions,
    provider: &str,
    model: &str,
) -> LlmResolvedModelInfo {
    let configured = connection.models.iter().find(|entry| entry.id == model);
    let model_info = configured.map_or_else(
        || LlmModelInfo {
            provider: provider.into(),
            id: model.into(),
            name: model.into(),
            input_modalities: Some(vec!["text".into()]),
            ..Default::default()
        },
        |entry| catalog_model_info(provider, entry, None),
    );
    let (efforts, default_effort): (Vec<_>, ReasoningEffortId) =
        if connection.defaults.thinking.as_deref() == Some("disabled") {
            (
                vec![effort(
                    "off",
                    "Off",
                    "Use for simple tasks that do not need reasoning.",
                )],
                "off".into(),
            )
        } else {
            (
                vec![
                    effort(
                        "off",
                        "Off",
                        "Use for simple tasks that do not need reasoning.",
                    ),
                    effort(
                        "low",
                        "Low",
                        "Prefer for routine or latency-sensitive tasks.",
                    ),
                    effort("high", "High", "The default balance for most tasks."),
                    effort("max", "Max", "Reserve for the hardest quality-first tasks."),
                ],
                match connection.defaults.reasoning_effort.as_deref() {
                    Some("off") => "off",
                    Some("low") => "low",
                    Some("max") => "max",
                    _ => "high",
                }
                .into(),
            )
        };
    LlmResolvedModelInfo {
        model: model_info,
        context: Some(LlmModelContext {
            context_window: configured
                .and_then(|entry| entry.context_window)
                .unwrap_or(connection.default_context_window),
        }),
        default_max_tokens: Some(
            configured
                .and_then(|entry| entry.max_tokens)
                .unwrap_or(connection.max_tokens),
        ),
        system_prompt_update: configured.and_then(|entry| entry.system_prompt_update.clone()),
        tool_update: configured.and_then(|entry| entry.tool_update.clone()),
        reasoning: Some(LlmModelReasoningInfo {
            efforts,
            default_effort: Some(default_effort),
        }),
    }
}
