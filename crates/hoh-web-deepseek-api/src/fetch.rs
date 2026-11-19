use crate::transport::{
    DeepSeekWebAuth, DeepSeekWebGuidance, DeepSeekWebRequest, dispatch_deep_seek_web,
    has_deep_seek_web_credential, invalid_response, is_deep_seek_web_auth_ready,
};
use hoh_error::BoxError;
use hoh_package_manifest::LocalizedText;
use hoh_timeout::AbortSignal;
use hoh_web::{
    WebFetchBody, WebFetchProvider, WebFetchRequest, WebFetchResult, WebFuture, WebProviderInfo,
};
use indexmap::IndexMap;
use reqwest::Client;
use serde_json::{Value, json};
use std::sync::Arc;
use url::Url;

pub const DEEPSEEK_FETCH_API_PROVIDER_ID: &str = "deepseek-fetch-api";

pub struct DeepSeekFetchApiOptions {
    pub client: Client,
    pub auth: DeepSeekWebAuth,
    pub guidance: DeepSeekWebGuidance,
}

type OptionsResolver = dyn Fn() -> Result<DeepSeekFetchApiOptions, BoxError> + Send + Sync;
type ReadyAuthResolver =
    dyn for<'a> Fn(Option<&'a str>) -> Result<DeepSeekWebAuth, BoxError> + Send + Sync;

pub struct DeepSeekFetchApiProvider {
    resolve_options: Arc<OptionsResolver>,
    resolve_ready_auth: Arc<ReadyAuthResolver>,
}

impl DeepSeekFetchApiProvider {
    pub fn new(
        resolve_options: impl Fn() -> DeepSeekFetchApiOptions + Send + Sync + 'static,
    ) -> Self {
        let resolve_options = Arc::new(resolve_options);
        let ready_options = resolve_options.clone();
        Self {
            resolve_options: Arc::new(move || Ok(resolve_options())),
            resolve_ready_auth: Arc::new(move |_| Ok((ready_options)().auth)),
        }
    }
    pub fn with_ready_auth(
        resolve_options: impl Fn() -> Result<DeepSeekFetchApiOptions, BoxError> + Send + Sync + 'static,
        resolve_ready_auth: impl Fn(Option<&str>) -> Result<DeepSeekWebAuth, BoxError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            resolve_options: Arc::new(resolve_options),
            resolve_ready_auth: Arc::new(resolve_ready_auth),
        }
    }
    fn options(&self) -> Result<DeepSeekFetchApiOptions, BoxError> {
        (self.resolve_options)()
    }
}

impl WebFetchProvider for DeepSeekFetchApiProvider {
    fn info(&self) -> WebProviderInfo {
        WebProviderInfo {
            id: DEEPSEEK_FETCH_API_PROVIDER_ID.into(),
            title: None,
            description: Some(LocalizedText::Translations(IndexMap::from([
                ("en".into(), "Can't reach intranet or local addresses. Incurs charges, paid by the account or API key chosen under Billing.".into()),
                ("zh".into(), "无法访问内网和本机。会产生费用，由「计费方式」选定的账号或 API Key 支付。".into()),
                ("ja".into(), "イントラネットやローカルのアドレスにはアクセスできません。料金が発生し、「課金方法」で選んだアカウントまたは API キーに請求されます。".into()),
                ("es".into(), "No puede acceder a direcciones de la intranet ni locales. Genera cargos, que paga la cuenta o la clave de API elegida en Facturación.".into()),
                ("fr".into(), "N'accède pas aux adresses intranet ou locales. Entraîne des frais, payés par le compte ou la clé API choisi dans Facturation.".into()),
                ("de".into(), "Erreicht keine Intranet- oder lokalen Adressen. Verursacht Kosten, die das unter Abrechnung gewählte Konto oder der API-Schlüssel trägt.".into()),
                ("ru".into(), "Адреса интрасети и локальные адреса недоступны. Услуга платная: оплата идёт с аккаунта или ключа API, выбранного в разделе «Оплата».".into()),
                ("pt".into(), "Não acessa endereços da intranet nem locais. Gera cobranças, pagas pela conta ou chave de API escolhida em Cobrança.".into()),
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
            }))
    }
    fn ready<'a>(&'a self, llm_provider: Option<&'a str>) -> WebFuture<'a, bool> {
        let auth = (self.resolve_ready_auth)(llm_provider);
        Box::pin(async move {
            let auth = auth?;
            Ok(is_deep_seek_web_auth_ready(&auth, "/fetch").await)
        })
    }
    fn fetch<'a>(
        &'a self,
        request: WebFetchRequest,
        signal: Option<&'a AbortSignal>,
    ) -> WebFuture<'a, WebFetchResult> {
        let options = self.options();
        Box::pin(async move {
            let options = options?;
            let requested_url = request.url;
            let returned_url = requested_url.clone();
            let operation = DeepSeekWebRequest {
                client: options.client,
                path: "/fetch".into(),
                body: json!({"url": requested_url}),
                headers: Default::default(),
                api_key_headers: None,
                decode: Arc::new(move |payload| map_fetch_response(returned_url.clone(), payload)),
                guidance: options.guidance,
                record_request: None,
                wait_to_send: None,
            };
            dispatch_deep_seek_web(&options.auth, &operation, signal).await
        })
    }
}

