use crate::transport::{
    DeepSeekWebAuth, DeepSeekWebGuidance, DeepSeekWebRequest, dispatch_deep_seek_web,
    has_deep_seek_web_credential, invalid_response, is_deep_seek_web_auth_ready, search_aborted,
};
use hoh_error::BoxError;
use hoh_package_manifest::LocalizedText;
use hoh_timeout::AbortSignal;
use hoh_web::{
    WebFuture, WebProviderInfo, WebSearchProvider, WebSearchRequest, WebSearchResult,
    WebSearchSource,
};
use indexmap::IndexMap;
use reqwest::Client;
use serde_json::{Map, Value};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use tokio::time::Instant;
use url::Url;

pub const DEEPSEEK_SEARCH_API_PROVIDER_ID: &str = "deepseek-search-api";
pub const DEEPSEEK_WEB_API_BASE_URL: &str = "https://api.deepseek.com/v1/tools";
pub const DEEPSEEK_DEFAULT_COUNT: u64 = 10;
pub const DEEPSEEK_SEARCH_REQUESTS_PER_SECOND: u64 = 5;
const ORIGINAL_QUESTION_MAX_CHARS: usize = 128;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub struct DeepSeekSearchApiOptions {
    pub client: Client,
    pub auth: DeepSeekWebAuth,
    pub guidance: DeepSeekWebGuidance,
    pub count: u64,
    pub requests_per_second: u64,
    pub original_question: Option<Arc<dyn Fn() -> Option<String> + Send + Sync>>,
}

type OptionsResolver = dyn Fn() -> Result<DeepSeekSearchApiOptions, BoxError> + Send + Sync;
type ReadyAuthResolver =
    dyn for<'a> Fn(Option<&'a str>) -> Result<DeepSeekWebAuth, BoxError> + Send + Sync;

pub struct DeepSeekSearchApiProvider {
    resolve_options: Arc<OptionsResolver>,
    resolve_ready_auth: Arc<ReadyAuthResolver>,
    starts: Arc<Mutex<VecDeque<Instant>>>,
}

