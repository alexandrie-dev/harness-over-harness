use futures_util::future::BoxFuture;
use hoh_credentials::CredentialRef;
use hoh_error::{BoxError, HarnessError};
use hoh_timeout::AbortSignal;
use hoh_web::{WebError, WebFuture};
use reqwest::{
    Client, StatusCode,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use serde_json::Value;
use std::{collections::BTreeMap, future::Future, sync::Arc};

const USER_AGENT: &str = "deepseek-harness/0.0.1";

pub type CredentialFuture<T> = BoxFuture<'static, Result<T, BoxError>>;
pub type ConfiguredCredential = Arc<dyn Fn() -> CredentialFuture<bool> + Send + Sync>;
pub type ResolveCredential = Arc<dyn Fn() -> CredentialFuture<Option<String>> + Send + Sync>;
pub type ResolveAccountToken =
    Arc<dyn Fn(String) -> CredentialFuture<Option<String>> + Send + Sync>;
pub type RejectAccountToken = Arc<dyn Fn(String) -> CredentialFuture<()> + Send + Sync>;
pub type ApiKeyHeaders = Arc<dyn Fn(&str) -> BTreeMap<String, String> + Send + Sync>;
pub type RecordRequest = Arc<dyn Fn(&str) -> Result<(), BoxError> + Send + Sync>;
pub type WaitToSend = Arc<dyn Fn(Option<AbortSignal>) -> CredentialFuture<()> + Send + Sync>;

#[derive(Clone)]
pub enum DeepSeekWebApiKey {
    Literal(String),
    Referenced {
        reference: CredentialRef,
        configured: ConfiguredCredential,
        resolve: ResolveCredential,
    },
}

#[derive(Clone)]
pub enum DeepSeekWebAuthSource {
    Account {
        base_url: String,
        resolve_token: ResolveAccountToken,
        reject_token: RejectAccountToken,
    },
    ApiKey {
        base_url: String,
        key: DeepSeekWebApiKey,
    },
    Misconfigured {
        message: String,
    },
}

#[derive(Clone, Default)]
pub struct DeepSeekWebAuth {
    pub sources: Vec<DeepSeekWebAuthSource>,
    pub missing_credential_message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeepSeekWebGuidance {
    pub subject: &'static str,
    pub plugin: &'static str,
    pub custom: bool,
    pub base_url_field: String,
    pub api_key_env_field: String,
}

pub fn has_deep_seek_web_credential(auth: &DeepSeekWebAuth) -> bool {
    auth.sources.iter().any(|source| match source {
        DeepSeekWebAuthSource::ApiKey {
            key: DeepSeekWebApiKey::Literal(key),
            ..
        } => !key.is_empty(),
        _ => true,
    })
}

pub async fn is_deep_seek_web_auth_ready(auth: &DeepSeekWebAuth, path: &str) -> bool {
    for source in &auth.sources {
        match source {
            DeepSeekWebAuthSource::Misconfigured { .. } => return false,
            DeepSeekWebAuthSource::Account {
                base_url,
                resolve_token,
                ..
            } => match resolve_token(join_path(base_url, path)).await {
                Ok(Some(token)) if !token.is_empty() => return true,
                Ok(_) => {}
                Err(_) => return false,
            },
            DeepSeekWebAuthSource::ApiKey { key, .. } => match key {
                DeepSeekWebApiKey::Literal(key) if !key.is_empty() => return true,
                DeepSeekWebApiKey::Literal(_) => {}
                DeepSeekWebApiKey::Referenced { configured, .. } => match configured().await {
                    Ok(true) => return true,
                    Ok(false) => {}
                    Err(_) => return false,
                },
            },
        }
    }
    false
}

pub fn invalid_response() -> HarnessError {
    WebError::new(
        "DeepSeek returned an invalid response body",
        "WEB_PROVIDER_ERROR",
    )
}

pub fn search_aborted(cause: Option<BoxError>) -> HarnessError {
    let error = WebError::new("DeepSeek web request aborted", "WEB_ABORTED");
    cause.map_or(error.clone(), |cause| error.with_cause(cause))
}

pub struct DeepSeekWebRequest<T> {
    pub client: Client,
    pub path: String,
    pub body: Value,
    pub headers: BTreeMap<String, String>,
    pub api_key_headers: Option<ApiKeyHeaders>,
    pub decode: Arc<dyn Fn(Value) -> Result<T, BoxError> + Send + Sync>,
    pub guidance: DeepSeekWebGuidance,
    pub record_request: Option<RecordRequest>,
    pub wait_to_send: Option<WaitToSend>,
}

enum ResolvedAuth {
    Account {
        endpoint: String,
        token: String,
        reject: RejectAccountToken,
    },
    ApiKey {
        endpoint: String,
        key: String,
    },
}

impl std::fmt::Debug for ResolvedAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResolvedAuth")
            .field("endpoint", &self.endpoint())
            .finish_non_exhaustive()
    }
}

impl ResolvedAuth {
    fn endpoint(&self) -> &str {
        match self {
            Self::Account { endpoint, .. } | Self::ApiKey { endpoint, .. } => endpoint,
        }
    }
}

