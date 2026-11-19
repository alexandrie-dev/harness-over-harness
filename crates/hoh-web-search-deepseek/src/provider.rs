//! DeepSeek native web search over its Anthropic-compatible Messages API.

use crate::types::{AnthropicResponse, ContentBlock};
use futures_util::future::BoxFuture;
use hoh_agent::spawn_inherited;
use hoh_credentials::CredentialRef;
use hoh_error::{BoxError, HarnessError};
use hoh_package_manifest::LocalizedText;
use hoh_timeout::AbortSignal;
use hoh_web::{
    WebError, WebFuture, WebProviderInfo, WebSearchProvider, WebSearchRequest, WebSearchResult,
    WebSearchSource,
};
use indexmap::IndexMap;
use reqwest::{
    Client, Request, Response, StatusCode,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use url::Url;

pub const DEEPSEEK_PROVIDER_ID: &str = "deepseek-official";
pub const DEEPSEEK_DEFAULT_BASE_URL: &str = "https://api.deepseek.com/anthropic/v1";
pub const DEEPSEEK_DEFAULT_MODEL: &str = "deepseek-v4-flash";
pub const DEEPSEEK_DEFAULT_API_VERSION: &str = "2023-06-01";
pub const DEEPSEEK_DEFAULT_MAX_TOKENS: u64 = 4096;
pub const DEEPSEEK_DEFAULT_MAX_USES: u64 = 5;
const USER_AGENT: &str = "deepseek-harness/0.0.1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepSeekSearchLlmRequest {
    pub endpoint: String,
    pub api_version: String,
    pub body: DeepSeekSearchRequestBody,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeepSeekSearchRequestBody {
    pub model: String,
    pub max_tokens: u64,
    pub messages: Vec<DeepSeekSearchMessage>,
    pub tools: Vec<DeepSeekSearchTool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<DeepSeekThinking>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeepSeekSearchMessage {
    pub role: String,
    pub content: Vec<DeepSeekSearchText>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeepSeekSearchText {
    pub r#type: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeepSeekSearchTool {
    pub r#type: String,
    pub name: String,
    pub max_uses: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeepSeekThinking {
    pub r#type: String,
}

pub type CredentialFuture<T> = BoxFuture<'static, Result<T, BoxError>>;
pub type ResolveAccountToken =
    Arc<dyn Fn(String) -> CredentialFuture<Option<String>> + Send + Sync>;
pub type RejectAccountToken = Arc<dyn Fn(String) -> CredentialFuture<()> + Send + Sync>;
pub type ResolveApiKey = Arc<dyn Fn() -> CredentialFuture<Option<String>> + Send + Sync>;
pub type RecordRequest =
    Arc<dyn Fn(DeepSeekSearchLlmRequest) -> Result<(), BoxError> + Send + Sync>;

#[derive(Clone)]
pub struct DeepSeekSearchProviderOptions {
    pub resolve_account_token: Option<ResolveAccountToken>,
    pub reject_account_token: Option<RejectAccountToken>,
    pub api_key: Option<String>,
    pub resolve_api_key: Option<ResolveApiKey>,
    pub api_key_env: Option<CredentialRef>,
    pub base_url: String,
    pub model: String,
    pub api_version: String,
    pub max_tokens: u64,
    pub max_uses: u64,
    pub record_request: Option<RecordRequest>,
}

type OptionsResolver =
    dyn Fn() -> Result<DeepSeekSearchProviderOptions, BoxError> + Send + Sync + 'static;
type ReadyResolver = dyn Fn(Option<String>) -> CredentialFuture<bool> + Send + Sync + 'static;

pub struct DeepSeekSearchProvider {
    client: Result<Client, Arc<str>>,
    resolve_options: Arc<OptionsResolver>,
    resolve_ready: Arc<ReadyResolver>,
}

impl DeepSeekSearchProvider {
    pub fn new(
        resolve_options: impl Fn() -> DeepSeekSearchProviderOptions + Send + Sync + 'static,
    ) -> Self {
        let resolve_options = Arc::new(resolve_options);
        let ready_options = resolve_options.clone();
        Self {
            client: Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|error| Arc::<str>::from(error.to_string())),
            resolve_options: Arc::new(move || Ok(resolve_options())),
            resolve_ready: Arc::new(move |_| {
                let options = ready_options();
                Box::pin(async move { Ok(has_credential_path(&options)) })
            }),
        }
    }

    /// Fallible options mirror a JavaScript options thunk that may throw.
    pub(crate) fn with_ready(
        client: Client,
        resolve_options: impl Fn() -> Result<DeepSeekSearchProviderOptions, BoxError>
        + Send
        + Sync
        + 'static,
        resolve_ready: impl Fn(Option<String>) -> CredentialFuture<bool> + Send + Sync + 'static,
    ) -> Self {
        Self {
            client: Ok(client),
            resolve_options: Arc::new(resolve_options),
            resolve_ready: Arc::new(resolve_ready),
        }
    }

    fn options(&self) -> Result<DeepSeekSearchProviderOptions, BoxError> {
        (self.resolve_options)()
    }

    async fn search_inner(
        &self,
        request: WebSearchRequest,
        signal: Option<&AbortSignal>,
    ) -> Result<WebSearchResult, BoxError> {
        let options = self.options()?;
        let endpoint = format!("{}/messages", options.base_url);
        let auth = auth_headers(&options, &endpoint, signal).await?;
        throw_if_search_aborted(signal)?;

        let body = DeepSeekSearchRequestBody {
            model: options.model.clone(),
            max_tokens: options.max_tokens,
            messages: vec![DeepSeekSearchMessage {
                role: "user".into(),
                content: vec![DeepSeekSearchText {
                    r#type: "text".into(),
                    text: format!("Perform a web search for the query: {}", request.query),
                }],
            }],
            tools: vec![DeepSeekSearchTool {
                r#type: "web_search_20250305".into(),
                name: "web_search".into(),
                max_uses: options.max_uses,
            }],
            thinking: Some(DeepSeekThinking {
                r#type: "disabled".into(),
            }),
        };
        if let Some(record) = &options.record_request {
            record(DeepSeekSearchLlmRequest {
                endpoint: endpoint.clone(),
                api_version: options.api_version.clone(),
                body: body.clone(),
            })?;
        }
        throw_if_search_aborted(signal)?;

        let client = self.client.as_ref().map_err(|error| {
            Box::new(search_endpoint_error(
                &endpoint,
                &format!("DeepSeek search request failed: Error: {error}"),
                None,
            )) as BoxError
        })?;
        let request = build_request(client, &endpoint, &auth, &options.api_version, &body)
            .map_err(|error| {
                Box::new(search_endpoint_error(
                    &endpoint,
                    &format!(
                        "DeepSeek search request failed: {}",
                        js_error_string(&error)
                    ),
                    Some(error),
                )) as BoxError
            })?;
        let response = abortable_reqwest(client.execute(request), signal)
            .await
            .map_err(|error| {
                if error_code(&error) == Some("WEB_ABORTED") {
                    error
                } else {
                    Box::new(search_endpoint_error(
                        &endpoint,
                        &format!(
                            "DeepSeek search request failed: {}",
                            js_error_string(&error)
                        ),
                        Some(error),
                    )) as BoxError
                }
            })?;

        finish_response(response, auth, &endpoint, signal).await
    }
}

fn build_request(
    client: &Client,
    endpoint: &str,
    auth: &SearchAuth,
    api_version: &str,
    body: &DeepSeekSearchRequestBody,
) -> Result<Request, BoxError> {
    let headers = request_headers(&auth.headers, api_version)?;
    let bytes = serde_json::to_vec(body)?;
    client
        .post(endpoint)
        .headers(headers)
        .body(bytes)
        .build()
        .map_err(|error| Box::new(error) as BoxError)
}

impl WebSearchProvider for DeepSeekSearchProvider {
    fn info(&self) -> WebProviderInfo {
        WebProviderInfo {
            id: DEEPSEEK_PROVIDER_ID.into(),
            title: None,
            description: Some(LocalizedText::Translations(IndexMap::from([
                ("en".into(), "Slower, and every search uses extra model tokens. Requires a DeepSeek account or API key.".into()),
                ("zh".into(), "速度较慢，每次搜索都会额外消耗模型 token。需要 DeepSeek 账号或 API Key。".into()),
                ("ja".into(), "低速で、検索のたびに追加のモデルトークンを消費します。DeepSeek アカウントまたは API キーが必要です。".into()),
                ("es".into(), "Más lenta, y cada búsqueda consume tokens adicionales del modelo. Requiere una cuenta de DeepSeek o una clave de API.".into()),
                ("fr".into(), "Plus lente, et chaque recherche consomme des jetons de modèle supplémentaires. Nécessite un compte DeepSeek ou une clé API.".into()),
                ("de".into(), "Langsamer, und jede Suche verbraucht zusätzliche Modell-Tokens. Erfordert ein DeepSeek-Konto oder einen API-Schlüssel.".into()),
                ("ru".into(), "Работает медленнее, и каждый поиск расходует дополнительные токены модели. Требуется учётная запись DeepSeek или ключ API.".into()),
                ("pt".into(), "Mais lenta, e cada pesquisa consome tokens extras do modelo. Requer uma conta DeepSeek ou uma chave de API.".into()),
            ]))),
        }
    }

    fn available(&self) -> Result<bool, BoxError> {
        let options = self.options()?;
        Ok(has_credential_path(&options)
            && Url::parse(&options.base_url).is_ok()
            && options.max_tokens > 0
            && options.max_uses > 0)
    }

    fn ready<'a>(&'a self, llm_provider: Option<&'a str>) -> WebFuture<'a, bool> {
        let operation = (self.resolve_ready)(llm_provider.map(str::to_owned));
        Box::pin(operation)
    }

    fn search<'a>(
        &'a self,
        request: WebSearchRequest,
        signal: Option<&'a AbortSignal>,
    ) -> WebFuture<'a, WebSearchResult> {
        Box::pin(self.search_inner(request, signal))
    }
}

fn has_credential_path(options: &DeepSeekSearchProviderOptions) -> bool {
    options.api_key.as_ref().is_some_and(|key| !key.is_empty())
        || options.resolve_api_key.is_some()
        || options.resolve_account_token.is_some()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AuthKind {
    Account,
    ApiKey,
}

struct SearchAuth {
    kind: AuthKind,
    headers: Vec<(&'static str, String)>,
    reject: Option<Arc<dyn Fn() -> CredentialFuture<()> + Send + Sync>>,
}

async fn finish_response(
    response: Response,
    auth: SearchAuth,
    endpoint: &str,
    signal: Option<&AbortSignal>,
) -> Result<WebSearchResult, BoxError> {
    let status = response.status();
    if !status.is_success() {
        let parsed = abortable_reqwest(response.bytes(), signal)
            .await
            .and_then(|bytes| parse_response_json::<Value>(&bytes));
        return Err(http_response_error(status, parsed, auth, endpoint).await);
    }

    let payload = abortable_reqwest(response.bytes(), signal)
        .await
        .and_then(|bytes| parse_response_json::<AnthropicResponse>(&bytes))
        .map_err(|error| {
            if error_code(&error) == Some("WEB_ABORTED") {
                error
            } else {
                Box::new(search_endpoint_error(
                    endpoint,
                    &format!(
                        "DeepSeek returned an unprocessable response body: {}",
                        js_error_string(&error)
                    ),
                    Some(error),
                )) as BoxError
            }
        })?;
    map_anthropic_response(payload).map_err(|error| {
        let message = HarnessError::of(error.as_ref()).map_or_else(
            || format!("DeepSeek returned an unprocessable response body: {error}"),
            |error| error.message().to_owned(),
        );
        Box::new(search_endpoint_error(endpoint, &message, Some(error))) as BoxError
    })
}

async fn http_response_error(
    status: StatusCode,
    parsed: Result<Value, BoxError>,
    auth: SearchAuth,
    endpoint: &str,
) -> BoxError {
    let mut message = format!("DeepSeek API error (HTTP {})", status.as_u16());
    let mut aborted = None;
    match parsed {
        Ok(value) => {
            if let Some(detail) = error_detail(&value) {
                message.push_str(": ");
                message.push_str(detail);
            }
        }
        Err(error) if error_code(&error) == Some("WEB_ABORTED") => aborted = Some(error),
        Err(_) => {}
    }
    if status == StatusCode::UNAUTHORIZED && auth.kind == AuthKind::Account {
        if let Some(reject) = auth.reject {
            let _ = reject().await;
        }
        if let Some(aborted) = aborted {
            return aborted;
        }
        return account_rejected_error(&message).into();
    }
    if let Some(aborted) = aborted {
        return aborted;
    }
    search_endpoint_error(endpoint, &message, None).into()
}

async fn auth_headers(
    options: &DeepSeekSearchProviderOptions,
    endpoint: &str,
    signal: Option<&AbortSignal>,
) -> Result<SearchAuth, BoxError> {
    throw_if_search_aborted(signal)?;
    if let Some(resolve) = &options.resolve_account_token {
        let resolve = resolve.clone();
        let endpoint = endpoint.to_owned();
        let token = resolve_credential(resolve(endpoint), signal).await?;
        if let Some(token) = token.filter(|token| !token.is_empty()) {
            let sent_token = token.clone();
            let reject = options.reject_account_token.as_ref().map(|reject| {
                let reject = reject.clone();
                Arc::new(move || reject(sent_token.clone()))
                    as Arc<dyn Fn() -> CredentialFuture<()> + Send + Sync>
            });
            return Ok(SearchAuth {
                kind: AuthKind::Account,
                headers: vec![("x-dsh-auth-token", token)],
                reject,
            });
        }
    }
    throw_if_search_aborted(signal)?;
    let api_key = if let Some(key) = options.api_key.as_ref().filter(|key| !key.is_empty()) {
        Some(key.clone())
    } else if let Some(resolve) = &options.resolve_api_key {
        resolve_credential(resolve(), signal).await?
    } else {
        None
    };
    let Some(api_key) = api_key.filter(|key| !key.is_empty()) else {
        let reference = options
            .api_key_env
            .as_ref()
            .map_or("DEEPSEEK_API_KEY", CredentialRef::as_str);
        return Err(WebError::new(
            format!(
                "DeepSeek search has no API key for {reference:?}; store it through the credentials service (the web Models page writes it), export it in the launching environment, or set a literal \"apiKey\" in the web-search-deepseek config; a conversation using a DeepSeek Account model searches with the account sign-in instead"
            ),
            "WEB_PROVIDER_CREDENTIAL_MISSING",
        )
        .into());
    };
    Ok(SearchAuth {
        kind: AuthKind::ApiKey,
        headers: vec![
            ("x-api-key", api_key.clone()),
            ("authorization", format!("Bearer {api_key}")),
        ],
        reject: None,
    })
}

async fn resolve_credential<T>(
    operation: CredentialFuture<T>,
    signal: Option<&AbortSignal>,
) -> Result<T, BoxError>
where
    T: Send + 'static,
{
    let observes_rejection = signal.is_some();
    let result = match signal {
        None => operation.await,
        Some(signal) => {
            let mut task = spawn_inherited(operation);
            tokio::select! {
                biased;
                reason = signal.cancelled() => Err(search_aborted(Some(Box::new(reason))).into()),
                joined = &mut task => joined.map_err(|error| Box::new(error) as BoxError)?,
            }
        }
    };
    result.map_err(|error| {
        if error_code(&error) == Some("WEB_ABORTED") {
            error
        } else {
            WebError::new(
                format!(
                    "DeepSeek search credential resolution failed: {}",
                    credential_error_string(&error, observes_rejection)
                ),
                "WEB_PROVIDER_ERROR",
            )
            .with_cause(error)
            .into()
        }
    })
}

fn credential_error_string(error: &BoxError, observed_rejection: bool) -> String {
    let text = js_error_string(error);
    if !observed_rejection {
        return text;
    }
    format!("Error: {}", text.strip_prefix("Error: ").unwrap_or(&text))
}

async fn abortable_reqwest<T>(
    operation: impl Future<Output = Result<T, reqwest::Error>>,
    signal: Option<&AbortSignal>,
) -> Result<T, BoxError> {
    match signal {
        None => operation.await.map_err(|error| Box::new(error) as BoxError),
        Some(signal) => tokio::select! {
            biased;
            reason = signal.cancelled() => Err(search_aborted(Some(Box::new(reason))).into()),
            result = operation => result.map_err(|error| Box::new(error) as BoxError),
        },
    }
}

fn throw_if_search_aborted(signal: Option<&AbortSignal>) -> Result<(), BoxError> {
    if let Some(signal) = signal
        && signal.aborted()
    {
        return Err(
            search_aborted(signal.reason().map(|reason| Box::new(reason) as BoxError)).into(),
        );
    }
    Ok(())
}

fn search_aborted(cause: Option<BoxError>) -> HarnessError {
    let error = WebError::new("DeepSeek search aborted", "WEB_ABORTED");
    cause.map_or(error.clone(), |cause| error.with_cause(cause))
}

fn account_rejected_error(message: &str) -> HarnessError {
    WebError::new(
        format!(
            "{message}\n\nDeepSeek rejected the account sign-in used for this web search. Guide the user to sign in to DeepSeek again; the search endpoint does not need changing."
        ),
        "WEB_PROVIDER_ERROR",
    )
}

fn search_endpoint_error(endpoint: &str, message: &str, cause: Option<BoxError>) -> HarnessError {
    let endpoint = Value::String(endpoint.to_owned()).to_string();
    let error = WebError::new(
        format!(
            "{message}\n\nThe web request used endpoint {endpoint}. Search endpoint configuration is separate from chat. If that endpoint is not intended, guide the user to Plugins > Built-in plugin settings > Web search and fetch, select DeepSeek (legacy) under Search, and change and save its Base URL. If that settings page is unavailable, the user can set web-search-deepseek.baseURL to a trusted DeepSeek endpoint. Only the user should choose or change the endpoint."
        ),
        "WEB_PROVIDER_ERROR",
    );
    cause.map_or(error.clone(), |cause| error.with_cause(cause))
}

fn error_detail(value: &Value) -> Option<&str> {
    let top_level = || value.get("message").and_then(Value::as_str);
    let detail = match value.get("error") {
        Some(Value::String(error)) => Some(error.as_str()),
        Some(error) => match error.get("message") {
            Some(Value::String(message)) => Some(message.as_str()),
            Some(Value::Null) | None => top_level(),
            Some(_) => None,
        },
        None => top_level(),
    };
    detail.filter(|detail| !detail.is_empty())
}

fn error_code(error: &BoxError) -> Option<&str> {
    HarnessError::of(error.as_ref()).map(HarnessError::code)
}

fn js_error_string(error: &BoxError) -> String {
    HarnessError::of(error.as_ref()).map_or_else(
        || format!("Error: {error}"),
        |error| format!("{}: {}", error.name(), error.message()),
    )
}

fn insert_header(
    headers: &mut HeaderMap,
    name: &str,
    value: &str,
    sensitive: bool,
) -> Result<(), BoxError> {
    let name =
        HeaderName::from_bytes(name.as_bytes()).map_err(|error| Box::new(error) as BoxError)?;
    let mut value = HeaderValue::from_str(value).map_err(|error| Box::new(error) as BoxError)?;
    value.set_sensitive(sensitive);
    headers.insert(name, value);
    Ok(())
}

fn request_headers(
    authentication: &[(&'static str, String)],
    api_version: &str,
) -> Result<HeaderMap, BoxError> {
    let mut headers = HeaderMap::new();
    for (name, value) in authentication {
        insert_header(&mut headers, name, value, true)?;
    }
    insert_header(&mut headers, "anthropic-version", api_version, false)?;
    insert_header(&mut headers, "content-type", "application/json", false)?;
    insert_header(&mut headers, "accept", "application/json", false)?;
    insert_header(&mut headers, "user-agent", USER_AGENT, false)?;
    Ok(headers)
}

fn parse_response_json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, BoxError> {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    serde_json::from_slice(bytes).map_err(|error| Box::new(error) as BoxError)
}

pub fn citation_snippets(blocks: &[ContentBlock]) -> Result<IndexMap<String, String>, BoxError> {
    let mut snippets = IndexMap::new();
    for block in blocks {
        if block.is_null() {
            return Err(response_shape_error(
                "Cannot read properties of null (reading 'type')",
            ));
        }
        if block.get("type").and_then(Value::as_str) != Some("text") {
            continue;
        }
        let Some(citations) = block.get("citations") else {
            continue;
        };
        if citations.is_null() || citations.is_string() {
            continue;
        }
        let Some(citations) = citations.as_array() else {
            return Err(response_shape_error("block.citations is not iterable"));
        };
        for citation in citations {
            if citation.is_null() {
                return Err(response_shape_error(
                    "Cannot read properties of null (reading 'url')",
                ));
            }
            let Some(url) = citation
                .get("url")
                .and_then(Value::as_str)
                .filter(|url| !url.is_empty())
            else {
                continue;
            };
            let Some(text) = citation
                .get("cited_text")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
            else {
                continue;
            };
            snippets.entry(url.into()).or_insert_with(|| text.into());
        }
    }
    Ok(snippets)
}

pub fn map_anthropic_response(response: AnthropicResponse) -> Result<WebSearchResult, BoxError> {
    let blocks = response.content.unwrap_or_default();
    if blocks.iter().any(Value::is_null) {
        return Err(response_shape_error(
            "Cannot read properties of null (reading 'type')",
        ));
    }
    if !blocks
        .iter()
        .any(|block| block.get("type").and_then(Value::as_str) == Some("web_search_tool_result"))
    {
        return Err(WebError::new(
            "DeepSeek returned no web_search_tool_result blocks; the request may not have triggered native web search",
            "WEB_PROVIDER_ERROR",
        )
        .into());
    }
    let snippets = citation_snippets(&blocks)?;
    let mut seen = IndexMap::<String, ()>::new();
    let mut sources = Vec::new();
    for block in &blocks {
        if block.get("type").and_then(Value::as_str) != Some("web_search_tool_result") {
            continue;
        }
        let Some(content) = block.get("content") else {
            continue;
        };
        if content.is_null() {
            continue;
        }
        if content.is_string() {
            continue;
        }
        let Some(items) = content.as_array() else {
            return Err(response_shape_error(
                "web_search_tool_result content is not iterable",
            ));
        };
        for item in items {
            if item.is_null() {
                return Err(response_shape_error(
                    "Cannot read properties of null (reading 'type')",
                ));
            }
            if item.get("type").and_then(Value::as_str) != Some("web_search_result") {
                continue;
            }
            let Some(url) = item.get("url").and_then(Value::as_str) else {
                return Err(response_shape_error(
                    "web_search_result url is not a string",
                ));
            };
            if url.is_empty() || seen.contains_key(url) {
                continue;
            }
            seen.insert(url.into(), ());
            sources.push(WebSearchSource {
                url: url.into(),
                title: non_empty_string(item.get("title")),
                snippet: snippets.get(url).cloned(),
                summary: None,
                published_at: non_empty_string(item.get("page_age")),
            });
        }
    }
    Ok(WebSearchResult {
        content: None,
        sources,
        truncated: false,
    })
}

fn response_shape_error(message: &'static str) -> BoxError {
    #[derive(Debug)]
    struct ResponseShapeError(&'static str);

    impl std::fmt::Display for ResponseShapeError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "TypeError: {}", self.0)
        }
    }

    impl std::error::Error for ResponseShapeError {}

    Box::new(ResponseShapeError(message))
}

fn non_empty_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoh_agent::{
        Agent, AgentOptions, AgentRegistryPlugin, AgentStatus, Agents, CancelOptions, Inbox,
        InboxTarget, MaintenanceFuture, MaintenanceTask, Model,
    };
    use hoh_kernel::Context;
    use hoh_llm::UserMessage;
    use hoh_llm_deepseek::{DeepSeekAccount, DeepSeekAccountApi};
    use hoh_scope::ScopeKey;
    use hoh_session::{AgentCancelCause, Session, SessionId};
    use hoh_session_projection::SessionProjectionRegistry;
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    struct InitiatorAgent {
        id: SessionId,
        scope: ScopeKey,
        session: Session,
        ctx: Context,
        options: AgentOptions,
    }

    impl InitiatorAgent {
        fn new(ctx: &Context) -> Arc<Self> {
            let session = Session::create(
                SessionId::from("web-search-initiator"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
            Arc::new(Self {
                id: session.id().clone(),
                scope: ScopeKey::new(),
                session,
                ctx: ctx.clone(),
                options: AgentOptions::default(),
            })
        }
    }

    impl Agent for InitiatorAgent {
        fn id(&self) -> &SessionId {
            &self.id
        }
        fn scope_key(&self) -> ScopeKey {
            self.scope.clone()
        }
        fn options(&self) -> &AgentOptions {
            &self.options
        }
        fn session(&self) -> &Session {
            &self.session
        }
        fn model(&self) -> &dyn Model {
            panic!("unused")
        }
        fn inbox(&self) -> &dyn Inbox {
            panic!("unused")
        }
        fn status(&self) -> AgentStatus {
            AgentStatus::Idle
        }
        fn ctx(&self) -> &Context {
            &self.ctx
        }
        fn cancel(&self, _: AgentCancelCause, _: CancelOptions) -> Result<(), BoxError> {
            Ok(())
        }
        fn when_idle(&self) -> BoxFuture<'static, ()> {
            Box::pin(async {})
        }
        fn run_maintenance_erased(
            &self,
            _: MaintenanceTask,
        ) -> Result<MaintenanceFuture, BoxError> {
            panic!("unused")
        }
        fn send(&self, _: UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
            panic!("unused")
        }
        fn followup(&self, _: UserMessage) -> Result<(), BoxError> {
            panic!("unused")
        }
        fn steer(&self, _: UserMessage) -> Result<(), BoxError> {
            panic!("unused")
        }
        fn inject(&self, _: UserMessage) -> Result<(), BoxError> {
            panic!("unused")
        }
    }

    struct RouteAccount {
        asked: Arc<Mutex<Vec<String>>>,
    }

    impl DeepSeekAccountApi for RouteAccount {
        fn resolve_token<'a>(
            &'a self,
            endpoint: &'a str,
        ) -> BoxFuture<'a, Result<Option<String>, BoxError>> {
            self.asked.lock().unwrap().push(endpoint.to_owned());
            Box::pin(async { Ok(Some("account-token".into())) })
        }

        fn reject_token<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
            Box::pin(async { Ok(()) })
        }
    }

    fn options(base_url: &str) -> DeepSeekSearchProviderOptions {
        DeepSeekSearchProviderOptions {
            resolve_account_token: None,
            reject_account_token: None,
            api_key: Some("key".into()),
            resolve_api_key: None,
            api_key_env: None,
            base_url: base_url.into(),
            model: "model".into(),
            api_version: "2023-06-01".into(),
            max_tokens: 16,
            max_uses: 1,
            record_request: None,
        }
    }

    fn response(status: StatusCode, body: &str) -> Response {
        http::Response::builder()
            .status(status)
            .body(body.to_owned())
            .unwrap()
            .into()
    }

    #[test]
    fn response_json_accepts_utf8_bom() {
        let response = parse_response_json::<AnthropicResponse>(
            b"\xef\xbb\xbf{\"content\":[{\"type\":\"web_search_tool_result\"}]}",
        )
        .unwrap();
        assert_eq!(
            response.content.unwrap()[0]["type"],
            "web_search_tool_result"
        );
        let error = parse_response_json::<Value>(b"\xef\xbb\xbf{\"error\":\"bad\"}").unwrap();
        assert_eq!(error_detail(&error), Some("bad"));
    }

    #[test]
    fn error_detail_matches_anthropic_string_object_and_fallback_shapes() {
        assert_eq!(
            error_detail(&serde_json::json!({"error":"string detail"})),
            Some("string detail")
        );
        assert_eq!(
            error_detail(&serde_json::json!({"error":{"message":"nested"}})),
            Some("nested")
        );
        assert_eq!(
            error_detail(&serde_json::json!({"error":{"code":1},"message":"top"})),
            Some("top")
        );
        assert_eq!(error_detail(&serde_json::json!({"error":{}})), None);
        assert_eq!(
            error_detail(&serde_json::json!({
                "error":{"message":42},
                "message":"must not be used"
            })),
            None
        );
    }

    #[tokio::test]
    async fn credential_headers_are_exact_and_sensitive_but_protocol_headers_are_not() {
        let mut account_options = options("https://api.test/v1");
        account_options.api_key = Some("fallback-key".into());
        account_options.resolve_account_token = Some(Arc::new(|_| {
            Box::pin(async { Ok(Some("account-token".into())) })
        }));
        let account_auth = auth_headers(&account_options, "https://api.test/v1/messages", None)
            .await
            .unwrap();
        assert_eq!(
            account_auth.headers,
            vec![("x-dsh-auth-token", "account-token".into())]
        );

        let mut key_options = options("https://api.test/v1");
        key_options.api_key = Some("api-key".into());
        let key_auth = auth_headers(&key_options, "https://api.test/v1/messages", None)
            .await
            .unwrap();
        assert_eq!(
            key_auth.headers,
            vec![
                ("x-api-key", "api-key".into()),
                ("authorization", "Bearer api-key".into())
            ]
        );

        let headers = request_headers(
            &[
                ("x-api-key", "secret".into()),
                ("authorization", "Bearer secret".into()),
            ],
            "2023-06-01",
        )
        .unwrap();
        assert!(headers["x-api-key"].is_sensitive());
        assert!(headers["authorization"].is_sensitive());
        assert!(!headers["anthropic-version"].is_sensitive());

        let account =
            request_headers(&[("x-dsh-auth-token", "token".into())], "2023-06-01").unwrap();
        assert!(account["x-dsh-auth-token"].is_sensitive());
        assert!(!account.contains_key("x-api-key"));
        assert!(!account.contains_key("authorization"));
    }

    #[test]
    fn built_request_is_the_recorded_post_body_with_all_protocol_headers() {
        let client = Client::builder().no_proxy().build().unwrap();
        let auth = SearchAuth {
            kind: AuthKind::ApiKey,
            headers: vec![
                ("x-api-key", "secret".into()),
                ("authorization", "Bearer secret".into()),
            ],
            reject: None,
        };
        let body = DeepSeekSearchRequestBody {
            model: "deepseek-chat".into(),
            max_tokens: 4096,
            messages: vec![DeepSeekSearchMessage {
                role: "user".into(),
                content: vec![DeepSeekSearchText {
                    r#type: "text".into(),
                    text: "Perform a web search for the query: hello".into(),
                }],
            }],
            tools: vec![DeepSeekSearchTool {
                r#type: "web_search_20250305".into(),
                name: "web_search".into(),
                max_uses: 5,
            }],
            thinking: Some(DeepSeekThinking {
                r#type: "disabled".into(),
            }),
        };
        let request = build_request(
            &client,
            "https://api.deepseek.test/anthropic/v1/messages",
            &auth,
            "2023-06-01",
            &body,
        )
        .unwrap();
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(
            request.url().as_str(),
            "https://api.deepseek.test/anthropic/v1/messages"
        );
        assert_eq!(request.headers()["x-api-key"], "secret");
        assert_eq!(request.headers()["authorization"], "Bearer secret");
        assert_eq!(request.headers()["anthropic-version"], "2023-06-01");
        assert_eq!(request.headers()["content-type"], "application/json");
        assert_eq!(request.headers()["accept"], "application/json");
        assert_eq!(request.headers()["user-agent"], USER_AGENT);
        let sent: Value =
            serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(sent, serde_json::to_value(body).unwrap());
    }

    #[tokio::test]
    async fn search_uses_the_client_stored_by_the_provider() {
        let fixed = options("http://example.invalid/v1");
        let mut provider = DeepSeekSearchProvider::new(move || fixed.clone());
        provider.client = Ok(Client::builder()
            .no_proxy()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap());
        let error = provider
            .search(
                WebSearchRequest {
                    query: "q".into(),
                    max_results: None,
                },
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(
            HarnessError::of(error.as_ref()).unwrap().code(),
            "WEB_PROVIDER_ERROR"
        );
    }

    #[tokio::test]
    async fn account_401_rejects_once_but_403_and_api_key_401_do_not() {
        let rejected = Arc::new(AtomicUsize::new(0));
        let calls = rejected.clone();
        let reject = Arc::new(move || {
            calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(std::io::Error::other("removal failed").into()) })
                as CredentialFuture<()>
        });
        let account = || SearchAuth {
            kind: AuthKind::Account,
            headers: vec![("x-dsh-auth-token", "token".into())],
            reject: Some(reject.clone()),
        };
        let error = http_response_error(
            StatusCode::UNAUTHORIZED,
            Ok(serde_json::json!({"error":"invalid token"})),
            account(),
            "https://api.test/messages",
        )
        .await;
        let error = HarnessError::of(error.as_ref()).unwrap();
        assert_eq!(error.code(), "WEB_PROVIDER_ERROR");
        assert_eq!(
            error.message(),
            "DeepSeek API error (HTTP 401): invalid token\n\nDeepSeek rejected the account sign-in used for this web search. Guide the user to sign in to DeepSeek again; the search endpoint does not need changing."
        );
        assert_eq!(rejected.load(Ordering::SeqCst), 1);

        let error = http_response_error(
            StatusCode::FORBIDDEN,
            Ok(serde_json::json!({})),
            account(),
            "https://api.test/messages",
        )
        .await;
        assert_eq!(
            HarnessError::of(error.as_ref()).unwrap().code(),
            "WEB_PROVIDER_ERROR"
        );
        assert_eq!(rejected.load(Ordering::SeqCst), 1);

        let api_key = SearchAuth {
            kind: AuthKind::ApiKey,
            headers: vec![("x-api-key", "key".into())],
            reject: None,
        };
        let _ = http_response_error(
            StatusCode::UNAUTHORIZED,
            Ok(serde_json::json!({})),
            api_key,
            "https://api.test/messages",
        )
        .await;
        assert_eq!(rejected.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn account_401_still_rejects_when_body_read_was_aborted() {
        let rejected = Arc::new(AtomicUsize::new(0));
        let calls = rejected.clone();
        let error = http_response_error(
            StatusCode::UNAUTHORIZED,
            Err(search_aborted(None).into()),
            SearchAuth {
                kind: AuthKind::Account,
                headers: vec![("x-dsh-auth-token", "token".into())],
                reject: Some(Arc::new(move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async { Ok(()) })
                })),
            },
            "https://api.test/messages",
        )
        .await;
        assert_eq!(
            HarnessError::of(error.as_ref()).unwrap().code(),
            "WEB_ABORTED"
        );
        assert_eq!(rejected.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn response_boundary_handles_account_401_and_exact_sent_token() {
        let rejected = Arc::new(Mutex::new(Vec::new()));
        let captured = rejected.clone();
        let mut value = options("https://api.test/v1");
        value.resolve_account_token = Some(Arc::new(|_| {
            Box::pin(async { Ok(Some("sent-token".into())) })
        }));
        value.reject_account_token = Some(Arc::new(move |token| {
            captured.lock().unwrap().push(token);
            Box::pin(async { Err(std::io::Error::other("removal failed").into()) })
        }));
        let auth = auth_headers(&value, "https://api.test/v1/messages", None)
            .await
            .unwrap();
        let error = finish_response(
            response(
                StatusCode::UNAUTHORIZED,
                r#"{"error":{"message":"invalid token"}}"#,
            ),
            auth,
            "https://api.test/v1/messages",
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(
            HarnessError::of(error.as_ref()).unwrap().message(),
            "DeepSeek API error (HTTP 401): invalid token\n\nDeepSeek rejected the account sign-in used for this web search. Guide the user to sign in to DeepSeek again; the search endpoint does not need changing."
        );
        assert_eq!(*rejected.lock().unwrap(), vec!["sent-token"]);
    }

    #[tokio::test]
    async fn response_boundary_covers_http_and_malformed_body_diagnostics() {
        let endpoint = "https://api.test/v1/messages";
        let api_auth = || SearchAuth {
            kind: AuthKind::ApiKey,
            headers: vec![("x-api-key", "key".into())],
            reject: None,
        };
        for (status, body, expected) in [
            (
                StatusCode::TOO_MANY_REQUESTS,
                r#"{"error":"slow down"}"#,
                "DeepSeek API error (HTTP 429): slow down",
            ),
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "not json",
                "DeepSeek API error (HTTP 503)",
            ),
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "{}",
                "DeepSeek API error (HTTP 500)",
            ),
            (
                StatusCode::BAD_REQUEST,
                r#"{"error":"bad request"}"#,
                "DeepSeek API error (HTTP 400): bad request",
            ),
        ] {
            let error = finish_response(response(status, body), api_auth(), endpoint, None)
                .await
                .unwrap_err();
            let message = HarnessError::of(error.as_ref()).unwrap().message();
            assert!(message.starts_with(expected));
            assert!(message.contains("Search endpoint configuration is separate from chat."));
        }

        let error = finish_response(
            response(
                StatusCode::TOO_MANY_REQUESTS,
                r#"{"error":{"message":"rate limited"}}"#,
            ),
            api_auth(),
            endpoint,
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(
            HarnessError::of(error.as_ref()).unwrap().message(),
            "DeepSeek API error (HTTP 429): rate limited\n\nThe web request used endpoint \"https://api.test/v1/messages\". Search endpoint configuration is separate from chat. If that endpoint is not intended, guide the user to Plugins > Built-in plugin settings > Web search and fetch, select DeepSeek (legacy) under Search, and change and save its Base URL. If that settings page is unavailable, the user can set web-search-deepseek.baseURL to a trusted DeepSeek endpoint. Only the user should choose or change the endpoint."
        );

        for body in [
            r#"{"content":{}}"#,
            r#"{"content":[{"type":"text","text":"prose"}]}"#,
            r#"{"content":[{"type":"web_search_tool_result","content":{}}]}"#,
            r#"{"content":[{"type":"web_search_tool_result","content":[null]}]}"#,
        ] {
            let error = finish_response(response(StatusCode::OK, body), api_auth(), endpoint, None)
                .await
                .unwrap_err();
            let error = HarnessError::of(error.as_ref()).unwrap();
            assert_eq!(error.code(), "WEB_PROVIDER_ERROR");
            assert!(error.message().contains(endpoint));
            if body.contains("web_search_tool_result") {
                assert!(
                    error.message().starts_with(
                        "DeepSeek returned an unprocessable response body: TypeError:"
                    ),
                    "unexpected malformed-response message: {}",
                    error.message()
                );
            }
        }
    }

    #[tokio::test]
    async fn response_boundary_returns_successful_sources() {
        let result = finish_response(
            response(
                StatusCode::OK,
                r#"{"content":[{"type":"text","citations":[{"url":"https://result.test","cited_text":"excerpt"}]},{"type":"web_search_tool_result","content":[{"type":"web_search_result","url":"https://result.test","title":"Result"}]}]}"#,
            ),
            SearchAuth {
                kind: AuthKind::ApiKey,
                headers: vec![("x-api-key", "key".into())],
                reject: None,
            },
            "https://api.test/v1/messages",
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.sources.len(), 1);
        assert_eq!(result.sources[0].url, "https://result.test");
        assert_eq!(result.sources[0].title.as_deref(), Some("Result"));
        assert_eq!(result.sources[0].snippet.as_deref(), Some("excerpt"));
    }

    #[tokio::test]
    async fn response_boundary_aborts_401_body_but_still_rejects_account_token() {
        let rejected = Arc::new(AtomicUsize::new(0));
        let calls = rejected.clone();
        let body = reqwest::Body::wrap_stream(futures_util::stream::pending::<
            Result<Vec<u8>, std::io::Error>,
        >());
        let response: Response = http::Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(body)
            .unwrap()
            .into();
        let controller = hoh_timeout::AbortController::new();
        let signal = controller.signal();
        let running = tokio::spawn(async move {
            finish_response(
                response,
                SearchAuth {
                    kind: AuthKind::Account,
                    headers: vec![("x-dsh-auth-token", "sent-token".into())],
                    reject: Some(Arc::new(move || {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Box::pin(async { Ok(()) })
                    })),
                },
                "https://api.test/v1/messages",
                Some(&signal),
            )
            .await
        });
        tokio::task::yield_now().await;
        controller.abort_with(std::io::Error::other("caller stopped"));
        let error = running.await.unwrap().unwrap_err();
        assert_eq!(
            HarnessError::of(error.as_ref()).unwrap().code(),
            "WEB_ABORTED"
        );
        assert_eq!(rejected.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn request_send_and_success_body_observe_cancellation() {
        let controller = hoh_timeout::AbortController::new();
        let signal = controller.signal();
        let send = tokio::spawn(async move {
            abortable_reqwest(
                futures_util::future::pending::<Result<Response, reqwest::Error>>(),
                Some(&signal),
            )
            .await
        });
        tokio::task::yield_now().await;
        controller.abort_with(std::io::Error::other("cancel send"));
        let error = send.await.unwrap().unwrap_err();
        assert_eq!(
            HarnessError::of(error.as_ref()).unwrap().code(),
            "WEB_ABORTED"
        );

        let body = reqwest::Body::wrap_stream(futures_util::stream::pending::<
            Result<Vec<u8>, std::io::Error>,
        >());
        let response: Response = http::Response::builder()
            .status(StatusCode::OK)
            .body(body)
            .unwrap()
            .into();
        let controller = hoh_timeout::AbortController::new();
        let signal = controller.signal();
        let body_read = tokio::spawn(async move {
            finish_response(
                response,
                SearchAuth {
                    kind: AuthKind::ApiKey,
                    headers: vec![("x-api-key", "key".into())],
                    reject: None,
                },
                "https://api.test/v1/messages",
                Some(&signal),
            )
            .await
        });
        tokio::task::yield_now().await;
        controller.abort_with(std::io::Error::other("cancel body"));
        let error = body_read.await.unwrap().unwrap_err();
        assert_eq!(
            HarnessError::of(error.as_ref()).unwrap().code(),
            "WEB_ABORTED"
        );
    }

    #[tokio::test]
    async fn detached_credential_resolution_inherits_the_initiating_agent() {
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        let projection = ctx
            .plugin::<SessionProjectionRegistry>(serde_json::json!({}))
            .unwrap();
        projection.settled().await.unwrap();
        let registry = ctx
            .plugin::<AgentRegistryPlugin>(serde_json::json!({}))
            .unwrap();
        registry.settled().await.unwrap();
        let agents = (*ctx.get::<Agents>().unwrap()).clone();
        let initiator: Arc<dyn Agent> = InitiatorAgent::new(&ctx);
        let expected = initiator.id().as_str().to_owned();
        let resolver_ctx = ctx.clone();
        let controller = hoh_timeout::AbortController::new();
        let signal = controller.signal();
        let resolved = agents
            .with_initiator(initiator, async move {
                let operation: CredentialFuture<Option<String>> = Box::pin(async move {
                    tokio::task::yield_now().await;
                    let current = resolver_ctx
                        .get::<Agents>()
                        .unwrap()
                        .current_initiator()?
                        .map(|agent| agent.id().as_str().to_owned());
                    Ok(current)
                });
                resolve_credential(operation, Some(&signal)).await
            })
            .unwrap()
            .await
            .unwrap();
        assert_eq!(resolved.as_deref(), Some(expected.as_str()));
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn plugin_options_select_account_route_and_record_the_session_event() {
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        let projection = ctx
            .plugin::<SessionProjectionRegistry>(serde_json::json!({}))
            .unwrap();
        projection.settled().await.unwrap();
        let registry = ctx
            .plugin::<AgentRegistryPlugin>(serde_json::json!({}))
            .unwrap();
        registry.settled().await.unwrap();
        let asked = Arc::new(Mutex::new(Vec::new()));
        ctx.provide::<DeepSeekAccount>(Arc::new(RouteAccount {
            asked: asked.clone(),
        }))
        .await
        .unwrap();
        let agents = (*ctx.get::<Agents>().unwrap()).clone();
        let initiator = InitiatorAgent::new(&ctx);
        let options = crate::resolve_options(&ctx, &crate::Config::default()).unwrap();
        let resolve = options.resolve_account_token.unwrap();
        let token = resolve_credential(
            resolve("https://api.test/messages".into()),
            Some(&hoh_timeout::AbortController::new().signal()),
        )
        .await
        .unwrap();
        assert_eq!(token, None);
        assert!(asked.lock().unwrap().is_empty());
        initiator
            .session()
            .append(
                "request/context",
                serde_json::json!({"provider":"deepseek-account","model":"m"}),
                None,
            )
            .unwrap();
        let mut config = crate::Config::default();
        config.api_key.0 = Some("api-key".into());
        let options = crate::resolve_options(&ctx, &config).unwrap();
        let resolve = options.resolve_account_token.clone().unwrap();
        let record = options.record_request.clone().unwrap();
        let request = DeepSeekSearchLlmRequest {
            endpoint: "https://api.test/messages".into(),
            api_version: "2023-06-01".into(),
            body: DeepSeekSearchRequestBody {
                model: "m".into(),
                max_tokens: 1,
                messages: vec![],
                tools: vec![],
                thinking: None,
            },
        };
        let operation_agent: Arc<dyn Agent> = initiator.clone();
        let token = agents
            .with_initiator(operation_agent, async move {
                let token = resolve_credential(
                    resolve("https://api.test/messages".into()),
                    Some(&hoh_timeout::AbortController::new().signal()),
                )
                .await?;
                record(request)?;
                Ok::<_, BoxError>(token)
            })
            .unwrap()
            .await
            .unwrap();
        assert_eq!(token.as_deref(), Some("account-token"));
        assert_eq!(*asked.lock().unwrap(), vec!["https://api.test/messages"]);
        let event = initiator.session().own_events().last().unwrap().clone();
        assert_eq!(event.event_type(), "web/deepseek-search-llm-request");
        assert_eq!(event.data()["endpoint"], "https://api.test/messages");

        initiator
            .session()
            .append(
                "request/context",
                serde_json::json!({"provider":"deepseek-official","model":"m"}),
                None,
            )
            .unwrap();
        let options = crate::resolve_options(&ctx, &config).unwrap();
        let resolve = options.resolve_account_token.unwrap();
        let operation_agent: Arc<dyn Agent> = initiator;
        let token = agents
            .with_initiator(operation_agent, async move {
                resolve_credential(
                    resolve("https://api.test/messages".into()),
                    Some(&hoh_timeout::AbortController::new().signal()),
                )
                .await
            })
            .unwrap()
            .await
            .unwrap();
        assert_eq!(token, None);
        assert_eq!(asked.lock().unwrap().len(), 1);
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn active_signal_wraps_harness_resolver_rejection_like_a_js_error() {
        let controller = hoh_timeout::AbortController::new();
        let signal = controller.signal();
        let error = resolve_credential::<Option<String>>(
            Box::pin(async {
                Err(WebError::new("credential failed", "WEB_PROVIDER_MISCONFIGURED").into())
            }),
            Some(&signal),
        )
        .await
        .unwrap_err();
        assert_eq!(
            HarnessError::of(error.as_ref()).unwrap().message(),
            "DeepSeek search credential resolution failed: Error: WebError: credential failed"
        );

        let error = resolve_credential::<Option<String>>(
            Box::pin(async {
                Err(WebError::new("credential failed", "WEB_PROVIDER_MISCONFIGURED").into())
            }),
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(
            HarnessError::of(error.as_ref()).unwrap().message(),
            "DeepSeek search credential resolution failed: WebError: credential failed"
        );
    }
}