impl DeepSeekSearchApiProvider {
    pub fn new(
        resolve_options: impl Fn() -> DeepSeekSearchApiOptions + Send + Sync + 'static,
    ) -> Self {
        let resolve_options = Arc::new(resolve_options);
        let ready_options = resolve_options.clone();
        Self {
            resolve_options: Arc::new(move || Ok(resolve_options())),
            resolve_ready_auth: Arc::new(move |_| Ok((ready_options)().auth)),
            starts: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    pub fn with_ready_auth(
        resolve_options: impl Fn() -> Result<DeepSeekSearchApiOptions, BoxError> + Send + Sync + 'static,
        resolve_ready_auth: impl Fn(Option<&str>) -> Result<DeepSeekWebAuth, BoxError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            resolve_options: Arc::new(resolve_options),
            resolve_ready_auth: Arc::new(resolve_ready_auth),
            starts: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    fn options(&self) -> Result<DeepSeekSearchApiOptions, BoxError> {
        (self.resolve_options)()
    }

    async fn wait_for_request_slot(
        starts: Arc<Mutex<VecDeque<Instant>>>,
        limit: u64,
        signal: Option<AbortSignal>,
    ) -> Result<(), BoxError> {
        loop {
            if signal.as_ref().is_some_and(AbortSignal::aborted) {
                return Err(search_aborted(
                    signal
                        .as_ref()
                        .and_then(AbortSignal::reason)
                        .map(|reason| Box::new(reason) as BoxError),
                )
                .into());
            }
            let wait = {
                let now = Instant::now();
                let mut starts = lock(&starts);
                while starts
                    .front()
                    .is_some_and(|start| *start < now - Duration::from_secs(1))
                {
                    starts.pop_front();
                }
                if starts.len() < limit as usize {
                    starts.push_back(now);
                    return Ok(());
                }
                starts.front().copied().expect("rate-limited queue") + Duration::from_millis(1001)
                    - now
            };
            match signal.as_ref() {
                None => tokio::time::sleep(wait).await,
                Some(signal) => tokio::select! {
                    biased;
                    reason = signal.cancelled() => return Err(search_aborted(Some(Box::new(reason))).into()),
                    () = tokio::time::sleep(wait) => {}
                },
            }
        }
    }
}

impl WebSearchProvider for DeepSeekSearchApiProvider {
    fn info(&self) -> WebProviderInfo {
        WebProviderInfo {
            id: DEEPSEEK_SEARCH_API_PROVIDER_ID.into(),
            title: None,
            description: Some(LocalizedText::Translations(IndexMap::from([
                ("en".into(), "Incurs charges, paid by the account or API key chosen under Billing.".into()),
                ("zh".into(), "会产生费用，由「计费方式」选定的账号或 API Key 支付。".into()),
                ("ja".into(), "料金が発生し、「課金方法」で選んだアカウントまたは API キーに請求されます。".into()),
                ("es".into(), "Genera cargos, que paga la cuenta o la clave de API elegida en Facturación.".into()),
                ("fr".into(), "Entraîne des frais, payés par le compte ou la clé API choisi dans Facturation.".into()),
                ("de".into(), "Verursacht Kosten, die das unter Abrechnung gewählte Konto oder der API-Schlüssel trägt.".into()),
                ("ru".into(), "Услуга платная: оплата идёт с аккаунта или ключа API, выбранного в разделе «Оплата».".into()),
                ("pt".into(), "Gera cobranças, pagas pela conta ou chave de API escolhida em Cobrança.".into()),
            ]))),
        }
    }

    fn available(&self) -> Result<bool, BoxError> {
        let options = self.options()?;
        Ok(has_deep_seek_web_credential(&options.auth)
            && options.auth.sources.iter().all(|source| match source {
                crate::DeepSeekWebAuthSource::Misconfigured { .. } => true,
                crate::DeepSeekWebAuthSource::Account { base_url, .. }
                | crate::DeepSeekWebAuthSource::ApiKey { base_url, .. } => {
                    Url::parse(base_url).is_ok()
                }
            })
            && options.count > 0
            && options.requests_per_second > 0)
    }

    fn ready<'a>(&'a self, llm_provider: Option<&'a str>) -> WebFuture<'a, bool> {
        let auth = (self.resolve_ready_auth)(llm_provider);
        Box::pin(async move {
            let auth = auth?;
            Ok(is_deep_seek_web_auth_ready(&auth, "/search").await)
        })
    }

    fn search<'a>(
        &'a self,
        request: WebSearchRequest,
        signal: Option<&'a AbortSignal>,
    ) -> WebFuture<'a, WebSearchResult> {
        let options = self.options();
        let starts = self.starts.clone();
        Box::pin(async move {
            let options = options?;
            let question = options
                .original_question
                .as_ref()
                .and_then(|read| read())
                .and_then(|value| bounded_question(&value));
            let body = build_search_body(request, options.count, question);
            let limit = options.requests_per_second;
            let request = DeepSeekWebRequest {
                client: options.client,
                path: "/search".into(),
                body,
                headers: Default::default(),
                api_key_headers: None,
                decode: Arc::new(map_search_response),
                guidance: options.guidance,
                record_request: None,
                wait_to_send: Some(Arc::new(move |signal| {
                    Box::pin(Self::wait_for_request_slot(starts.clone(), limit, signal))
                })),
            };
            dispatch_deep_seek_web(&options.auth, &request, signal).await
        })
    }
}

fn build_search_body(
    request: WebSearchRequest,
    configured_count: u64,
    original_question: Option<String>,
) -> Value {
    let mut body = Map::from_iter([
        ("query".into(), Value::String(request.query)),
        (
            "count".into(),
            Value::from(
                request
                    .max_results
                    .unwrap_or(configured_count)
                    .min(configured_count),
            ),
        ),
        ("need_content".into(), Value::Bool(false)),
    ]);
    if let Some(question) = original_question {
        body.insert("ori_question".into(), Value::String(question));
    }
    Value::Object(body)
}

pub fn map_search_response(response: Value) -> Result<WebSearchResult, BoxError> {
    let results = response
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| Box::new(invalid_response()) as BoxError)?;
    let mut sources = Vec::with_capacity(results.len());
    for item in results {
        let item = item
            .as_object()
            .ok_or_else(|| Box::new(invalid_response()) as BoxError)?;
        let url = required_string(item, "url")?;
        if url.is_empty() {
            return Err(invalid_response().into());
        }
        let title = required_string(item, "title")?;
        let snippet = required_string(item, "snippet")?;
        let summary = optional_string(item, "summary")?;
        let date = nullable_string(item, "date")?;
        sources.push(WebSearchSource {
            url,
            title: non_empty(title),
            snippet: non_empty(snippet),
            summary: summary.and_then(non_empty),
            published_at: date.and_then(non_empty),
        });
    }
    Ok(WebSearchResult {
        content: None,
        sources,
        truncated: false,
    })
}

