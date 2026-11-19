use hoh_credentials::{CredentialRef, credential_ref};
use hoh_kernel::Volatile;
use hoh_launch_environment::LaunchEnvironmentSnapshot;
use hoh_llm::{ResolvedRetryPolicy, RetryPolicyConfig, resolve_retry_policy};
use hoh_timeout::MAX_TIMER_DELAY_MS;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

pub const PUBLIC_BASE_URL: &str = "https://api.deepseek.com/anthropic";
const BASE_URL_ENV: &str = "DEEPSEEK_BASE_URL";
pub const DEFAULT_MODELS_TIMEOUT_MS: f64 = 5_000.0;
pub const DEFAULT_MODELS_MAX_AGE_MS: f64 = 600_000.0;
const DEFAULT_STREAM_IDLE_TIMEOUT_MS: f64 = 300_000.0;
const DEFAULT_CONTEXT_WINDOW: u64 = 1_000_000;
const DEFAULT_MAX_TOKENS: u64 = 256_000;
const DEFAULT_MAX_REQUEST_FILES_BYTES: u64 = 128 * 1024 * 1024;
const DEFAULT_MAX_INLINE_REQUEST_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
const DEFAULT_MAX_IMAGES_PER_REQUEST: u64 = 600;
const DEFAULT_IMAGE_OFFLOAD_BYTE_QUANTUM: u64 = 64 * 1024 * 1024;
const DEFAULT_INLINE_IMAGE_OFFLOAD_BYTE_QUANTUM: u64 = 10 * 1024 * 1024;
const DEFAULT_IMAGE_OFFLOAD_COUNT_QUANTUM: u64 = 20;
const DEFAULT_REQUEST_IMAGE_MAX_BYTES: u64 = 2 * 1024 * 1024;
const DEFAULT_FILES_API_TIMEOUT_MS: f64 = 20_000.0;
const DEFAULT_FILE_EXPIRY_SECONDS: u64 = 7 * 24 * 60 * 60;
const DEFAULT_FILE_REFRESH_MARGIN_SECONDS: u64 = 60 * 60;
const DEFAULT_FILE_QUOTA_CLEANUP_BATCH: u64 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Thinking {
    Enabled,
    Disabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Off,
    Low,
    High,
    Max,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ImagePixelBudget {
    Pixels(u64),
    Low(ImagePixelBudgetLow),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ImagePixelBudgetLow {
    #[serde(rename = "low")]
    Low,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeepSeekCatalogModel {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_modalities: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_pixel_budget: Option<ImagePixelBudget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_max_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt_update: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_update: Option<String>,
}

pub fn default_models() -> Vec<DeepSeekCatalogModel> {
    vec![
        DeepSeekCatalogModel {
            id: "deepseek-flash".into(),
            name: Some("DeepSeek-V4.1-Flash".into()),
            description: None,
            context_window: Some(1_048_576),
            max_tokens: None,
            input_modalities: Some(vec!["text".into(), "image".into()]),
            image_pixel_budget: None,
            image_max_bytes: None,
            system_prompt_update: Some("in-history".into()),
            tool_update: Some("addition-only".into()),
        },
        DeepSeekCatalogModel {
            id: "deepseek-v4-pro".into(),
            name: Some("DeepSeek-V4-Pro".into()),
            description: Some("Stronger agentic coding, knowledge, and difficult reasoning; suited to complex or quality-critical tasks at higher cost.".into()),
            context_window: Some(1_048_576),
            max_tokens: None,
            input_modalities: Some(vec!["text".into()]),
            image_pixel_budget: None,
            image_max_bytes: None,
            system_prompt_update: None,
            tool_update: None,
        },
    ]
}

fn v<T>(value: T) -> Volatile<T> {
    Volatile(value)
}
fn default_api_key_env() -> Volatile<String> {
    v("DEEPSEEK_API_KEY".into())
}
fn default_max_tokens() -> Volatile<u64> {
    v(DEFAULT_MAX_TOKENS)
}
fn default_context_window() -> Volatile<u64> {
    v(DEFAULT_CONTEXT_WINDOW)
}
fn default_models_config() -> Volatile<Vec<DeepSeekCatalogModel>> {
    v(default_models())
}
fn default_stream_idle_timeout() -> Volatile<f64> {
    v(DEFAULT_STREAM_IDLE_TIMEOUT_MS)
}
fn default_max_request_files_bytes() -> Volatile<u64> {
    v(DEFAULT_MAX_REQUEST_FILES_BYTES)
}
fn default_max_inline_request_image_bytes() -> Volatile<u64> {
    v(DEFAULT_MAX_INLINE_REQUEST_IMAGE_BYTES)
}
fn default_max_images_per_request() -> Volatile<u64> {
    v(DEFAULT_MAX_IMAGES_PER_REQUEST)
}
fn default_image_offload_byte_quantum() -> Volatile<u64> {
    v(DEFAULT_IMAGE_OFFLOAD_BYTE_QUANTUM)
}
fn default_inline_image_offload_byte_quantum() -> Volatile<u64> {
    v(DEFAULT_INLINE_IMAGE_OFFLOAD_BYTE_QUANTUM)
}
fn default_image_offload_count_quantum() -> Volatile<u64> {
    v(DEFAULT_IMAGE_OFFLOAD_COUNT_QUANTUM)
}
fn default_files_api_timeout() -> Volatile<f64> {
    v(DEFAULT_FILES_API_TIMEOUT_MS)
}
fn default_file_expiry() -> Volatile<u64> {
    v(DEFAULT_FILE_EXPIRY_SECONDS)
}
fn default_file_refresh_margin() -> Volatile<u64> {
    v(DEFAULT_FILE_REFRESH_MARGIN_SECONDS)
}
fn default_file_quota_cleanup_batch() -> Volatile<u64> {
    v(DEFAULT_FILE_QUOTA_CLEANUP_BATCH)
}
fn default_models_timeout() -> Volatile<f64> {
    v(DEFAULT_MODELS_TIMEOUT_MS)
}
fn default_models_max_age() -> Volatile<f64> {
    v(DEFAULT_MODELS_MAX_AGE_MS)
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    #[serde(rename = "baseURL")]
    pub base_url: Volatile<Option<String>>,
    #[serde(default)]
    pub thinking: Volatile<Option<Thinking>>,
    #[serde(default)]
    pub reasoning_effort: Volatile<Option<ReasoningEffort>>,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: Volatile<u64>,
    #[serde(default = "default_context_window")]
    pub default_context_window: Volatile<u64>,
    #[serde(default = "default_models_config")]
    pub models: Volatile<Vec<DeepSeekCatalogModel>>,
    #[serde(default = "default_stream_idle_timeout")]
    pub stream_idle_timeout_ms: Volatile<f64>,
    #[serde(default = "default_max_request_files_bytes")]
    pub max_request_files_bytes: Volatile<u64>,
    #[serde(default = "default_max_inline_request_image_bytes")]
    pub max_inline_request_image_bytes: Volatile<u64>,
    #[serde(default = "default_max_images_per_request")]
    pub max_images_per_request: Volatile<u64>,
    #[serde(default = "default_image_offload_byte_quantum")]
    pub image_offload_byte_quantum: Volatile<u64>,
    #[serde(default = "default_inline_image_offload_byte_quantum")]
    pub inline_image_offload_byte_quantum: Volatile<u64>,
    #[serde(default = "default_image_offload_count_quantum")]
    pub image_offload_count_quantum: Volatile<u64>,
    #[serde(default = "default_files_api_timeout")]
    pub files_api_timeout_ms: Volatile<f64>,
    #[serde(default = "default_file_expiry")]
    pub file_expires_after_seconds: Volatile<u64>,
    #[serde(default = "default_file_refresh_margin")]
    pub file_refresh_margin_seconds: Volatile<u64>,
    #[serde(default = "default_file_quota_cleanup_batch")]
    pub file_quota_cleanup_batch: Volatile<u64>,
    #[serde(default)]
    pub retry_policy: Volatile<Option<RetryPolicyConfig>>,
    #[serde(default = "default_models_timeout")]
    pub models_timeout_ms: Volatile<f64>,
    #[serde(default = "default_models_max_age")]
    pub models_max_age_ms: Volatile<f64>,
    #[serde(default = "default_api_key_env")]
    #[schemars(extend("x-role" = "credential-ref"))]
    pub api_key_env: Volatile<String>,
}

impl Default for Config {
    fn default() -> Self {
        serde_json::from_value(serde_json::json!({})).expect("default DeepSeek config")
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    #[serde(rename = "baseURL")]
    pub base_url: Option<String>,
    pub thinking: Option<Thinking>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub max_tokens: Option<u64>,
    pub default_context_window: Option<u64>,
    pub models: Option<Vec<DeepSeekCatalogModel>>,
    pub stream_idle_timeout_ms: Option<f64>,
    pub max_request_files_bytes: Option<u64>,
    pub max_inline_request_image_bytes: Option<u64>,
    pub max_images_per_request: Option<u64>,
    pub image_offload_byte_quantum: Option<u64>,
    pub inline_image_offload_byte_quantum: Option<u64>,
    pub image_offload_count_quantum: Option<u64>,
    pub files_api_timeout_ms: Option<f64>,
    pub file_expires_after_seconds: Option<u64>,
    pub file_refresh_margin_seconds: Option<u64>,
    pub file_quota_cleanup_batch: Option<u64>,
    pub retry_policy: Option<RetryPolicyConfig>,
    pub models_timeout_ms: Option<f64>,
    pub models_max_age_ms: Option<f64>,
    pub api_key_env: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestDefaults {
    pub thinking: Option<Thinking>,
    pub reasoning_effort: Option<ReasoningEffort>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePolicy {
    pub expires_after_seconds: u64,
    pub refresh_margin_seconds: u64,
    pub quota_cleanup_batch: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListingSettings {
    pub url: String,
    pub timeout_ms: f64,
    pub max_age_ms: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedDeepSeekOptions {
    pub base_url: String,
    pub defaults: RequestDefaults,
    pub max_tokens: u64,
    pub default_context_window: u64,
    pub models: Vec<DeepSeekCatalogModel>,
    pub stream_idle_timeout_ms: f64,
    pub max_request_files_bytes: u64,
    pub max_inline_request_image_bytes: u64,
    pub max_images_per_request: u64,
    pub image_offload_byte_quantum: u64,
    pub inline_image_offload_byte_quantum: u64,
    pub image_offload_count_quantum: u64,
    pub files_api_timeout_ms: f64,
    pub file_policy: FilePolicy,
    pub retry_policy: ResolvedRetryPolicy,
    pub api_key_env: CredentialRef,
    pub listing: Option<ListingSettings>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("{0}")]
pub struct ResolveError(pub String);

fn fail(message: impl Into<String>) -> ResolveError {
    ResolveError(message.into())
}

pub fn plain_options(config: &Config) -> Options {
    Options {
        base_url: config.base_url.0.clone(),
        thinking: config.thinking.0,
        reasoning_effort: config.reasoning_effort.0,
        max_tokens: Some(config.max_tokens.0),
        default_context_window: Some(config.default_context_window.0),
        models: Some(config.models.0.clone()),
        stream_idle_timeout_ms: Some(config.stream_idle_timeout_ms.0),
        max_request_files_bytes: Some(config.max_request_files_bytes.0),
        max_inline_request_image_bytes: Some(config.max_inline_request_image_bytes.0),
        max_images_per_request: Some(config.max_images_per_request.0),
        image_offload_byte_quantum: Some(config.image_offload_byte_quantum.0),
        inline_image_offload_byte_quantum: Some(config.inline_image_offload_byte_quantum.0),
        image_offload_count_quantum: Some(config.image_offload_count_quantum.0),
        files_api_timeout_ms: Some(config.files_api_timeout_ms.0),
        file_expires_after_seconds: Some(config.file_expires_after_seconds.0),
        file_refresh_margin_seconds: Some(config.file_refresh_margin_seconds.0),
        file_quota_cleanup_batch: Some(config.file_quota_cleanup_batch.0),
        retry_policy: config.retry_policy.0.clone(),
        models_timeout_ms: Some(config.models_timeout_ms.0),
        models_max_age_ms: Some(config.models_max_age_ms.0),
        api_key_env: Some(config.api_key_env.0.clone()),
    }
}

pub fn resolve_adapter_options(
    options: &Options,
    environment: Option<&LaunchEnvironmentSnapshot>,
) -> Result<ResolvedDeepSeekOptions, ResolveError> {
    let timeout_ms = listing_timeout(options.models_timeout_ms)?;
    let max_age_ms = listing_age(options.models_max_age_ms)?;
    if options.thinking == Some(Thinking::Disabled)
        && options
            .reasoning_effort
            .is_some_and(|value| value != ReasoningEffort::Off)
    {
        return Err(fail(
            "llm-deepseek: only reasoningEffort \"off\" can be configured when thinking is disabled",
        ));
    }
    let max_tokens = options.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);
    if max_tokens == 0 || max_tokens > 9_007_199_254_740_991 {
        return Err(fail(
            "llm-deepseek: maxTokens must be a positive safe integer",
        ));
    }
    let default_context_window = options
        .default_context_window
        .unwrap_or(DEFAULT_CONTEXT_WINDOW);
    if default_context_window == 0 {
        return Err(fail(
            "llm-deepseek: defaultContextWindow must be a positive integer",
        ));
    }
    let stream_idle_timeout_ms = finite_positive_timer(
        options
            .stream_idle_timeout_ms
            .unwrap_or(DEFAULT_STREAM_IDLE_TIMEOUT_MS),
        "streamIdleTimeoutMs",
    )?;
    let files_api_timeout_ms = finite_positive_timer(
        options
            .files_api_timeout_ms
            .unwrap_or(DEFAULT_FILES_API_TIMEOUT_MS),
        "filesApiTimeoutMs",
    )?;
    let max_request_files_bytes = positive_safe(
        options
            .max_request_files_bytes
            .unwrap_or(DEFAULT_MAX_REQUEST_FILES_BYTES),
        "maxRequestFilesBytes",
    )?;
    let max_inline_request_image_bytes = positive_safe(
        options
            .max_inline_request_image_bytes
            .unwrap_or(DEFAULT_MAX_INLINE_REQUEST_IMAGE_BYTES),
        "maxInlineRequestImageBytes",
    )?;
    let max_images_per_request = positive_safe(
        options
            .max_images_per_request
            .unwrap_or(DEFAULT_MAX_IMAGES_PER_REQUEST),
        "maxImagesPerRequest",
    )?;
    let image_offload_byte_quantum = positive_safe(
        options
            .image_offload_byte_quantum
            .unwrap_or(DEFAULT_IMAGE_OFFLOAD_BYTE_QUANTUM),
        "imageOffloadByteQuantum",
    )?;
    if image_offload_byte_quantum > max_request_files_bytes {
        return Err(fail(
            "llm-deepseek: imageOffloadByteQuantum must not exceed maxRequestFilesBytes",
        ));
    }
    let inline_image_offload_byte_quantum = positive_safe(
        options
            .inline_image_offload_byte_quantum
            .unwrap_or(DEFAULT_INLINE_IMAGE_OFFLOAD_BYTE_QUANTUM),
        "inlineImageOffloadByteQuantum",
    )?;
    if inline_image_offload_byte_quantum > max_inline_request_image_bytes {
        return Err(fail(
            "llm-deepseek: inlineImageOffloadByteQuantum must not exceed maxInlineRequestImageBytes",
        ));
    }
    let image_offload_count_quantum = positive_safe(
        options
            .image_offload_count_quantum
            .unwrap_or(DEFAULT_IMAGE_OFFLOAD_COUNT_QUANTUM),
        "imageOffloadCountQuantum",
    )?;
    if image_offload_count_quantum > max_images_per_request {
        return Err(fail(
            "llm-deepseek: imageOffloadCountQuantum must not exceed maxImagesPerRequest",
        ));
    }
    let file_expires_after_seconds = options
        .file_expires_after_seconds
        .unwrap_or(DEFAULT_FILE_EXPIRY_SECONDS);
    if !(3_600..=2_592_000).contains(&file_expires_after_seconds) {
        return Err(fail(
            "llm-deepseek: fileExpiresAfterSeconds must be an integer from 3600 through 2592000",
        ));
    }
    let file_refresh_margin_seconds = options
        .file_refresh_margin_seconds
        .unwrap_or(DEFAULT_FILE_REFRESH_MARGIN_SECONDS);
    if file_refresh_margin_seconds >= file_expires_after_seconds {
        return Err(fail(
            "llm-deepseek: fileRefreshMarginSeconds must be a non-negative integer below fileExpiresAfterSeconds",
        ));
    }
    let file_quota_cleanup_batch = options
        .file_quota_cleanup_batch
        .unwrap_or(DEFAULT_FILE_QUOTA_CLEANUP_BATCH);
    if !(1..=1_000).contains(&file_quota_cleanup_batch) {
        return Err(fail(
            "llm-deepseek: fileQuotaCleanupBatch must be an integer from 1 through 1000",
        ));
    }
    let models = resolve_models(options.models.as_deref().unwrap_or(&default_models()))?;
    let base_url = options
        .base_url
        .clone()
        .or_else(|| environment?.get(BASE_URL_ENV).map(|entry| entry.value))
        .unwrap_or_else(|| PUBLIC_BASE_URL.into());
    let parsed = Url::parse(&base_url).map_err(|error| fail(error.to_string()))?;
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
    let official = Url::parse(PUBLIC_BASE_URL).expect("public URL");
    let built_in = resolve_models(&default_models()).expect("built-in model catalog is valid");
    let listed = parsed.origin() == official.origin() && models == built_in;
    let api_key_env = credential_ref(options.api_key_env.as_deref().unwrap_or("DEEPSEEK_API_KEY"))
        .map_err(|error| fail(error.to_string()))?;
    let retry_policy =
        resolve_retry_policy(options.retry_policy.as_ref(), "llm-deepseek: retryPolicy")
            .map_err(|error| fail(error.to_string()))?;
    Ok(ResolvedDeepSeekOptions {
        base_url: base_url.clone(),
        defaults: RequestDefaults {
            thinking: options.thinking,
            reasoning_effort: options.reasoning_effort,
        },
        max_tokens,
        default_context_window,
        models,
        stream_idle_timeout_ms,
        max_request_files_bytes,
        max_inline_request_image_bytes,
        max_images_per_request,
        image_offload_byte_quantum,
        inline_image_offload_byte_quantum,
        image_offload_count_quantum,
        files_api_timeout_ms,
        file_policy: FilePolicy {
            expires_after_seconds: file_expires_after_seconds,
            refresh_margin_seconds: file_refresh_margin_seconds,
            quota_cleanup_batch: file_quota_cleanup_batch,
        },
        retry_policy,
        api_key_env,
        listing: listed.then(|| ListingSettings {
            url: model_listing_url(&base_url),
            timeout_ms,
            max_age_ms,
        }),
    })
}

fn finite_positive_timer(value: f64, name: &str) -> Result<f64, ResolveError> {
    if !value.is_finite() || value <= 0.0 || value > MAX_TIMER_DELAY_MS as f64 {
        return Err(fail(format!(
            "llm-deepseek: {name} must be a positive finite number no greater than {MAX_TIMER_DELAY_MS}"
        )));
    }
    Ok(value)
}

fn positive_safe(value: u64, name: &str) -> Result<u64, ResolveError> {
    if value == 0 || value > 9_007_199_254_740_991 {
        Err(fail(format!(
            "llm-deepseek: {name} must be a positive safe integer"
        )))
    } else {
        Ok(value)
    }
}

fn listing_timeout(value: Option<f64>) -> Result<f64, ResolveError> {
    let value = value.unwrap_or(DEFAULT_MODELS_TIMEOUT_MS);
    if !value.is_finite() || value <= 0.0 || value > MAX_TIMER_DELAY_MS as f64 {
        return Err(fail(format!(
            "llm-deepseek-api-key: modelsTimeoutMs must be a positive finite number no greater than {MAX_TIMER_DELAY_MS}"
        )));
    }
    Ok(value)
}

fn listing_age(value: Option<f64>) -> Result<f64, ResolveError> {
    let value = value.unwrap_or(DEFAULT_MODELS_MAX_AGE_MS);
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > 9_007_199_254_740_991.0
    {
        return Err(fail(
            "llm-deepseek-api-key: modelsMaxAgeMs must be a non-negative safe integer",
        ));
    }
    Ok(value)
}

fn resolve_models(
    models: &[DeepSeekCatalogModel],
) -> Result<Vec<DeepSeekCatalogModel>, ResolveError> {
    let mut seen = std::collections::HashSet::new();
    let mut resolved = Vec::with_capacity(models.len());
    for model in models {
        if model.id.is_empty() {
            return Err(fail("llm-deepseek: catalog model ids must be non-empty"));
        }
        if model.name.as_ref().is_some_and(String::is_empty) {
            return Err(fail(format!(
                "llm-deepseek: catalog model \"{}\" has an empty name",
                model.id
            )));
        }
        if !seen.insert(model.id.clone()) {
            return Err(fail(format!(
                "llm-deepseek: duplicate catalog model \"{}\"",
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
        let mut next = model.clone();
        let modalities = next
            .input_modalities
            .clone()
            .unwrap_or_else(|| vec!["text".into()]);
        if modalities.is_empty() {
            return Err(fail(format!(
                "llm-deepseek: catalog model \"{}\" inputModalities must not be empty",
                model.id
            )));
        }
        if modalities
            .iter()
            .any(|value| value != "text" && value != "image")
        {
            return Err(fail(format!(
                "llm-deepseek: catalog model \"{}\" inputModalities must contain only \"text\" and \"image\"",
                model.id
            )));
        }
        if modalities
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != modalities.len()
        {
            return Err(fail(format!(
                "llm-deepseek: catalog model \"{}\" inputModalities must not contain duplicates",
                model.id
            )));
        }
        let has_image = modalities.iter().any(|value| value == "image");
        if !has_image && (next.image_pixel_budget.is_some() || next.image_max_bytes.is_some()) {
            return Err(fail(format!(
                "llm-deepseek: text-only catalog model \"{}\" cannot declare image request limits",
                model.id
            )));
        }
        if matches!(next.image_pixel_budget, Some(ImagePixelBudget::Pixels(0))) {
            return Err(fail(format!(
                "llm-deepseek: catalog model \"{}\" imagePixelBudget must be \"low\" or a positive safe integer",
                model.id
            )));
        }
        if next.image_max_bytes == Some(0) {
            return Err(fail(format!(
                "llm-deepseek: catalog model \"{}\" imageMaxBytes must be a positive safe integer",
                model.id
            )));
        }
        if next
            .system_prompt_update
            .as_deref()
            .is_some_and(|value| value != "in-history")
        {
            return Err(fail(format!(
                "llm-deepseek: catalog model \"{}\" systemPromptUpdate must be \"in-history\" when present",
                model.id
            )));
        }
        if next
            .tool_update
            .as_deref()
            .is_some_and(|value| value != "in-history" && value != "addition-only")
        {
            return Err(fail(format!(
                "llm-deepseek: catalog model \"{}\" toolUpdate must be \"in-history\" or \"addition-only\" when present",
                model.id
            )));
        }
        next.input_modalities = Some(modalities);
        if has_image && next.image_max_bytes.is_none() {
            next.image_max_bytes = Some(DEFAULT_REQUEST_IMAGE_MAX_BYTES);
        }
        resolved.push(next);
    }
    Ok(resolved)
}

pub(crate) fn model_listing_url(base_url: &str) -> String {
    let mut url = Url::parse(base_url).expect("resolved base URL");
    url.set_path("/models");
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}
