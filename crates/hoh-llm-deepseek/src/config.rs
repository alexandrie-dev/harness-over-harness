//! Plugin configuration and complete request-local resolution.

use crate::defaults::*;
use crate::models::default_models;
use crate::request_pricing::{
    DEFAULT_MAX_IMAGES_PER_REQUEST, DEFAULT_MAX_REQUEST_FILES_BYTES,
    DEFAULT_REQUEST_IMAGE_MAX_BYTES,
};
use crate::types::{
    DeepSeekCatalogModel, DeepSeekConnectionOptions, DeepSeekFilePolicy, ImagePixelBudget,
    RequestDefaults,
};
use hoh_launch_environment::LaunchEnvironmentSnapshot;
use hoh_llm::resolve_retry_policy;
use hoh_timeout::MAX_TIMER_DELAY_MS;
use std::collections::HashSet;
use url::Url;

pub use crate::types::{Options, ResolvedDeepSeekOptions};

pub const PUBLIC_BASE_URL: &str = "https://api.deepseek.com/anthropic";
const BASE_URL_ENV: &str = "DEEPSEEK_BASE_URL";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ConfigError(pub String);

fn fail(message: impl Into<String>) -> ConfigError {
    ConfigError(message.into())
}

fn resolve_models(
    models: Option<&[DeepSeekCatalogModel]>,
) -> Result<Vec<DeepSeekCatalogModel>, ConfigError> {
    let source = models.map_or_else(default_models, <[DeepSeekCatalogModel]>::to_vec);
    let mut seen = HashSet::new();
    source
        .into_iter()
        .map(|mut model| {
            if model.image_detail {
                return Err(fail(
                    "llm-deepseek: catalog model imageDetail is no longer supported; use imagePixelBudget",
                ));
            }
            if model.id.is_empty() {
                return Err(fail("llm-deepseek: catalog model ids must be non-empty"));
            }
            if model.name.as_ref().is_some_and(String::is_empty) {
                return Err(fail(format!(
                    "llm-deepseek: catalog model \"{}\" has an empty name",
                    model.id
                )));
            }
            if model.context_window == Some(0) {
                return Err(fail(format!(
                    "llm-deepseek: catalog model \"{}\" contextWindow must be a positive integer",
                    model.id
                )));
            }
            if model.max_tokens == Some(0) {
                return Err(fail(format!(
                    "llm-deepseek: catalog model \"{}\" maxTokens must be a positive integer",
                    model.id
                )));
            }
            let modalities = model
                .input_modalities
                .take()
                .unwrap_or_else(|| vec!["text".into()]);
            if modalities.is_empty() {
                return Err(fail(format!(
                    "llm-deepseek: catalog model \"{}\" inputModalities must not be empty",
                    model.id
                )));
            }
            if modalities
                .iter()
                .any(|item| item != "text" && item != "image")
            {
                return Err(fail(format!(
                    "llm-deepseek: catalog model \"{}\" inputModalities must contain only \"text\" and \"image\"",
                    model.id
                )));
            }
            let unique: HashSet<_> = modalities.iter().collect();
            if unique.len() != modalities.len() {
                return Err(fail(format!(
                    "llm-deepseek: catalog model \"{}\" inputModalities must not contain duplicates",
                    model.id
                )));
            }
            let has_image = modalities.iter().any(|item| item == "image");
            if !has_image
                && (model.image_pixel_budget.is_some() || model.image_max_bytes.is_some())
            {
                return Err(fail(format!(
                    "llm-deepseek: text-only catalog model \"{}\" cannot declare image request limits",
                    model.id
                )));
            }
            if model.image_pixel_budget.as_ref().is_some_and(|budget| {
                matches!(budget, ImagePixelBudget::Pixels(value) if *value == 0 || *value > 9_007_199_254_740_991)
            }) {
                return Err(fail(format!(
                    "llm-deepseek: catalog model \"{}\" imagePixelBudget must be \"low\" or a positive safe integer",
                    model.id
                )));
            }
            if model
                .image_max_bytes
                .is_some_and(|value| value == 0 || value > 9_007_199_254_740_991)
            {
                return Err(fail(format!(
                    "llm-deepseek: catalog model \"{}\" imageMaxBytes must be a positive safe integer",
                    model.id
                )));
            }
            if model
                .system_prompt_update
                .as_deref()
                .is_some_and(|value| value != "in-history")
            {
                return Err(fail(format!(
                    "llm-deepseek: catalog model \"{}\" systemPromptUpdate must be \"in-history\" when present",
                    model.id
                )));
            }
            if model.tool_update.as_deref().is_some_and(|value| {
                value != "in-history" && value != "addition-only"
            }) {
                return Err(fail(format!(
                    "llm-deepseek: catalog model \"{}\" toolUpdate must be \"in-history\" or \"addition-only\" when present",
                    model.id
                )));
            }
            if !seen.insert(model.id.clone()) {
                return Err(fail(format!(
                    "llm-deepseek: duplicate catalog model \"{}\"",
                    model.id
                )));
            }
            model.input_modalities = Some(modalities);
            if has_image && model.image_max_bytes.is_none() {
                model.image_max_bytes = Some(DEFAULT_REQUEST_IMAGE_MAX_BYTES);
            }
            Ok(model)
        })
        .collect()
}