fn map_fetch_response(url: String, payload: Value) -> Result<WebFetchResult, BoxError> {
    let content = payload
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| Box::new(invalid_response()) as BoxError)?;
    Ok(WebFetchResult {
        url,
        status_code: 200,
        body: WebFetchBody::Text {
            content: content.into(),
        },
        truncated: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoh_timeout::{AbortController, AbortError};

    fn provider(base: &str, key: &str) -> DeepSeekFetchApiProvider {
        let base = base.to_owned();
        let key = key.to_owned();
        DeepSeekFetchApiProvider::new(move || DeepSeekFetchApiOptions {
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
            guidance: DeepSeekWebGuidance {
                subject: "Fetch",
                plugin: "web-deepseek-api",
                custom: true,
                base_url_field: "fetchBaseURL".into(),
                api_key_env_field: "fetchApiKeyEnv".into(),
            },
        })
    }
    #[test]
    fn availability_requires_credential_and_parseable_base() {
        assert!(provider("https://api.test", "key").available().unwrap());
        assert!(!provider("https://api.test", "").available().unwrap());
        assert!(!provider("not a url", "key").available().unwrap());
    }
    #[tokio::test]
    async fn ready_checks_credential_presence() {
        assert!(
            provider("https://api.test", "key")
                .ready(None)
                .await
                .unwrap()
        );
        assert!(!provider("https://api.test", "").ready(None).await.unwrap());
    }
    #[test]
    fn provider_metadata_is_complete() {
        let info = provider("https://api.test", "key").info();
        assert_eq!(info.id, DEEPSEEK_FETCH_API_PROVIDER_ID);
        let LocalizedText::Translations(translations) = info.description.unwrap() else {
            panic!("translated description")
        };
        assert!(translations.contains_key("en"));
        assert!(translations.contains_key("zh"));
        assert!(translations.contains_key("ru"));
    }

    #[test]
    fn decodes_markdown_and_rejects_malformed_responses() {
        let result = map_fetch_response(
            "https://page.test".into(),
            serde_json::json!({"content":"# Heading\n\nBody"}),
        )
        .unwrap();
        assert_eq!(result.url, "https://page.test");
        assert_eq!(result.status_code, 200);
        assert_eq!(
            result.body,
            WebFetchBody::Text {
                content: "# Heading\n\nBody".into()
            }
        );
        for payload in [
            Value::Null,
            serde_json::json!({}),
            serde_json::json!({"content": 1}),
        ] {
            let error = map_fetch_response("https://page.test".into(), payload).unwrap_err();
            assert_eq!(
                hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
                "WEB_PROVIDER_ERROR"
            );
        }
    }

    #[tokio::test]
    async fn pre_aborted_fetch_dispatches_nothing() {
        let controller = AbortController::new();
        controller.abort_with(AbortError::new("caller stopped"));
        let error = provider("http://example.invalid", "key")
            .fetch(
                WebFetchRequest {
                    url: "https://page.test".into(),
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