pub async fn dispatch_deep_seek_web<T: Send + 'static>(
    auth: &DeepSeekWebAuth,
    request: &DeepSeekWebRequest<T>,
    signal: Option<&AbortSignal>,
) -> Result<T, BoxError> {
    throw_if_aborted(signal)?;
    let resolved = auth_headers(auth, request, signal).await?;
    if let Some(wait) = &request.wait_to_send {
        abortable(wait(signal.cloned()), signal).await?;
    }
    throw_if_aborted(signal)?;
    if let Some(record) = &request.record_request {
        record(resolved.endpoint())?;
    }
    throw_if_aborted(signal)?;

    let body = serde_json::to_vec(&request.body).map_err(|error| {
        endpoint_error(
            &request.guidance,
            resolved.endpoint(),
            &format!("DeepSeek web request failed: Error: {error}"),
            Some(Box::new(error)),
        )
    })?;
    let headers = request_headers(&resolved, request).map_err(|error| {
        endpoint_error(
            &request.guidance,
            resolved.endpoint(),
            &format!("DeepSeek web request failed: {error}"),
            Some(error),
        )
    })?;
    let outgoing = request
        .client
        .post(resolved.endpoint())
        .headers(headers)
        .body(body);
    let response = abortable_reqwest(outgoing.send(), signal)
        .await
        .map_err(|error| {
            if code(&error) == Some("WEB_ABORTED") {
                error
            } else {
                endpoint_error(
                    &request.guidance,
                    resolved.endpoint(),
                    &format!("DeepSeek web request failed: {error}"),
                    Some(error),
                )
                .into()
            }
        })?;
    let status = response.status();
    if !status.is_success() {
        let parsed = abortable_reqwest(response.bytes(), signal)
            .await
            .and_then(|bytes| parse_response_json(&bytes));
        let mut message = format!("DeepSeek API error (HTTP {})", status.as_u16());
        match parsed {
            Ok(value) => {
                if let Some(detail) = error_detail(&value) {
                    message.push_str(": ");
                    message.push_str(detail);
                }
            }
            Err(error) if code(&error) == Some("WEB_ABORTED") => {
                if status == StatusCode::UNAUTHORIZED
                    && let ResolvedAuth::Account { token, reject, .. } = &resolved
                {
                    let _ = reject(token.clone()).await;
                }
                return Err(error);
            }
            Err(_) => {}
        }
        if status == StatusCode::UNAUTHORIZED
            && let ResolvedAuth::Account { token, reject, .. } = &resolved
        {
            let _ = reject(token.clone()).await;
            return Err(WebError::new(
                format!("{message}\n\nDeepSeek rejected the account sign-in used for this web request. Guide the user to sign in to DeepSeek again; the endpoint does not need changing."),
                "WEB_PROVIDER_ERROR",
            ).into());
        }
        return Err(endpoint_error(&request.guidance, resolved.endpoint(), &message, None).into());
    }
    let payload = abortable_reqwest(response.bytes(), signal)
        .await
        .and_then(|bytes| parse_response_json(&bytes))
        .map_err(|error| {
            if code(&error) == Some("WEB_ABORTED") {
                error
            } else {
                endpoint_error(
                    &request.guidance,
                    resolved.endpoint(),
                    &format!("DeepSeek returned an unprocessable response body: {error}"),
                    Some(error),
                )
                .into()
            }
        })?;
    throw_if_aborted(signal)?;
    (request.decode)(payload).map_err(|error| {
        let message = HarnessError::of(error.as_ref()).map_or_else(
            || format!("DeepSeek returned an unprocessable response body: {error}"),
            |error| error.message().to_owned(),
        );
        endpoint_error(
            &request.guidance,
            resolved.endpoint(),
            &message,
            Some(error),
        )
        .into()
    })
}

async fn auth_headers<T>(
    auth: &DeepSeekWebAuth,
    request: &DeepSeekWebRequest<T>,
    signal: Option<&AbortSignal>,
) -> Result<ResolvedAuth, BoxError> {
    throw_if_aborted(signal)?;
    for source in &auth.sources {
        throw_if_aborted(signal)?;
        match source {
            DeepSeekWebAuthSource::Misconfigured { message } => {
                return Err(WebError::new(message, "WEB_PROVIDER_MISCONFIGURED").into());
            }
            DeepSeekWebAuthSource::Account {
                base_url,
                resolve_token,
                reject_token,
            } => {
                let endpoint = join_path(base_url, &request.path);
                if let Some(token) =
                    resolve_credential(resolve_token(endpoint.clone()), signal).await?
                    && !token.is_empty()
                {
                    return Ok(ResolvedAuth::Account {
                        endpoint,
                        token,
                        reject: reject_token.clone(),
                    });
                }
            }
            DeepSeekWebAuthSource::ApiKey { base_url, key } => {
                let key = match key {
                    DeepSeekWebApiKey::Literal(key) => Some(key.clone()),
                    DeepSeekWebApiKey::Referenced { resolve, .. } => {
                        resolve_credential(resolve(), signal).await?
                    }
                };
                if let Some(key) = key.filter(|key| !key.is_empty()) {
                    return Ok(ResolvedAuth::ApiKey {
                        endpoint: join_path(base_url, &request.path),
                        key,
                    });
                }
            }
        }
    }
    Err(WebError::new(
        auth.missing_credential_message
            .as_deref()
            .unwrap_or("DeepSeek web access has no credential to send"),
        "WEB_PROVIDER_CREDENTIAL_MISSING",
    )
    .into())
}