pub fn is_built_in_catalog(models: &[DeepSeekCatalogModel]) -> bool {
    resolve_models(None).is_ok_and(|built_in| built_in == models)
}

pub fn resolve_adapter_options(config: &Options) -> Result<DeepSeekConnectionOptions, ConfigError> {
    resolve_adapter_options_with_environment(config, None)
}

pub fn resolve_adapter_options_with_environment(
    config: &Options,
    environment: Option<&LaunchEnvironmentSnapshot>,
) -> Result<DeepSeekConnectionOptions, ConfigError> {
    if config.extra.contains_key("protocol") {
        return Err(fail(
            "llm-deepseek: protocol is not configurable; remove it and use a Messages-compatible baseURL",
        ));
    }
    if config.thinking.as_deref() == Some("disabled")
        && config
            .reasoning_effort
            .as_deref()
            .is_some_and(|value| value != "off")
    {
        return Err(fail(
            "llm-deepseek: only reasoningEffort \"off\" can be configured when thinking is disabled",
        ));
    }
    if config
        .thinking
        .as_deref()
        .is_some_and(|value| value != "enabled" && value != "disabled")
    {
        return Err(fail(
            "llm-deepseek: thinking must be \"enabled\" or \"disabled\"",
        ));
    }
    if config
        .reasoning_effort
        .as_deref()
        .is_some_and(|value| !matches!(value, "off" | "low" | "high" | "max"))
    {
        return Err(fail(
            "llm-deepseek: reasoningEffort must be \"off\", \"low\", \"high\", or \"max\"",
        ));
    }
    let positive = |value: Option<u64>, default, name: &str| {
        let value = value.unwrap_or(default);
        (value > 0 && value <= 9_007_199_254_740_991)
            .then_some(value)
            .ok_or_else(|| {
                fail(format!(
                    "llm-deepseek: {name} must be a positive safe integer"
                ))
            })
    };
    let max_tokens = positive(config.max_tokens, DEFAULT_MAX_TOKENS, "maxTokens")?;
    let default_context_window = config
        .default_context_window
        .unwrap_or(DEFAULT_CONTEXT_WINDOW);
    if default_context_window == 0 {
        return Err(fail(
            "llm-deepseek: defaultContextWindow must be a positive integer",
        ));
    }
    let timer = |value: Option<f64>, default: u64, name: &str| {
        let value = value.unwrap_or(default as f64);
        (value.is_finite() && value > 0.0 && value <= MAX_TIMER_DELAY_MS as f64)
            .then_some(value)
            .ok_or_else(|| {
                fail(format!(
                    "llm-deepseek: {name} must be a positive finite number no greater than {MAX_TIMER_DELAY_MS}"
                ))
            })
    };
    let stream_idle_timeout_ms = timer(
        config.stream_idle_timeout_ms,
        DEFAULT_STREAM_IDLE_TIMEOUT_MS,
        "streamIdleTimeoutMs",
    )?;
    let max_request_files_bytes = positive(
        config.max_request_files_bytes,
        DEFAULT_MAX_REQUEST_FILES_BYTES,
        "maxRequestFilesBytes",
    )?;
    let max_inline_request_image_bytes = positive(
        config.max_inline_request_image_bytes,
        DEFAULT_MAX_INLINE_REQUEST_IMAGE_BYTES,
        "maxInlineRequestImageBytes",
    )?;
    let max_images_per_request = positive(
        config.max_images_per_request,
        DEFAULT_MAX_IMAGES_PER_REQUEST,
        "maxImagesPerRequest",
    )?;
    let image_offload_byte_quantum = positive(
        config.image_offload_byte_quantum,
        DEFAULT_IMAGE_OFFLOAD_BYTE_QUANTUM,
        "imageOffloadByteQuantum",
    )?;
    if image_offload_byte_quantum > max_request_files_bytes {
        return Err(fail(
            "llm-deepseek: imageOffloadByteQuantum must not exceed maxRequestFilesBytes",
        ));
    }
    let inline_image_offload_byte_quantum = positive(
        config.inline_image_offload_byte_quantum,
        DEFAULT_INLINE_IMAGE_OFFLOAD_BYTE_QUANTUM,
        "inlineImageOffloadByteQuantum",
    )?;
    if inline_image_offload_byte_quantum > max_inline_request_image_bytes {
        return Err(fail(
            "llm-deepseek: inlineImageOffloadByteQuantum must not exceed maxInlineRequestImageBytes",
        ));
    }
    let image_offload_count_quantum = positive(
        config.image_offload_count_quantum,
        DEFAULT_IMAGE_OFFLOAD_COUNT_QUANTUM,
        "imageOffloadCountQuantum",
    )?;
    if image_offload_count_quantum > max_images_per_request {
        return Err(fail(
            "llm-deepseek: imageOffloadCountQuantum must not exceed maxImagesPerRequest",
        ));
    }
    let files_api_timeout_ms = timer(
        config.files_api_timeout_ms,
        DEFAULT_FILES_API_TIMEOUT_MS,
        "filesApiTimeoutMs",
    )?;
    let file_expires_after_seconds = config
        .file_expires_after_seconds
        .unwrap_or(DEFAULT_FILE_EXPIRY_SECONDS);
    if !(3_600..=2_592_000).contains(&file_expires_after_seconds) {
        return Err(fail(
            "llm-deepseek: fileExpiresAfterSeconds must be an integer from 3600 through 2592000",
        ));
    }
    let file_refresh_margin_seconds = config
        .file_refresh_margin_seconds
        .unwrap_or(DEFAULT_FILE_REFRESH_MARGIN_SECONDS);
    if file_refresh_margin_seconds >= file_expires_after_seconds {
        return Err(fail(
            "llm-deepseek: fileRefreshMarginSeconds must be a non-negative integer below fileExpiresAfterSeconds",
        ));
    }
    let file_quota_cleanup_batch = config
        .file_quota_cleanup_batch
        .unwrap_or(DEFAULT_FILE_QUOTA_CLEANUP_BATCH);
    if !(1..=1_000).contains(&file_quota_cleanup_batch) {
        return Err(fail(
            "llm-deepseek: fileQuotaCleanupBatch must be an integer from 1 through 1000",
        ));
    }
    let environment_base_url = environment
        .and_then(|environment| environment.get(BASE_URL_ENV))
        .map(|entry| entry.value);
    let base_url = config
        .base_url
        .as_deref()
        .or(environment_base_url.as_deref())
        .unwrap_or(PUBLIC_BASE_URL);
    let parsed = Url::parse(base_url).map_err(|_| {
        fail("llm-deepseek: Messages baseURL must be an HTTP(S) root without credentials, query, or fragment")
    })?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(fail(
            "llm-deepseek: Messages baseURL must be an HTTP(S) root without credentials, query, or fragment",
        ));
    }
    let retry_policy =
        resolve_retry_policy(config.retry_policy.as_ref(), "llm-deepseek: retryPolicy")
            .map_err(|error| fail(error.to_string()))?;
    Ok(DeepSeekConnectionOptions {
        base_url: base_url.into(),
        defaults: RequestDefaults {
            thinking: config.thinking.clone(),
            reasoning_effort: config.reasoning_effort.clone(),
        },
        max_tokens,
        default_context_window,
        models: resolve_models(config.models.as_deref())?,
        stream_idle_timeout_ms,
        max_request_files_bytes,
        max_inline_request_image_bytes,
        max_images_per_request,
        image_offload_byte_quantum,
        inline_image_offload_byte_quantum,
        image_offload_count_quantum,
        files_api_timeout_ms,
        file_policy: DeepSeekFilePolicy {
            expires_after_seconds: file_expires_after_seconds,
            refresh_margin_seconds: file_refresh_margin_seconds,
            quota_cleanup_batch: file_quota_cleanup_batch,
        },
        retry_policy,
    })
}
