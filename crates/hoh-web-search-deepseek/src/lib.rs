//! DeepSeek-backed web search through the Anthropic-compatible Messages API.

pub mod provider;
pub mod types;

pub use provider::{
    DEEPSEEK_DEFAULT_API_VERSION, DEEPSEEK_DEFAULT_BASE_URL, DEEPSEEK_DEFAULT_MAX_TOKENS,
    DEEPSEEK_DEFAULT_MAX_USES, DEEPSEEK_DEFAULT_MODEL, DEEPSEEK_PROVIDER_ID,
    DeepSeekSearchLlmRequest, DeepSeekSearchProvider, DeepSeekSearchProviderOptions,
    citation_snippets, map_anthropic_response,
};

use hoh_agent::Agents;
use hoh_credentials::credential_ref;
use hoh_error::BoxError;
use hoh_http_proxy::{DispatcherOptions, install_proxy_from_environment, route_client_builder};
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service, Volatile};
use hoh_launch_environment::launch_environment_of;
use hoh_llm_deepseek::{
    DeepSeekAccount, deep_seek_api_key_configured, reject_deep_seek_account_token,
    resolve_deep_seek_account_token, resolve_deep_seek_api_key,
};
use hoh_web::Web;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const NAME: &str = "web-search-deepseek";
pub const WEB_SEARCH_DEEPSEEK_SETTINGS_NAMESPACE: &str = "web-search-deepseek";
const DEFAULT_API_KEY_ENV: &str = "DEEPSEEK_API_KEY";
const SEARCH_BASE_URL_ENV: &str = "DEEPSEEK_SEARCH_BASE_URL";
const ACCOUNT_PROVIDER: &str = "deepseek-account";
const REQUEST_EVENT: &str = "web/deepseek-search-llm-request";

fn default_api_key_env() -> Volatile<String> {
    Volatile(DEFAULT_API_KEY_ENV.into())
}
fn default_model() -> Volatile<String> {
    Volatile(DEEPSEEK_DEFAULT_MODEL.into())
}
fn default_api_version() -> Volatile<String> {
    Volatile(DEEPSEEK_DEFAULT_API_VERSION.into())
}
fn default_max_tokens() -> Volatile<u64> {
    Volatile(DEEPSEEK_DEFAULT_MAX_TOKENS)
}
fn default_max_uses() -> Volatile<u64> {
    Volatile(DEEPSEEK_DEFAULT_MAX_USES)
}

fn deserialize_integer_volatile<'de, D>(deserializer: D) -> Result<Volatile<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    let Some(number) = value.as_number() else {
        return Err(serde::de::Error::custom("expected an integer"));
    };
    if let Some(value) = number.as_u64() {
        return Ok(Volatile(value));
    }
    let Some(value) = number.as_f64() else {
        return Err(serde::de::Error::custom("expected an integer"));
    };
    if !value.is_finite()
        || value.fract() != 0.0
        || !(0.0..18_446_744_073_709_551_616.0).contains(&value)
    {
        return Err(serde::de::Error::custom("expected an integer"));
    }
    Ok(Volatile(value as u64))
}

#[derive(Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default)]
    #[schemars(extend("x-role" = "secret"))]
    pub api_key: Volatile<Option<String>>,
    #[serde(default = "default_api_key_env")]
    #[schemars(extend("x-role" = "credential-ref"))]
    pub api_key_env: Volatile<String>,
    #[serde(default, rename = "baseURL")]
    pub base_url: Volatile<Option<String>>,
    #[serde(default = "default_model")]
    pub model: Volatile<String>,
    #[serde(default = "default_api_version")]
    pub api_version: Volatile<String>,
    #[serde(default = "default_max_tokens")]
    #[schemars(range(min = 1))]
    pub max_tokens: Volatile<u64>,
    #[serde(default = "default_max_uses")]
    #[schemars(range(min = 1))]
    pub max_uses: Volatile<u64>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Config")
            .field("api_key", &self.api_key.0.as_ref().map(|_| "<redacted>"))
            .field("api_key_env", &self.api_key_env)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_version", &self.api_version)
            .field("max_tokens", &self.max_tokens)
            .field("max_uses", &self.max_uses)
            .finish()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawConfig {
    #[serde(default)]
    api_key: StrictOptionalString,
    #[serde(default = "default_api_key_env")]
    api_key_env: Volatile<String>,
    #[serde(default, rename = "baseURL")]
    base_url: StrictOptionalString,
    #[serde(default = "default_model")]
    model: Volatile<String>,
    #[serde(default = "default_api_version")]
    api_version: Volatile<String>,
    #[serde(
        default = "default_max_tokens",
        deserialize_with = "deserialize_integer_volatile"
    )]
    max_tokens: Volatile<u64>,
    #[serde(
        default = "default_max_uses",
        deserialize_with = "deserialize_integer_volatile"
    )]
    max_uses: Volatile<u64>,
}

