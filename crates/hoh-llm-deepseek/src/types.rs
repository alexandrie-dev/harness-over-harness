//! Model catalog and request-local DeepSeek Messages types.

use hoh_error::{BoxError, HarnessError};
use hoh_llm::{ResolvedRetryPolicy, RetryPolicyConfig};
use serde::{Deserialize, Deserializer, Serialize, de::IgnoredAny};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ImagePixelBudget {
    Pixels(u64),
    Low(ImageLow),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageLow {
    #[serde(rename = "low")]
    Low,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
    /// Removed source setting retained only so deserialized legacy config fails loudly.
    #[doc(hidden)]
    #[serde(
        default,
        rename = "imageDetail",
        skip_serializing,
        deserialize_with = "legacy_image_detail_present"
    )]
    pub image_detail: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_pixel_budget: Option<ImagePixelBudget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_max_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt_update: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_update: Option<String>,
}

fn legacy_image_detail_present<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    IgnoredAny::deserialize(deserializer)?;
    Ok(true)
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepSeekFilePolicy {
    pub expires_after_seconds: u64,
    pub refresh_margin_seconds: u64,
    pub quota_cleanup_batch: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepSeekConnectionOptions {
    #[serde(rename = "baseURL")]
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
    pub file_policy: DeepSeekFilePolicy,
    pub retry_policy: ResolvedRetryPolicy,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    #[serde(default, rename = "baseURL", skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<DeepSeekCatalogModel>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_idle_timeout_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_request_files_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_inline_request_image_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_images_per_request: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_offload_byte_quantum: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_image_offload_byte_quantum: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_offload_count_quantum: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_api_timeout_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_expires_after_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_refresh_margin_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_quota_cleanup_batch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_policy: Option<RetryPolicyConfig>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

pub type ResolvedDeepSeekOptions = DeepSeekConnectionOptions;

pub type DeepSeekRequestErrorFuture =
    Pin<Box<dyn Future<Output = Result<HarnessError, BoxError>> + Send + 'static>>;
pub type DeepSeekRequestErrorMapper =
    dyn Fn(HarnessError) -> DeepSeekRequestErrorFuture + Send + Sync + 'static;

#[derive(Clone, Default)]
pub struct DeepSeekRequestAuth {
    pub headers: indexmap::IndexMap<String, String>,
    pub on_request_error: Option<Arc<DeepSeekRequestErrorMapper>>,
}

impl std::fmt::Debug for DeepSeekRequestAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeepSeekRequestAuth")
            .field("headers", &self.headers.keys().collect::<Vec<_>>())
            .field("on_request_error", &self.on_request_error.is_some())
            .finish()
    }
}
