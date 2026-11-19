use crate::{ReplayProviderConfig, Result, replay::Replay};
use hoh_llm::*;
use std::sync::Arc;
pub(crate) struct ReplayAdapter {
    pub providers: Vec<ReplayProviderConfig>,
    pub replay: Arc<Replay>,
}
impl ReplayAdapter {
    fn provider(&self, id: &str) -> Option<&ReplayProviderConfig> {
        self.providers.iter().rev().find(|p| p.id == id)
    }
    fn model(&self, provider: &str, id: &str) -> Option<&crate::ReplayModelConfig> {
        self.provider(provider)?
            .models
            .as_ref()?
            .iter()
            .find(|m| m.id == id)
    }
}
impl LlmAdapter for ReplayAdapter {
    fn provider_info(&self, provider: &str) -> Result<LlmProviderInfo> {
        Ok(LlmProviderInfo {
            id: provider.into(),
            name: self
                .provider(provider)
                .and_then(|p| p.name.clone())
                .unwrap_or_else(|| provider.into()),
        })
    }
    fn provider_retry_policy(&self, provider: &str) -> Result<Option<ResolvedRetryPolicy>> {
        self.provider(provider)
            .and_then(|p| p.retry_policy.as_ref())
            .map(|value| {
                resolve_retry_policy_value(
                    Some(value),
                    &format!("llm-replay: provider \"{provider}\" retryPolicy"),
                )
                .map_err(Into::into)
            })
            .transpose()
    }
    fn image_request_pricing(
        &self,
        provider: &str,
        model: &str,
    ) -> Option<Arc<dyn LlmImageRequestPricing>> {
        Some(Arc::new(Pricing(
            self.model(provider, model)?.image_request_tokens?,
        )))
    }
    fn list_models<'a>(&'a self, provider: &'a str) -> LlmFuture<'a, Vec<LlmModelInfo>> {
        Box::pin(async move {
            Ok(self
                .provider(provider)
                .and_then(|p| p.models.as_ref())
                .into_iter()
                .flatten()
                .map(|m| LlmModelInfo {
                    provider: provider.into(),
                    id: m.id.clone(),
                    name: m.name.clone().unwrap_or_else(|| m.id.clone()),
                    description: m.description.clone(),
                    input_modalities: m.input_modalities.clone(),
                    badge: m.badge.clone(),
                })
                .collect())
        })
    }
    fn resolve_model<'a>(
        &'a self,
        provider: &'a str,
        model: &'a str,
        _signal: Option<&'a hoh_timeout::AbortSignal>,
    ) -> LlmFuture<'a, LlmResolvedModelInfo> {
        Box::pin(async move {
            let configured = self.model(provider, model);
            Ok(LlmResolvedModelInfo {
                model: LlmModelInfo {
                    provider: provider.into(),
                    id: model.into(),
                    name: configured
                        .and_then(|m| m.name.clone())
                        .unwrap_or_else(|| model.into()),
                    description: configured.and_then(|m| m.description.clone()),
                    input_modalities: configured.and_then(|m| m.input_modalities.clone()),
                    badge: None,
                },
                context: configured
                    .and_then(|m| m.context_window)
                    .map(|context_window| LlmModelContext { context_window }),
                default_max_tokens: configured.and_then(|m| m.default_max_tokens),
                system_prompt_update: configured.and_then(|m| m.system_prompt_update.clone()),
                tool_update: configured.and_then(|m| m.tool_update.clone()),
                reasoning: configured.and_then(|m| {
                    m.reasoning_efforts
                        .as_ref()
                        .map(|efforts| LlmModelReasoningInfo {
                            efforts: efforts
                                .iter()
                                .map(|id| LlmReasoningEffortInfo {
                                    id: id.as_str().into(),
                                    name: id.clone(),
                                    description: None,
                                })
                                .collect(),
                            default_effort: m.default_reasoning_effort.as_deref().map(Into::into),
                        })
                }),
            })
        })
    }
    fn stream(&self, options: GenerateOptions) -> Result<LlmStream> {
        Ok(self.replay.stream(options))
    }
}
struct Pricing(u64);
impl LlmImageRequestPricing for Pricing {
    fn price_images(&self, images: &[ImageBlock]) -> Vec<LlmImageRequestPrice> {
        images
            .iter()
            .map(|image| {
                let reference = &image.attachment;
                if image.offloaded == Some(true) {
                    LlmImageRequestPrice {
                        visual_tokens: 0,
                        text: offloaded_image_text(reference, None),
                    }
                } else {
                    LlmImageRequestPrice {
                        visual_tokens: self.0,
                        text: request_image_handle_text(
                            reference,
                            reference.width,
                            reference.height,
                            None,
                        ),
                    }
                }
            })
            .collect()
    }
}