#[derive(Default)]
struct StrictOptionalString(Option<String>);

impl<'de> Deserialize<'de> for StrictOptionalString {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|value| Self(Some(value)))
    }
}

impl<'de> Deserialize<'de> for Config {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawConfig::deserialize(deserializer)?;
        if raw.max_tokens.0 == 0 {
            return Err(serde::de::Error::custom(
                "web-search-deepseek: maxTokens must be a positive integer",
            ));
        }
        if raw.max_uses.0 == 0 {
            return Err(serde::de::Error::custom(
                "web-search-deepseek: maxUses must be a positive integer",
            ));
        }
        Ok(Self {
            api_key: Volatile(raw.api_key.0),
            api_key_env: raw.api_key_env,
            base_url: Volatile(raw.base_url.0),
            model: raw.model,
            api_version: raw.api_version,
            max_tokens: raw.max_tokens,
            max_uses: raw.max_uses,
        })
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_key: Volatile(None),
            api_key_env: default_api_key_env(),
            base_url: Volatile(None),
            model: default_model(),
            api_version: default_api_version(),
            max_tokens: default_max_tokens(),
            max_uses: default_max_uses(),
        }
    }
}

#[derive(Inject)]
pub struct Deps {
    web: Service<Web>,
}

pub struct WebSearchDeepSeek;

impl Plugin for WebSearchDeepSeek {
    const NAME: &'static str = NAME;
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let client = transport_client(&ctx)?;
        let options_ctx = ctx.clone();
        let options_config = config.clone();
        let ready_ctx = ctx.clone();
        let ready_config = config.clone();
        let provider = DeepSeekSearchProvider::with_ready(
            client,
            move || resolve_options(&options_ctx, &options_config.get()),
            move |route| {
                let context = ready_ctx.clone();
                let options = resolve_options(&context, &ready_config.get());
                Box::pin(async move {
                    let options = options?;
                    Ok(search_configured(&context, &options, route.as_deref()).await)
                })
            },
        );
        deps.web
            .register_search_provider(&ctx, Arc::new(provider))?;
        Ok(())
    }
}

fn transport_client(ctx: &Context) -> Result<reqwest::Client, BoxError> {
    let environment = launch_environment_of(ctx);
    let lookup = |name: &str| environment.get(name).map(|entry| entry.value);
    let installation =
        install_proxy_from_environment(&lookup, |_| {}, DispatcherOptions::default())?;
    Ok(
        route_client_builder(installation.policy().clone(), installation.options())?
            .redirect(reqwest::redirect::Policy::none())
            .build()?,
    )
}