fn required_string(object: &Map<String, Value>, key: &str) -> Result<String, BoxError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| invalid_response().into())
}
fn optional_string(object: &Map<String, Value>, key: &str) -> Result<Option<String>, BoxError> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(invalid_response().into()),
    }
}
fn nullable_string(object: &Map<String, Value>, key: &str) -> Result<Option<String>, BoxError> {
    match object.get(key) {
        Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        _ => Err(invalid_response().into()),
    }
}
fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}
fn bounded_question(question: &str) -> Option<String> {
    let collapsed = question
        .split(hoh_util_values::is_js_white_space)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (!collapsed.is_empty()).then(|| {
        collapsed
            .chars()
            .take(ORIGINAL_QUESTION_MAX_CHARS)
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoh_timeout::{AbortController, AbortError};

    fn guidance() -> DeepSeekWebGuidance {
        DeepSeekWebGuidance {
            subject: "Search",
            plugin: "web-deepseek-api",
            custom: true,
            base_url_field: "searchBaseURL".into(),
            api_key_env_field: "searchApiKeyEnv".into(),
        }
    }
    fn provider(count: u64, rate: u64, base: &str, key: &str) -> DeepSeekSearchApiProvider {
        let base = base.to_owned();
        let key = key.to_owned();
        DeepSeekSearchApiProvider::new(move || DeepSeekSearchApiOptions {
            client: Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            auth: DeepSeekWebAuth {
                sources: vec![crate::DeepSeekWebAuthSource::ApiKey {
                    base_url: base.clone(),
                    key: crate::DeepSeekWebApiKey::Literal(key.clone()),
                }],
                missing_credential_message: None,
            },
            guidance: guidance(),
            count,
            requests_per_second: rate,
            original_question: None,
        })
    }

    #[test]
    fn maps_source_metadata_and_omits_empty_values() {
        let result = map_search_response(serde_json::json!({"results":[
            {"url":"https://a.test","title":"A","snippet":"excerpt","summary":"summary","date":"2026-02-02"},
            {"url":"https://b.test","title":"B","snippet":"","summary":"","date":null}
        ]})).unwrap();
        assert_eq!(result.sources[0].summary.as_deref(), Some("summary"));
        assert_eq!(result.sources[1].snippet, None);
        assert_eq!(result.sources[1].published_at, None);
    }

    #[test]
    fn accepts_empty_results_and_rejects_malformed_json() {
        assert!(
            map_search_response(serde_json::json!({"results":[]}))
                .unwrap()
                .sources
                .is_empty()
        );
        for value in [
            Value::Null,
            serde_json::json!({}),
            serde_json::json!({"results":{}}),
            serde_json::json!({"results":[null]}),
        ] {
            assert_eq!(
                hoh_error::HarnessError::of(map_search_response(value).unwrap_err().as_ref())
                    .unwrap()
                    .code(),
                "WEB_PROVIDER_ERROR"
            );
        }
    }

    #[test]
    fn bounds_original_question_by_unicode_scalars() {
        assert_eq!(
            bounded_question("  How do\n\tI  install it?  ").as_deref(),
            Some("How do I install it?")
        );
        assert_eq!(
            bounded_question("a\u{feff}\u{feff}b").as_deref(),
            Some("a b")
        );
        let input = format!("{}😀😀", "a".repeat(127));
        assert_eq!(
            bounded_question(&input).unwrap(),
            format!("{}😀", "a".repeat(127))
        );
    }

    #[test]
    fn availability_requires_a_credential() {
        assert!(
            provider(10, 5, "https://api.test", "key")
                .available()
                .unwrap()
        );
        assert!(!provider(10, 5, "https://api.test", "").available().unwrap());
    }
    #[test]
    fn availability_requires_parseable_base() {
        assert!(!provider(10, 5, "not a url", "key").available().unwrap());
    }
    #[test]
    fn availability_rejects_invalid_count() {
        assert!(
            !provider(0, 5, "https://api.test", "key")
                .available()
                .unwrap()
        );
        assert!(
            provider(51, 5, "https://api.test", "key")
                .available()
                .unwrap()
        );
    }
    #[test]
    fn availability_rejects_zero_rate() {
        assert!(
            !provider(10, 0, "https://api.test", "key")
                .available()
                .unwrap()
        );
    }
    #[tokio::test]
    async fn ready_checks_credential_presence() {
        assert!(
            provider(10, 5, "https://api.test", "key")
                .ready(None)
                .await
                .unwrap()
        );
        assert!(
            !provider(10, 5, "https://api.test", "")
                .ready(None)
                .await
                .unwrap()
        );
    }
    #[tokio::test]
    async fn ready_passes_the_model_route_and_preserves_target_guidance_errors() {
        let routes = Arc::new(Mutex::new(Vec::new()));
        let captured = routes.clone();
        let provider = DeepSeekSearchApiProvider::with_ready_auth(
            || unreachable!("ready must not resolve operation options"),
            move |route| {
                captured.lock().unwrap().push(route.map(str::to_owned));
                Err(hoh_web::WebError::new(
                    "correct the model route or web-deepseek-api.searchBaseURL",
                    "WEB_PROVIDER_MISCONFIGURED",
                )
                .into())
            },
        );
        let error = provider.ready(Some("moonshotai")).await.unwrap_err();
        let error = hoh_error::HarnessError::of(error.as_ref()).unwrap();
        assert_eq!(error.code(), "WEB_PROVIDER_MISCONFIGURED");
        assert_eq!(
            error.message(),
            "correct the model route or web-deepseek-api.searchBaseURL"
        );
        assert_eq!(
            routes.lock().unwrap().as_slice(),
            [Some("moonshotai".to_owned())]
        );
    }
    #[test]
    fn malformed_result_fields_are_rejected() {
        for value in [
            serde_json::json!({"results":[{"url":"","title":"","snippet":"","date":null}]}),
            serde_json::json!({"results":[{"url":"https://a","title":1,"snippet":"","date":null}]}),
            serde_json::json!({"results":[{"url":"https://a","title":"","snippet":1,"date":null}]}),
            serde_json::json!({"results":[{"url":"https://a","title":"","snippet":"","date":1}]}),
            serde_json::json!({"results":[{"url":"https://a","title":"","snippet":"","summary":1,"date":null}]}),
        ] {
            assert!(map_search_response(value).is_err());
        }
    }

    #[test]
    fn request_body_uses_minimum_count_and_optional_original_question() {
        let request = WebSearchRequest {
            query: "rust".into(),
            max_results: Some(50),
        };
        assert_eq!(
            build_search_body(request, 7, Some("How do I install it?".into())),
            serde_json::json!({
                "query":"rust",
                "count":7,
                "need_content":false,
                "ori_question":"How do I install it?"
            })
        );
        assert_eq!(
            build_search_body(
                WebSearchRequest {
                    query: "rust".into(),
                    max_results: Some(3),
                },
                7,
                None,
            ),
            serde_json::json!({"query":"rust","count":3,"need_content":false})
        );
    }

    #[tokio::test(start_paused = true)]
    async fn pacing_opens_after_1001ms_and_observes_cancellation() {
        let starts = Arc::new(Mutex::new(VecDeque::new()));
        DeepSeekSearchApiProvider::wait_for_request_slot(starts.clone(), 1, None)
            .await
            .unwrap();
        let waiting_starts = starts.clone();
        let waiting = tokio::spawn(async move {
            DeepSeekSearchApiProvider::wait_for_request_slot(waiting_starts, 1, None).await
        });
        tokio::time::advance(Duration::from_millis(1000)).await;
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        tokio::time::advance(Duration::from_millis(1)).await;
        waiting.await.unwrap().unwrap();

        let controller = AbortController::new();
        let signal = controller.signal();
        let cancelled_starts = starts.clone();
        let cancelled = tokio::spawn(async move {
            DeepSeekSearchApiProvider::wait_for_request_slot(cancelled_starts, 1, Some(signal))
                .await
        });
        tokio::task::yield_now().await;
        controller.abort_with(AbortError::new("caller stopped"));
        let error = cancelled.await.unwrap().unwrap_err();
        assert_eq!(
            hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
            "WEB_ABORTED"
        );
    }

    #[tokio::test]
    async fn pre_aborted_search_dispatches_nothing() {
        let controller = AbortController::new();
        controller.abort_with(AbortError::new("caller stopped"));
        let error = provider(10, 5, "http://example.invalid", "key")
            .search(
                WebSearchRequest {
                    query: "q".into(),
                    max_results: None,
                },
                Some(&controller.signal()),
            )
            .await
            .unwrap_err();
        assert_eq!(
            hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
            "WEB_ABORTED"
        );
    }
}
