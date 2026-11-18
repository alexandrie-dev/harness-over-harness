//! Provider-neutral vocabulary for web search and fetch.
use hoh_error::BoxError;
use hoh_llm::HarnessError;
use hoh_package_manifest::LocalizedText;
use hoh_timeout::AbortSignal;
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin};

pub struct WebProvidersChanged;
hoh_kernel::event!(
    WebProvidersChanged = "web/providers-changed",
    hoh_kernel::Contained
);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchRequest {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSearchResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    pub sources: Vec<WebSearchSource>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchSource {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebFetchRequest {
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebFetchResult {
    pub url: String,
    pub status_code: u64,
    pub body: WebFetchBody,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WebFetchBody {
    Html { content: String },
    Text { content: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebProviderInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<LocalizedText>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebProviderList {
    pub search: Vec<WebProviderInfo>,
    pub fetch: Vec<WebProviderInfo>,
}

/// Object-safe form of a provider's Promise-returning methods.
pub type WebFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, BoxError>> + Send + 'a>>;

pub trait WebSearchProvider: Send + Sync {
    /// Detached metadata; the id must stay stable throughout the registration.
    fn info(&self) -> WebProviderInfo;
    /// Cheap local usability check. Err represents a thrown provider exception.
    fn available(&self) -> Result<bool, BoxError>;
    fn ready<'a>(&'a self, _llm_provider: Option<&'a str>) -> WebFuture<'a, bool> {
        Box::pin(async { Ok(true) })
    }
    fn search<'a>(
        &'a self,
        request: WebSearchRequest,
        signal: Option<&'a AbortSignal>,
    ) -> WebFuture<'a, WebSearchResult>;
}

pub trait WebFetchProvider: Send + Sync {
    /// Detached metadata; the id must stay stable throughout the registration.
    fn info(&self) -> WebProviderInfo;
    /// Cheap local usability check. Err represents a thrown provider exception.
    fn available(&self) -> Result<bool, BoxError>;
    fn ready<'a>(&'a self, _llm_provider: Option<&'a str>) -> WebFuture<'a, bool> {
        Box::pin(async { Ok(true) })
    }
    fn fetch<'a>(
        &'a self,
        request: WebFetchRequest,
        signal: Option<&'a AbortSignal>,
    ) -> WebFuture<'a, WebFetchResult>;
}

/// Constructor namespace for the source HarnessError subclass, with an open code string.
pub struct WebError;
impl WebError {
    pub const NAME: &str = "WebError";
    #[allow(clippy::new_ret_no_self)] // PORTING §7 requires the common concrete error type.
    pub fn new(message: impl Into<String>, code: impl Into<String>) -> HarnessError {
        HarnessError::named(Self::NAME, message, code)
    }
}