fn resolve_options(
    ctx: &Context,
    config: &Config,
) -> Result<DeepSeekSearchProviderOptions, BoxError> {
    let api_key_env = credential_ref(&config.api_key_env.0)?;
    let literal = config
        .api_key
        .0
        .as_ref()
        .filter(|key| !key.is_empty())
        .cloned();
    let base_url = config.base_url.0.clone().unwrap_or_else(|| {
        launch_environment_of(ctx)
            .get(SEARCH_BASE_URL_ENV)
            .map_or_else(|| DEEPSEEK_DEFAULT_BASE_URL.into(), |entry| entry.value)
    });

    let account_ctx = ctx.clone();
    let reject_ctx = ctx.clone();
    let key_ctx = ctx.clone();
    let record_ctx = ctx.clone();
    let key_ref = api_key_env.clone();
    Ok(DeepSeekSearchProviderOptions {
        resolve_account_token: Some(Arc::new(move |endpoint| {
            let context = account_ctx.clone();
            Box::pin(async move {
                let Some(agents) = context.get::<Agents>() else {
                    return Ok(None);
                };
                let Some(agent) = agents.current_initiator()? else {
                    return Ok(None);
                };
                let provider = agent
                    .session()
                    .request_context()
                    .and_then(|value| value.get("provider").cloned())
                    .and_then(|value| value.as_str().map(str::to_owned));
                if provider.as_deref() != Some(ACCOUNT_PROVIDER) {
                    return Ok(None);
                }
                resolve_deep_seek_account_token(&context, &endpoint).await
            })
        })),
        reject_account_token: Some(Arc::new(move |token| {
            let context = reject_ctx.clone();
            Box::pin(async move {
                reject_deep_seek_account_token(&context, &token).await;
                Ok(())
            })
        })),
        api_key: literal,
        resolve_api_key: Some(Arc::new(move || {
            let context = key_ctx.clone();
            let reference = key_ref.clone();
            Box::pin(async move { resolve_deep_seek_api_key(&context, &reference).await })
        })),
        api_key_env: Some(api_key_env),
        base_url,
        model: config.model.0.clone(),
        api_version: config.api_version.0.clone(),
        max_tokens: config.max_tokens.0,
        max_uses: config.max_uses.0,
        record_request: Some(Arc::new(move |request| {
            let Some(agents) = record_ctx.get::<Agents>() else {
                return Ok(());
            };
            let Some(agent) = agents.current_initiator()? else {
                return Ok(());
            };
            agent
                .session()
                .append(REQUEST_EVENT, serde_json::to_value(request)?, None)?;
            Ok(())
        })),
    })
}