async fn resolve_credential<T>(
    operation: CredentialFuture<T>,
    signal: Option<&AbortSignal>,
) -> Result<T, BoxError>
where
    T: Send + 'static,
{
    throw_if_aborted(signal)?;
    let result = match signal {
        None => operation.await,
        Some(signal) => {
            let mut task = tokio::spawn(operation);
            tokio::select! {
                biased;
                reason = signal.cancelled() => Err(search_aborted(Some(Box::new(reason))).into()),
                joined = &mut task => joined.map_err(|error| Box::new(error) as BoxError)?,
            }
        }
    };
    result.map_err(|error| {
        if code(&error) == Some("WEB_ABORTED") {
            error
        } else {
            WebError::new(
                format!(
                    "DeepSeek web credential resolution failed: {}",
                    js_error_string(&error)
                ),
                "WEB_PROVIDER_ERROR",
            )
            .with_cause(error)
            .into()
        }
    })
}

async fn abortable<T>(
    operation: impl Future<Output = Result<T, BoxError>>,
    signal: Option<&AbortSignal>,
) -> Result<T, BoxError> {
    match signal {
        None => operation.await,
        Some(signal) => {
            tokio::select! {
                biased;
                reason = signal.cancelled() => Err(search_aborted(Some(Box::new(reason))).into()),
                result = operation => result,
            }
        }
    }
}

async fn abortable_reqwest<T>(
    operation: impl Future<Output = Result<T, reqwest::Error>>,
    signal: Option<&AbortSignal>,
) -> Result<T, BoxError> {
    match signal {
        None => operation.await.map_err(|error| Box::new(error) as BoxError),
        Some(signal) => {
            tokio::select! {
                biased;
                reason = signal.cancelled() => Err(search_aborted(Some(Box::new(reason))).into()),
                result = operation => result.map_err(|error| Box::new(error) as BoxError),
            }
        }
    }
}

fn throw_if_aborted(signal: Option<&AbortSignal>) -> Result<(), BoxError> {
    if let Some(signal) = signal
        && signal.aborted()
    {
        return Err(
            search_aborted(signal.reason().map(|reason| Box::new(reason) as BoxError)).into(),
        );
    }
    Ok(())
}

fn endpoint_error(
    guidance: &DeepSeekWebGuidance,
    endpoint: &str,
    message: &str,
    cause: Option<BoxError>,
) -> HarnessError {
    let base = format!("{}.{}", guidance.plugin, guidance.base_url_field);
    let key = format!("{}.{}", guidance.plugin, guidance.api_key_env_field);
    let recovery = if guidance.custom {
        format!(
            "{} sends the API key under {key} to {base}. If that endpoint is not intended, guide the user to correct {base} and {key}, or to remove both so {} uses the DeepSeek credential of the conversation.",
            guidance.subject, guidance.subject
        )
    } else {
        format!(
            "{} derives this endpoint from the base URL of the DeepSeek model provider whose credential it uses. If that endpoint is not intended, guide the user to check that provider's base URL on the Models page, or to set {base} to a trusted DeepSeek endpoint and {key} to the credential reference of its API key.",
            guidance.subject
        )
    };
    let error = WebError::new(
        format!(
            "{message}\n\nThe web request used endpoint {}. {recovery} Only the user should choose or change the endpoint.",
            Value::String(endpoint.to_owned())
        ),
        "WEB_PROVIDER_ERROR",
    );
    cause.map_or(error.clone(), |cause| error.with_cause(cause))
}

fn error_detail(value: &Value) -> Option<&str> {
    let detail = match value.get("error") {
        Some(Value::String(error)) => Some(error.as_str()),
        Some(Value::Object(error)) => error.get("message").and_then(Value::as_str),
        _ => value.get("message").and_then(Value::as_str),
    };
    detail.filter(|detail| !detail.is_empty())
}

fn join_path(base_url: &str, path: &str) -> String {
    format!("{}{path}", base_url.trim_end_matches('/'))
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

fn request_headers<T>(
    resolved: &ResolvedAuth,
    request: &DeepSeekWebRequest<T>,
) -> Result<HeaderMap, BoxError> {
    let mut headers = HeaderMap::new();
    let mut sensitive_names = Vec::new();
    match resolved {
        ResolvedAuth::Account { token, .. } => {
            insert_header(&mut headers, "x-dsh-auth-token", token, true)?;
            sensitive_names.push("x-dsh-auth-token".to_owned());
        }
        ResolvedAuth::ApiKey { key, .. } => {
            if let Some(extra) = &request.api_key_headers {
                for (name, value) in extra(key) {
                    insert_header(&mut headers, &name, &value, true)?;
                    sensitive_names.push(name);
                }
            }
            insert_header(
                &mut headers,
                "authorization",
                &format!("Bearer {key}"),
                true,
            )?;
            sensitive_names.push("authorization".to_owned());
        }
    }
    for (name, value) in &request.headers {
        insert_header(&mut headers, name, value, false)?;
    }
    for (name, value) in [
        ("content-type", "application/json"),
        ("accept", "application/json"),
        ("user-agent", USER_AGENT),
    ] {
        insert_header(&mut headers, name, value, false)?;
    }
    for name in sensitive_names {
        if let Some(value) = headers.get_mut(&name) {
            value.set_sensitive(true);
        }
    }
    Ok(headers)
}

fn code(error: &BoxError) -> Option<&str> {
    HarnessError::of(error.as_ref()).map(HarnessError::code)
}

fn js_error_string(error: &BoxError) -> String {
    HarnessError::of(error.as_ref()).map_or_else(
        || format!("Error: {error}"),
        |error| format!("{}: {}", error.name(), error.message()),
    )
}

fn parse_response_json(bytes: &[u8]) -> Result<Value, BoxError> {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    serde_json::from_slice(bytes).map_err(|error| Box::new(error) as BoxError)
}

#[allow(dead_code)]
fn _future_shape<'a, T: 'a>(future: WebFuture<'a, T>) -> WebFuture<'a, T> {
    future
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoh_credentials::credential_ref;
    use hoh_timeout::{AbortController, AbortError};
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    fn guidance() -> DeepSeekWebGuidance {
        DeepSeekWebGuidance {
            subject: "Search",
            plugin: "web-deepseek-api",
            custom: true,
            base_url_field: "searchBaseURL".into(),
            api_key_env_field: "searchApiKeyEnv".into(),
        }
    }
    fn request() -> DeepSeekWebRequest<Value> {
        DeepSeekWebRequest {
            client: Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            path: "/search".into(),
            body: serde_json::json!({"query":"q"}),
            headers: Default::default(),
            api_key_headers: None,
            decode: Arc::new(Ok),
            guidance: guidance(),
            record_request: None,
            wait_to_send: None,
        }
    }
    fn literal(value: &str) -> DeepSeekWebAuthSource {
        DeepSeekWebAuthSource::ApiKey {
            base_url: "https://api.test/v1/tools".into(),
            key: DeepSeekWebApiKey::Literal(value.into()),
        }
    }
    fn account(value: Option<&str>) -> DeepSeekWebAuthSource {
        let value = value.map(str::to_owned);
        DeepSeekWebAuthSource::Account {
            base_url: "https://account.test/v1/tools".into(),
            resolve_token: Arc::new(move |_| {
                let value = value.clone();
                Box::pin(async move { Ok(value) })
            }),
            reject_token: Arc::new(|_| Box::pin(async { Ok(()) })),
        }
    }
    fn referenced(configured: bool, value: Option<&str>) -> DeepSeekWebAuthSource {
        let value = value.map(str::to_owned);
        DeepSeekWebAuthSource::ApiKey {
            base_url: "https://api.test/v1/tools".into(),
            key: DeepSeekWebApiKey::Referenced {
                reference: credential_ref("TEST_KEY").unwrap(),
                configured: Arc::new(move || Box::pin(async move { Ok(configured) })),
                resolve: Arc::new(move || {
                    let value = value.clone();
                    Box::pin(async move { Ok(value) })
                }),
            },
        }
    }
    fn harness(error: &BoxError) -> &HarnessError {
        HarnessError::of(error.as_ref()).expect("HarnessError")
    }

    async fn server(response: &'static str) -> (String, oneshot::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback fixture bind");
        let address = listener.local_addr().unwrap();
        let (sent, received) = oneshot::channel();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0_u8; 1024];
            loop {
                let count = stream.read(&mut chunk).await.unwrap();
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..count]);
                let Some(header_end) = bytes
                    .windows(4)
                    .position(|part| part == b"\r\n\r\n")
                    .map(|index| index + 4)
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&bytes[..header_end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if bytes.len() >= header_end + length {
                    break;
                }
            }
            let _ = sent.send(String::from_utf8(bytes).unwrap());
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        (format!("http://{address}"), received)
    }

    #[test]
    fn request_headers_follow_object_spread_priority_without_duplicates() {
        let mut web_request = request();
        web_request.api_key_headers = Some(Arc::new(|_| {
            BTreeMap::from([
                ("authorization".into(), "from-api-key-headers".into()),
                ("content-type".into(), "from-api-key-headers".into()),
                ("x-extra".into(), "extra".into()),
            ])
        }));
        web_request
            .headers
            .insert("authorization".into(), "from-request".into());
        web_request
            .headers
            .insert("content-type".into(), "from-request".into());
        web_request
            .headers
            .insert("user-agent".into(), "from-request".into());
        let resolved = ResolvedAuth::ApiKey {
            endpoint: "https://api.test/search".into(),
            key: "secret".into(),
        };

        let headers = request_headers(&resolved, &web_request).unwrap();

        assert_eq!(headers["authorization"], "from-request");
        assert_eq!(headers["content-type"], "application/json");
        assert_eq!(headers["user-agent"], USER_AGENT);
        assert_eq!(headers["x-extra"], "extra");
        assert_eq!(headers.get_all("authorization").iter().count(), 1);
        assert_eq!(headers.get_all("content-type").iter().count(), 1);
        assert!(headers["authorization"].is_sensitive());
        assert!(headers["x-extra"].is_sensitive());

        let account_headers = request_headers(
            &ResolvedAuth::Account {
                endpoint: "https://account.test/search".into(),
                token: "account-token".into(),
                reject: Arc::new(|_| Box::pin(async { Ok(()) })),
            },
            &request(),
        )
        .unwrap();
        assert!(account_headers["x-dsh-auth-token"].is_sensitive());
    }

    #[test]
    fn detects_sources_without_reading_them() {
        assert!(!has_deep_seek_web_credential(&DeepSeekWebAuth::default()));
        assert!(!has_deep_seek_web_credential(&DeepSeekWebAuth {
            sources: vec![literal("")],
            missing_credential_message: None
        }));
        assert!(has_deep_seek_web_credential(&DeepSeekWebAuth {
            sources: vec![literal("key")],
            missing_credential_message: None
        }));
        assert!(has_deep_seek_web_credential(&DeepSeekWebAuth {
            sources: vec![account(None)],
            missing_credential_message: None
        }));
        assert!(has_deep_seek_web_credential(&DeepSeekWebAuth {
            sources: vec![DeepSeekWebAuthSource::Misconfigured {
                message: "bad".into()
            }],
            missing_credential_message: None
        }));
    }

    #[tokio::test]
    async fn readiness_obeys_source_order() {
        assert!(
            is_deep_seek_web_auth_ready(
                &DeepSeekWebAuth {
                    sources: vec![referenced(true, None), account(None)],
                    missing_credential_message: None
                },
                "/search"
            )
            .await
        );
        assert!(
            is_deep_seek_web_auth_ready(
                &DeepSeekWebAuth {
                    sources: vec![referenced(false, None), account(Some("token"))],
                    missing_credential_message: None
                },
                "/fetch"
            )
            .await
        );
        assert!(
            !is_deep_seek_web_auth_ready(
                &DeepSeekWebAuth {
                    sources: vec![referenced(false, None), account(None)],
                    missing_credential_message: None
                },
                "/search"
            )
            .await
        );
    }

    #[tokio::test]
    async fn readiness_stops_at_misconfiguration() {
        let called = Arc::new(AtomicUsize::new(0));
        let later = called.clone();
        let auth = DeepSeekWebAuth {
            sources: vec![
                DeepSeekWebAuthSource::Misconfigured {
                    message: "bad route".into(),
                },
                DeepSeekWebAuthSource::Account {
                    base_url: "https://later.test".into(),
                    resolve_token: Arc::new(move |_| {
                        later.fetch_add(1, Ordering::SeqCst);
                        Box::pin(async { Ok(Some("token".into())) })
                    }),
                    reject_token: Arc::new(|_| Box::pin(async { Ok(()) })),
                },
            ],
            missing_credential_message: None,
        };
        assert!(!is_deep_seek_web_auth_ready(&auth, "/search").await);
        assert_eq!(called.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn readiness_uses_fetch_endpoint_and_stops_on_read_error() {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let capture = asked.clone();
        let auth = DeepSeekWebAuth {
            sources: vec![
                referenced(false, None),
                DeepSeekWebAuthSource::Account {
                    base_url: "https://account.test/v1/tools".into(),
                    resolve_token: Arc::new(move |endpoint| {
                        capture.lock().unwrap().push(endpoint);
                        Box::pin(async { Ok(Some("token".into())) })
                    }),
                    reject_token: Arc::new(|_| Box::pin(async { Ok(()) })),
                },
            ],
            missing_credential_message: None,
        };
        assert!(is_deep_seek_web_auth_ready(&auth, "/fetch").await);
        assert_eq!(
            *asked.lock().unwrap(),
            vec!["https://account.test/v1/tools/fetch"]
        );
    }

    #[tokio::test]
    async fn account_precedes_key_without_reading_key() {
        let read = Arc::new(AtomicUsize::new(0));
        let key_read = read.clone();
        let source = DeepSeekWebAuthSource::ApiKey {
            base_url: "https://api.test".into(),
            key: DeepSeekWebApiKey::Referenced {
                reference: credential_ref("KEY").unwrap(),
                configured: Arc::new(|| Box::pin(async { Ok(true) })),
                resolve: Arc::new(move || {
                    key_read.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async { Ok(Some("key".into())) })
                }),
            },
        };
        let resolved = auth_headers(
            &DeepSeekWebAuth {
                sources: vec![account(Some("token")), source],
                missing_credential_message: None,
            },
            &request(),
            None,
        )
        .await
        .unwrap();
        assert!(matches!(resolved, ResolvedAuth::Account { .. }));
        assert_eq!(read.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn key_precedes_account_without_reading_account() {
        let read = Arc::new(AtomicUsize::new(0));
        let account_read = read.clone();
        let source = DeepSeekWebAuthSource::Account {
            base_url: "https://account.test".into(),
            resolve_token: Arc::new(move |_| {
                account_read.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok(Some("token".into())) })
            }),
            reject_token: Arc::new(|_| Box::pin(async { Ok(()) })),
        };
        let resolved = auth_headers(
            &DeepSeekWebAuth {
                sources: vec![literal("key"), source],
                missing_credential_message: None,
            },
            &request(),
            None,
        )
        .await
        .unwrap();
        assert!(matches!(resolved, ResolvedAuth::ApiKey { .. }));
        assert_eq!(read.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn absent_first_source_falls_through() {
        let resolved = auth_headers(
            &DeepSeekWebAuth {
                sources: vec![account(None), literal("key")],
                missing_credential_message: None,
            },
            &request(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(resolved.endpoint(), "https://api.test/v1/tools/search");
    }

    #[tokio::test]
    async fn missing_credential_uses_caller_message() {
        let error = auth_headers(
            &DeepSeekWebAuth {
                sources: vec![],
                missing_credential_message: Some("no credential for this search".into()),
            },
            &request(),
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(harness(&error).code(), "WEB_PROVIDER_CREDENTIAL_MISSING");
        assert_eq!(harness(&error).message(), "no credential for this search");
    }

    #[tokio::test]
    async fn missing_credential_has_generic_message() {
        let error = auth_headers(&DeepSeekWebAuth::default(), &request(), None)
            .await
            .unwrap_err();
        assert_eq!(
            harness(&error).message(),
            "DeepSeek web access has no credential to send"
        );
    }

    #[tokio::test]
    async fn reached_misconfiguration_is_terminal() {
        let error = auth_headers(
            &DeepSeekWebAuth {
                sources: vec![
                    account(None),
                    DeepSeekWebAuthSource::Misconfigured {
                        message: "route settings do not resolve".into(),
                    },
                    literal("key"),
                ],
                missing_credential_message: None,
            },
            &request(),
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(harness(&error).code(), "WEB_PROVIDER_MISCONFIGURED");
    }

    #[tokio::test]
    async fn present_source_wins_before_later_misconfiguration() {
        let resolved = auth_headers(
            &DeepSeekWebAuth {
                sources: vec![
                    account(Some("token")),
                    DeepSeekWebAuthSource::Misconfigured {
                        message: "must not be reached".into(),
                    },
                ],
                missing_credential_message: None,
            },
            &request(),
            None,
        )
        .await
        .unwrap();
        assert!(matches!(resolved, ResolvedAuth::Account { .. }));
    }

    #[tokio::test]
    async fn credential_read_failure_is_provider_error() {
        let auth = DeepSeekWebAuth {
            sources: vec![DeepSeekWebAuthSource::Account {
                base_url: "https://account.test".into(),
                resolve_token: Arc::new(|_| {
                    Box::pin(async { Err("account storage failed".into()) })
                }),
                reject_token: Arc::new(|_| Box::pin(async { Ok(()) })),
            }],
            missing_credential_message: None,
        };
        let error = auth_headers(&auth, &request(), None).await.unwrap_err();
        assert_eq!(harness(&error).code(), "WEB_PROVIDER_ERROR");
        assert_eq!(
            harness(&error).message(),
            "DeepSeek web credential resolution failed: Error: account storage failed"
        );
    }

    #[tokio::test]
    async fn key_read_failure_is_retained_as_the_cause() {
        let auth = DeepSeekWebAuth {
            sources: vec![DeepSeekWebAuthSource::ApiKey {
                base_url: "https://api.test".into(),
                key: DeepSeekWebApiKey::Referenced {
                    reference: credential_ref("KEY").unwrap(),
                    configured: Arc::new(|| Box::pin(async { Ok(true) })),
                    resolve: Arc::new(|| {
                        Box::pin(async {
                            Err(WebError::new("malformed key", "WEB_PROVIDER_MISCONFIGURED").into())
                        })
                    }),
                },
            }],
            missing_credential_message: None,
        };
        let error = auth_headers(&auth, &request(), None).await.unwrap_err();
        assert_eq!(
            harness(&error).message(),
            "DeepSeek web credential resolution failed: WebError: malformed key"
        );
        let cause = harness(&error).cause().unwrap();
        assert_eq!(
            HarnessError::of(cause).unwrap().code(),
            "WEB_PROVIDER_MISCONFIGURED"
        );
    }

    #[tokio::test]
    async fn referenced_key_is_resolved_again_for_every_operation() {
        let reads = Arc::new(AtomicUsize::new(0));
        let count = reads.clone();
        let auth = DeepSeekWebAuth {
            sources: vec![DeepSeekWebAuthSource::ApiKey {
                base_url: "https://api.test".into(),
                key: DeepSeekWebApiKey::Referenced {
                    reference: credential_ref("KEY").unwrap(),
                    configured: Arc::new(|| Box::pin(async { Ok(true) })),
                    resolve: Arc::new(move || {
                        let value = count.fetch_add(1, Ordering::SeqCst) + 1;
                        Box::pin(async move { Ok(Some(format!("key-{value}"))) })
                    }),
                },
            }],
            missing_credential_message: None,
        };
        let first = auth_headers(&auth, &request(), None).await.unwrap();
        let second = auth_headers(&auth, &request(), None).await.unwrap();
        assert!(matches!(first, ResolvedAuth::ApiKey { key, .. } if key == "key-1"));
        assert!(matches!(second, ResolvedAuth::ApiKey { key, .. } if key == "key-2"));
        assert_eq!(reads.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn pre_aborted_call_starts_no_resolver() {
        let called = Arc::new(AtomicUsize::new(0));
        let resolver_called = called.clone();
        let auth = DeepSeekWebAuth {
            sources: vec![DeepSeekWebAuthSource::Account {
                base_url: "https://account.test".into(),
                resolve_token: Arc::new(move |_| {
                    resolver_called.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async { Ok(Some("token".into())) })
                }),
                reject_token: Arc::new(|_| Box::pin(async { Ok(()) })),
            }],
            missing_credential_message: None,
        };
        let controller = AbortController::new();
        controller.abort_with(AbortError::new("caller stopped"));
        let error = auth_headers(&auth, &request(), Some(&controller.signal()))
            .await
            .unwrap_err();
        assert_eq!(harness(&error).code(), "WEB_ABORTED");
        assert_eq!(called.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cancellation_triggered_inside_resolver_is_observed() {
        let controller = Arc::new(AbortController::new());
        let abort = controller.clone();
        let auth = DeepSeekWebAuth {
            sources: vec![DeepSeekWebAuthSource::Account {
                base_url: "https://account.test".into(),
                resolve_token: Arc::new(move |_| {
                    abort.abort_with(AbortError::new("resolver stopped caller"));
                    Box::pin(async { Ok(None) })
                }),
                reject_token: Arc::new(|_| Box::pin(async { Ok(()) })),
            }],
            missing_credential_message: None,
        };
        let error = auth_headers(&auth, &request(), Some(&controller.signal()))
            .await
            .unwrap_err();
        assert_eq!(harness(&error).code(), "WEB_ABORTED");
    }

    #[test]
    fn endpoint_guidance_names_custom_fields() {
        let error = endpoint_error(&guidance(), "https://api.test/search", "HTTP 404", None);
        assert_eq!(
            error.message(),
            "HTTP 404\n\nThe web request used endpoint \"https://api.test/search\". Search sends the API key under web-deepseek-api.searchApiKeyEnv to web-deepseek-api.searchBaseURL. If that endpoint is not intended, guide the user to correct web-deepseek-api.searchBaseURL and web-deepseek-api.searchApiKeyEnv, or to remove both so Search uses the DeepSeek credential of the conversation. Only the user should choose or change the endpoint."
        );
    }

    #[test]
    fn parses_error_detail_shapes() {
        assert_eq!(
            error_detail(&serde_json::json!({"error":"bad"})),
            Some("bad")
        );
        assert_eq!(
            error_detail(&serde_json::json!({"error":{"message":"nested"}})),
            Some("nested")
        );
        assert_eq!(
            error_detail(&serde_json::json!({"message":"top"})),
            Some("top")
        );
        assert_eq!(
            error_detail(&serde_json::json!({"error":{"code":1},"message":"top"})),
            None
        );
        for non_record in [
            serde_json::Value::Null,
            serde_json::json!(5),
            serde_json::json!([]),
            serde_json::json!(true),
        ] {
            assert_eq!(
                error_detail(&serde_json::json!({"error":non_record,"message":"top"})),
                Some("top")
            );
        }
        assert_eq!(
            error_detail(&serde_json::json!({"error":{"message":5},"message":"top"})),
            None
        );
        assert_eq!(error_detail(&serde_json::json!({})), None);
    }

    #[test]
    fn response_json_accepts_utf8_bom_for_success_and_error_bodies() {
        let bytes = b"\xef\xbb\xbf{\"error\":\"bad\"}";
        let parsed = parse_response_json(bytes).unwrap();
        assert_eq!(error_detail(&parsed), Some("bad"));
    }

    #[tokio::test]
    async fn injected_client_controls_dispatch_without_network() {
        let mut request = request();
        request.client = Client::builder()
            .no_proxy()
            .https_only(true)
            .build()
            .unwrap();
        let error = dispatch_deep_seek_web(
            &DeepSeekWebAuth {
                sources: vec![DeepSeekWebAuthSource::ApiKey {
                    base_url: "http://example.invalid/v1/tools".into(),
                    key: DeepSeekWebApiKey::Literal("key".into()),
                }],
                missing_credential_message: None,
            },
            &request,
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(harness(&error).code(), "WEB_PROVIDER_ERROR");
        assert!(
            harness(&error)
                .message()
                .contains("http://example.invalid/v1/tools/search")
        );
        let cause = harness(&error).cause().expect("reqwest cause");
        let cause = cause
            .downcast_ref::<reqwest::Error>()
            .expect("dispatch must use the injected reqwest client");
        assert!(cause.is_builder(), "unexpected cause: {cause}");
    }

    #[tokio::test]
    async fn aborted_credential_resolution_continues_observing_side_effects() {
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let (completed_tx, completed_rx) = oneshot::channel();
        let started_tx = Arc::new(Mutex::new(Some(started_tx)));
        let release_rx = Arc::new(Mutex::new(Some(release_rx)));
        let completed_tx = Arc::new(Mutex::new(Some(completed_tx)));
        let auth = DeepSeekWebAuth {
            sources: vec![DeepSeekWebAuthSource::Account {
                base_url: "https://account.test".into(),
                resolve_token: Arc::new(move |_| {
                    let started = started_tx.lock().unwrap().take();
                    let release = release_rx.lock().unwrap().take();
                    let completed = completed_tx.lock().unwrap().take();
                    Box::pin(async move {
                        if let Some(started) = started {
                            let _ = started.send(());
                        }
                        if let Some(release) = release {
                            let _ = release.await;
                        }
                        if let Some(completed) = completed {
                            let _ = completed.send(());
                        }
                        Ok(None)
                    })
                }),
                reject_token: Arc::new(|_| Box::pin(async { Ok(()) })),
            }],
            missing_credential_message: None,
        };
        let controller = AbortController::new();
        let signal = controller.signal();
        let running =
            tokio::spawn(async move { auth_headers(&auth, &request(), Some(&signal)).await });
        started_rx.await.unwrap();
        controller.abort_with(AbortError::new("caller stopped"));
        let error = running.await.unwrap().unwrap_err();
        assert_eq!(harness(&error).code(), "WEB_ABORTED");
        release_tx.send(()).unwrap();
        completed_rx.await.unwrap();
    }

    #[tokio::test]
    async fn empty_sources_fall_through_and_readiness_errors_stop_the_chain() {
        let resolved = auth_headers(
            &DeepSeekWebAuth {
                sources: vec![literal(""), account(None), literal("key")],
                missing_credential_message: None,
            },
            &request(),
            None,
        )
        .await
        .unwrap();
        assert!(matches!(resolved, ResolvedAuth::ApiKey { .. }));

        let later = Arc::new(AtomicUsize::new(0));
        let called = later.clone();
        let unreadable = DeepSeekWebAuthSource::ApiKey {
            base_url: "https://api.test".into(),
            key: DeepSeekWebApiKey::Referenced {
                reference: credential_ref("KEY").unwrap(),
                configured: Arc::new(|| {
                    Box::pin(async { Err(std::io::Error::other("store locked").into()) })
                }),
                resolve: Arc::new(|| Box::pin(async { Ok(Some("key".into())) })),
            },
        };
        let later_source = DeepSeekWebAuthSource::Account {
            base_url: "https://account.test".into(),
            resolve_token: Arc::new(move |_| {
                called.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok(Some("token".into())) })
            }),
            reject_token: Arc::new(|_| Box::pin(async { Ok(()) })),
        };
        assert!(
            !is_deep_seek_web_auth_ready(
                &DeepSeekWebAuth {
                    sources: vec![unreadable, later_source],
                    missing_credential_message: None,
                },
                "/fetch",
            )
            .await
        );
        assert_eq!(later.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn empty_referenced_key_falls_through_and_account_read_error_stops_readiness() {
        let empty_reference = DeepSeekWebAuthSource::ApiKey {
            base_url: "https://empty.test".into(),
            key: DeepSeekWebApiKey::Referenced {
                reference: credential_ref("EMPTY_KEY").unwrap(),
                configured: Arc::new(|| Box::pin(async { Ok(true) })),
                resolve: Arc::new(|| Box::pin(async { Ok(Some(String::new())) })),
            },
        };
        let resolved = auth_headers(
            &DeepSeekWebAuth {
                sources: vec![empty_reference, literal("fallback-key")],
                missing_credential_message: None,
            },
            &request(),
            None,
        )
        .await
        .unwrap();
        assert!(
            matches!(resolved, ResolvedAuth::ApiKey { endpoint, .. } if endpoint == "https://api.test/v1/tools/search")
        );

        let later_calls = Arc::new(AtomicUsize::new(0));
        let later = later_calls.clone();
        let auth = DeepSeekWebAuth {
            sources: vec![
                DeepSeekWebAuthSource::Account {
                    base_url: "https://account.test".into(),
                    resolve_token: Arc::new(|_| {
                        Box::pin(async {
                            Err(std::io::Error::other("account store locked").into())
                        })
                    }),
                    reject_token: Arc::new(|_| Box::pin(async { Ok(()) })),
                },
                DeepSeekWebAuthSource::ApiKey {
                    base_url: "https://later.test".into(),
                    key: DeepSeekWebApiKey::Referenced {
                        reference: credential_ref("LATER_KEY").unwrap(),
                        configured: Arc::new(move || {
                            later.fetch_add(1, Ordering::SeqCst);
                            Box::pin(async { Ok(true) })
                        }),
                        resolve: Arc::new(|| Box::pin(async { Ok(Some("key".into())) })),
                    },
                },
            ],
            missing_credential_message: None,
        };
        assert!(!is_deep_seek_web_auth_ready(&auth, "/search").await);
        assert_eq!(later_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn joins_path_without_double_slash() {
        assert_eq!(
            join_path("https://api.test/v1/tools///", "/search"),
            "https://api.test/v1/tools/search"
        );
    }

    #[tokio::test]
    #[ignore = "requires loopback bind, which the writer sandbox forbids"]
    async fn dispatch_posts_json_with_bearer_and_protocol_headers() {
        let (base, received) = server("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\nconnection: close\r\n\r\n{\"ok\":true}").await;
        let result = dispatch_deep_seek_web(
            &DeepSeekWebAuth {
                sources: vec![DeepSeekWebAuthSource::ApiKey {
                    base_url: base,
                    key: DeepSeekWebApiKey::Literal("ds-key".into()),
                }],
                missing_credential_message: None,
            },
            &request(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(result, serde_json::json!({"ok":true}));
        let wire = received.await.unwrap().to_ascii_lowercase();
        assert!(wire.starts_with("post /search http/1.1\r\n"));
        assert!(wire.contains("authorization: bearer ds-key\r\n"));
        assert!(wire.contains("content-type: application/json\r\n"));
        assert!(wire.ends_with("{\"query\":\"q\"}"));
    }

    #[tokio::test]
    #[ignore = "requires loopback bind, which the writer sandbox forbids"]
    async fn account_401_invalidates_exact_sent_token() {
        let (base, _received) = server("HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\ncontent-length: 25\r\nconnection: close\r\n\r\n{\"error\":\"invalid token\"}").await;
        let rejected = Arc::new(Mutex::new(Vec::new()));
        let capture = rejected.clone();
        let auth = DeepSeekWebAuth {
            sources: vec![DeepSeekWebAuthSource::Account {
                base_url: base,
                resolve_token: Arc::new(|_| Box::pin(async { Ok(Some("sent-token".into())) })),
                reject_token: Arc::new(move |token| {
                    let capture = capture.clone();
                    Box::pin(async move {
                        capture.lock().unwrap().push(token);
                        Ok(())
                    })
                }),
            }],
            missing_credential_message: None,
        };
        let error = dispatch_deep_seek_web(&auth, &request(), None)
            .await
            .unwrap_err();
        assert_eq!(harness(&error).code(), "WEB_PROVIDER_ERROR");
        assert!(
            harness(&error)
                .message()
                .contains("sign in to DeepSeek again")
        );
        assert_eq!(*rejected.lock().unwrap(), vec!["sent-token"]);
    }

    #[tokio::test]
    #[ignore = "requires loopback bind, which the writer sandbox forbids"]
    async fn redirect_response_is_rejected_without_following() {
        let (base, _received) = server("HTTP/1.1 307 Temporary Redirect\r\nlocation: http://127.0.0.1:9/collect\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
        let error = dispatch_deep_seek_web(
            &DeepSeekWebAuth {
                sources: vec![DeepSeekWebAuthSource::ApiKey {
                    base_url: base,
                    key: DeepSeekWebApiKey::Literal("key".into()),
                }],
                missing_credential_message: None,
            },
            &request(),
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(harness(&error).code(), "WEB_PROVIDER_ERROR");
        assert!(harness(&error).message().contains("HTTP 307"));
    }
}