async fn search_configured(
    ctx: &Context,
    options: &DeepSeekSearchProviderOptions,
    route: Option<&str>,
) -> bool {
    if route == Some(ACCOUNT_PROVIDER) && ctx.get::<DeepSeekAccount>().is_some() {
        match resolve_deep_seek_account_token(ctx, &format!("{}/messages", options.base_url)).await
        {
            Ok(Some(token)) if !token.is_empty() => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    if options.api_key.is_some() {
        return true;
    }
    let reference = match options.api_key_env.clone() {
        Some(reference) => reference,
        None => match credential_ref(DEFAULT_API_KEY_ENV) {
            Ok(reference) => reference,
            Err(_) => return false,
        },
    };
    deep_seek_api_key_configured(ctx, &reference)
        .await
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::future::BoxFuture;
    use hoh_agent::AgentRegistryPlugin;
    use hoh_credentials::{
        CredentialFuture, CredentialInfo, CredentialKey, CredentialMutator, CredentialProvider,
        CredentialRecord, CredentialRecordEntry, CredentialRecordInfo, CredentialRef, Credentials,
        ResolvedCredential,
    };
    use hoh_launch_environment::{
        LaunchEnvironment, LaunchEnvironmentLayerInput, LaunchEnvironmentSource,
        create_launch_environment_snapshot,
    };
    use hoh_llm_deepseek::DeepSeekAccountApi;
    use hoh_session_projection::SessionProjectionRegistry;
    use indexmap::IndexMap;
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    struct Account {
        result: Mutex<Option<Result<Option<String>, String>>>,
        calls: Arc<AtomicUsize>,
    }

    struct MemoryCredentials {
        value: Mutex<Option<String>>,
        resolutions: Arc<Mutex<Vec<Option<String>>>>,
        unreadable: bool,
    }

    impl CredentialProvider for MemoryCredentials {
        fn resolve<'a>(
            &'a self,
            _: &'a CredentialRef,
        ) -> CredentialFuture<'a, Option<ResolvedCredential>> {
            Box::pin(async move {
                if self.unreadable {
                    return Err(std::io::Error::other("credential store unreadable").into());
                }
                let value = self.value.lock().unwrap().clone();
                self.resolutions.lock().unwrap().push(value.clone());
                Ok(value.map(|value| ResolvedCredential {
                    value,
                    source: "test-store".into(),
                }))
            })
        }

        fn describe<'a>(&'a self, _: &'a CredentialRef) -> CredentialFuture<'a, CredentialInfo> {
            Box::pin(async move {
                if self.unreadable {
                    return Err(std::io::Error::other("credential store unreadable").into());
                }
                let configured = self.value.lock().unwrap().is_some();
                Ok(CredentialInfo {
                    configured,
                    source: configured.then(|| "test-store".into()),
                    writable: true,
                    masked: None,
                })
            })
        }

        fn set<'a>(&'a self, _: &'a CredentialRef, value: &'a str) -> CredentialFuture<'a, ()> {
            Box::pin(async move {
                *self.value.lock().unwrap() = Some(value.into());
                Ok(())
            })
        }

        fn unset<'a>(&'a self, _: &'a CredentialRef) -> CredentialFuture<'a, ()> {
            Box::pin(async move {
                *self.value.lock().unwrap() = None;
                Ok(())
            })
        }

        fn read_record<'a>(
            &'a self,
            _: &'a CredentialKey,
        ) -> CredentialFuture<'a, Option<CredentialRecord>> {
            Box::pin(async { Ok(None) })
        }

        fn describe_record<'a>(
            &'a self,
            _: &'a CredentialKey,
        ) -> CredentialFuture<'a, CredentialRecordInfo> {
            Box::pin(async {
                Ok(CredentialRecordInfo {
                    configured: false,
                    kind: None,
                    writable: false,
                })
            })
        }

        fn list_records(&self) -> CredentialFuture<'_, Vec<CredentialRecordEntry>> {
            Box::pin(async { Ok(vec![]) })
        }

        fn modify_record<'a>(
            &'a self,
            _: &'a CredentialKey,
            _: CredentialMutator,
        ) -> CredentialFuture<'a, Option<CredentialRecord>> {
            Box::pin(async { Ok(None) })
        }

        fn delete_record<'a>(&'a self, _: &'a CredentialKey) -> CredentialFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    impl DeepSeekAccountApi for Account {
        fn resolve_token<'a>(
            &'a self,
            _endpoint: &'a str,
        ) -> BoxFuture<'a, Result<Option<String>, BoxError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                match self.result.lock().unwrap().take().unwrap() {
                    Ok(value) => Ok(value),
                    Err(message) => Err(std::io::Error::other(message).into()),
                }
            })
        }

        fn reject_token<'a>(&'a self, _token: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn options_use_live_config_then_launch_environment_then_constant() {
        let default_kernel = hoh_kernel::Kernel::start();
        let default_ctx = default_kernel.root();
        default_ctx
            .provide::<LaunchEnvironment>(Arc::new(create_launch_environment_snapshot(&[])))
            .await
            .unwrap();
        let defaults = resolve_options(&default_ctx, &Config::default()).unwrap();
        assert_eq!(defaults.base_url, DEEPSEEK_DEFAULT_BASE_URL);
        assert_eq!(defaults.model, DEEPSEEK_DEFAULT_MODEL);
        default_kernel.shutdown().await;

        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        let environment = create_launch_environment_snapshot(&[LaunchEnvironmentLayerInput {
            source: LaunchEnvironmentSource::Process,
            path: None,
            values: IndexMap::from([(SEARCH_BASE_URL_ENV.into(), "https://env.test/v1".into())]),
        }]);
        ctx.provide::<LaunchEnvironment>(Arc::new(environment))
            .await
            .unwrap();
        let mut config = Config::default();
        let options = resolve_options(&ctx, &config).unwrap();
        assert_eq!(options.base_url, "https://env.test/v1");
        assert_eq!(options.model, DEEPSEEK_DEFAULT_MODEL);
        config.base_url.0 = Some("https://config.test/v1".into());
        config.api_key.0 = Some(String::new());
        let options = resolve_options(&ctx, &config).unwrap();
        assert_eq!(options.base_url, "https://config.test/v1");
        assert_eq!(options.api_key, None);
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn readiness_asks_account_only_on_account_route_and_account_error_is_terminal() {
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        let calls = Arc::new(AtomicUsize::new(0));
        ctx.provide::<DeepSeekAccount>(Arc::new(Account {
            result: Mutex::new(Some(Err("grant unreadable".into()))),
            calls: calls.clone(),
        }))
        .await
        .unwrap();
        let mut options = resolve_options(&ctx, &Config::default()).unwrap();
        options.api_key = Some("literal-key".into());
        assert!(search_configured(&ctx, &options, Some("openai")).await);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(!search_configured(&ctx, &options, Some(ACCOUNT_PROVIDER)).await);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn readiness_accepts_non_empty_account_token() {
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        ctx.provide::<DeepSeekAccount>(Arc::new(Account {
            result: Mutex::new(Some(Ok(Some("account-token".into())))),
            calls: Arc::new(AtomicUsize::new(0)),
        }))
        .await
        .unwrap();
        let options = resolve_options(&ctx, &Config::default()).unwrap();
        assert!(search_configured(&ctx, &options, Some(ACCOUNT_PROVIDER)).await);
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn readiness_falls_through_empty_account_token_to_literal_key() {
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        ctx.provide::<DeepSeekAccount>(Arc::new(Account {
            result: Mutex::new(Some(Ok(Some(String::new())))),
            calls: Arc::new(AtomicUsize::new(0)),
        }))
        .await
        .unwrap();
        let mut options = resolve_options(&ctx, &Config::default()).unwrap();
        options.api_key = Some("literal-key".into());
        assert!(search_configured(&ctx, &options, Some(ACCOUNT_PROVIDER)).await);
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn readiness_uses_key_from_launch_environment_on_any_model_route() {
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        let environment = create_launch_environment_snapshot(&[LaunchEnvironmentLayerInput {
            source: LaunchEnvironmentSource::Process,
            path: None,
            values: IndexMap::from([(DEFAULT_API_KEY_ENV.into(), "env-key".into())]),
        }]);
        ctx.provide::<LaunchEnvironment>(Arc::new(environment))
            .await
            .unwrap();
        let options = resolve_options(&ctx, &Config::default()).unwrap();
        assert!(search_configured(&ctx, &options, Some("openai")).await);
        assert!(search_configured(&ctx, &options, None).await);
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn credential_store_is_resolved_again_after_key_rotation() {
        use hoh_web::{WebSearchProvider, WebSearchRequest};

        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        let resolutions = Arc::new(Mutex::new(Vec::new()));
        let credentials = Arc::new(MemoryCredentials {
            value: Mutex::new(Some("stored-key".into())),
            resolutions: resolutions.clone(),
            unreadable: false,
        });
        let service: Arc<dyn CredentialProvider> = credentials.clone();
        ctx.provide::<Credentials>(service).await.unwrap();
        let mut config = Config::default();
        config.api_version.0 = "invalid\nversion".into();
        let options_ctx = ctx.clone();
        let search =
            DeepSeekSearchProvider::new(move || resolve_options(&options_ctx, &config).unwrap());

        for key in ["stored-key", "rotated-key"] {
            *credentials.value.lock().unwrap() = Some(key.into());
            assert!(
                search
                    .search(
                        WebSearchRequest {
                            query: key.into(),
                            max_results: None,
                        },
                        None,
                    )
                    .await
                    .is_err()
            );
        }
        assert_eq!(
            *resolutions.lock().unwrap(),
            vec![Some("stored-key".into()), Some("rotated-key".into())]
        );
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn unreadable_credential_store_is_not_ready_and_search_does_not_fall_back() {
        use hoh_web::{WebSearchProvider, WebSearchRequest};

        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        let service: Arc<dyn CredentialProvider> = Arc::new(MemoryCredentials {
            value: Mutex::new(None),
            resolutions: Arc::new(Mutex::new(Vec::new())),
            unreadable: true,
        });
        ctx.provide::<Credentials>(service).await.unwrap();
        let config = Config::default();
        let options = resolve_options(&ctx, &config).unwrap();
        assert!(!search_configured(&ctx, &options, Some("deepseek-official")).await);

        let search_options = resolve_options(&ctx, &config).unwrap();
        let search = DeepSeekSearchProvider::new(move || search_options.clone());
        let error = search
            .search(
                WebSearchRequest {
                    query: "q".into(),
                    max_results: None,
                },
                None,
            )
            .await
            .unwrap_err();
        let error = hoh_error::HarnessError::of(error.as_ref()).unwrap();
        assert_eq!(error.code(), "WEB_PROVIDER_ERROR");
        assert!(error.message().contains("credential store unreadable"));
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn account_and_request_callbacks_are_no_ops_without_an_initiating_agent() {
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        let projections = ctx
            .plugin::<SessionProjectionRegistry>(serde_json::json!({}))
            .unwrap();
        projections.settled().await.unwrap();
        let agents = ctx
            .plugin::<AgentRegistryPlugin>(serde_json::json!({}))
            .unwrap();
        agents.settled().await.unwrap();
        let options = resolve_options(&ctx, &Config::default()).unwrap();

        let resolve_account = options.resolve_account_token.unwrap();
        let token = resolve_account("https://api.test/messages".into())
            .await
            .unwrap();
        assert_eq!(token, None);
        let record_request = options.record_request.unwrap();
        record_request(DeepSeekSearchLlmRequest {
            endpoint: "https://api.test/messages".into(),
            api_version: DEEPSEEK_DEFAULT_API_VERSION.into(),
            body: crate::provider::DeepSeekSearchRequestBody {
                model: DEEPSEEK_DEFAULT_MODEL.into(),
                max_tokens: DEEPSEEK_DEFAULT_MAX_TOKENS,
                messages: vec![],
                tools: vec![],
                thinking: None,
            },
        })
        .unwrap();
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn malformed_credential_reference_is_a_normal_error() {
        let kernel = hoh_kernel::Kernel::start();
        let ctx = kernel.root();
        let mut config = Config::default();
        config.api_key_env.0 = "bad key".into();
        assert!(resolve_options(&ctx, &config).is_err());
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn malformed_launch_environment_key_fails_before_request_recording() {
        use hoh_web::{WebSearchProvider, WebSearchRequest};

        for value in ["bad key", "非法密钥"] {
            let kernel = hoh_kernel::Kernel::start();
            let ctx = kernel.root();
            let environment = create_launch_environment_snapshot(&[LaunchEnvironmentLayerInput {
                source: LaunchEnvironmentSource::Process,
                path: None,
                values: IndexMap::from([(DEFAULT_API_KEY_ENV.into(), value.into())]),
            }]);
            ctx.provide::<LaunchEnvironment>(Arc::new(environment))
                .await
                .unwrap();
            let mut options = resolve_options(&ctx, &Config::default()).unwrap();
            options.api_version = "invalid\nversion".into();
            let recorded = Arc::new(AtomicUsize::new(0));
            let calls = recorded.clone();
            options.record_request = Some(Arc::new(move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }));
            let provider = DeepSeekSearchProvider::new(move || options.clone());
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
                hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
                "WEB_PROVIDER_ERROR"
            );
            assert_eq!(recorded.load(Ordering::SeqCst), 0);
            kernel.shutdown().await;
        }
    }

    #[test]
    fn config_debug_redacts_literal_api_key() {
        let mut config = Config::default();
        config.api_key.0 = Some("sk-live-secret".into());
        let debug = format!("{config:?}");
        assert!(!debug.contains("sk-live-secret"));
        assert!(debug.contains("<redacted>"));
    }
}
