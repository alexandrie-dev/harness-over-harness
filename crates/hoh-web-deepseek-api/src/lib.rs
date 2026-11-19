//! DeepSeek Search and Fetch API providers over the shared web seam.

mod fetch;
mod search;
mod shim;
mod transport;

pub use fetch::{
    DEEPSEEK_FETCH_API_PROVIDER_ID, DeepSeekFetchApiOptions, DeepSeekFetchApiProvider,
};
pub use search::{
    DEEPSEEK_DEFAULT_COUNT, DEEPSEEK_SEARCH_API_PROVIDER_ID, DEEPSEEK_SEARCH_REQUESTS_PER_SECOND,
    DEEPSEEK_WEB_API_BASE_URL, DeepSeekSearchApiOptions, DeepSeekSearchApiProvider,
    map_search_response,
};
pub use transport::{
    DeepSeekWebApiKey, DeepSeekWebAuth, DeepSeekWebAuthSource, DeepSeekWebGuidance,
    DeepSeekWebRequest, dispatch_deep_seek_web, has_deep_seek_web_credential, invalid_response,
    is_deep_seek_web_auth_ready, search_aborted,
};

use hoh_agent::Agents;
use hoh_credentials::{CredentialRef, credential_ref};
use hoh_error::BoxError;
use hoh_http_proxy::{DispatcherOptions, install_proxy_from_environment, route_client_builder};
use hoh_kernel::{Cfg, Context, FiberState, Inject, Plugin, Service, Volatile};
use hoh_launch_environment::launch_environment_of;
use hoh_llm::Llm;
use hoh_llm_deepseek::{
    deep_seek_api_key_configured, reject_deep_seek_account_token, resolve_deep_seek_account_token,
    resolve_deep_seek_api_key,
};
use hoh_llm_deepseek_api_key::{Config as OfficialConfig, plain_options, resolve_adapter_options};
use hoh_loader::{EntryInfo, LoaderKey};
use hoh_web::Web;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use url::Url;

pub const NAME: &str = "web-deepseek-api";
const TOOLS_PATH: &str = "/v1/tools";
const ACCOUNT_PROVIDER: &str = "deepseek-account";
const OFFICIAL_PROVIDER: &str = "deepseek-official";
const ACCOUNT_PACKAGE: &str = "@deepseek-ai/dsh-llm-deepseek-account";
const OFFICIAL_PACKAGE: &str = "@deepseek-ai/dsh-llm-deepseek-api-key";

fn default_count() -> Volatile<u64> {
    Volatile(DEEPSEEK_DEFAULT_COUNT)
}
fn default_rate() -> Volatile<u64> {
    Volatile(DEEPSEEK_SEARCH_REQUESTS_PER_SECOND)
}
fn default_true() -> Volatile<bool> {
    Volatile(true)
}
fn default_billing() -> Volatile<Billing> {
    Volatile(Billing::Auto)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Billing {
    #[default]
    Auto,
    Account,
    ApiKey,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default = "default_count")]
    #[schemars(range(min = 1, max = 50))]
    pub count: Volatile<u64>,
    #[serde(default = "default_rate")]
    #[schemars(range(min = 1))]
    pub search_requests_per_second: Volatile<u64>,
    #[serde(default = "default_true")]
    pub send_original_question: Volatile<bool>,
    #[serde(default = "default_billing")]
    pub search_billing: Volatile<Billing>,
    #[serde(default = "default_billing")]
    pub fetch_billing: Volatile<Billing>,
    #[serde(default, rename = "searchBaseURL")]
    pub search_base_url: Volatile<Option<String>>,
    #[serde(default)]
    pub search_api_key_env: Volatile<Option<String>>,
    #[serde(default, rename = "fetchBaseURL")]
    pub fetch_base_url: Volatile<Option<String>>,
    #[serde(default)]
    pub fetch_api_key_env: Volatile<Option<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawConfig {
    #[serde(default = "default_count")]
    count: Volatile<u64>,
    #[serde(default = "default_rate")]
    search_requests_per_second: Volatile<u64>,
    #[serde(default = "default_true")]
    send_original_question: Volatile<bool>,
    #[serde(default = "default_billing")]
    search_billing: Volatile<Billing>,
    #[serde(default = "default_billing")]
    fetch_billing: Volatile<Billing>,
    #[serde(default, rename = "searchBaseURL")]
    search_base_url: Volatile<Option<String>>,
    #[serde(default)]
    search_api_key_env: Volatile<Option<String>>,
    #[serde(default, rename = "fetchBaseURL")]
    fetch_base_url: Volatile<Option<String>>,
    #[serde(default)]
    fetch_api_key_env: Volatile<Option<String>>,
}

impl<'de> Deserialize<'de> for Config {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawConfig::deserialize(deserializer)?;
        if !(1..=50).contains(&raw.count.0) {
            return Err(serde::de::Error::custom(
                "web-deepseek-api: count must be an integer from 1 through 50",
            ));
        }
        if raw.search_requests_per_second.0 == 0 {
            return Err(serde::de::Error::custom(
                "web-deepseek-api: searchRequestsPerSecond must be a positive integer",
            ));
        }
        Ok(Self {
            count: raw.count,
            search_requests_per_second: raw.search_requests_per_second,
            send_original_question: raw.send_original_question,
            search_billing: raw.search_billing,
            fetch_billing: raw.fetch_billing,
            search_base_url: raw.search_base_url,
            search_api_key_env: raw.search_api_key_env,
            fetch_base_url: raw.fetch_base_url,
            fetch_api_key_env: raw.fetch_api_key_env,
        })
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            count: default_count(),
            search_requests_per_second: default_rate(),
            send_original_question: default_true(),
            search_billing: default_billing(),
            fetch_billing: default_billing(),
            search_base_url: Volatile(None),
            search_api_key_env: Volatile(None),
            fetch_base_url: Volatile(None),
            fetch_api_key_env: Volatile(None),
        }
    }
}

#[derive(Inject)]
pub struct Deps {
    web: Service<Web>,
}

pub struct WebDeepSeekApi;
impl Plugin for WebDeepSeekApi {
    const NAME: &'static str = NAME;
    type Config = Config;
    type Deps = Deps;
    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        apply(&ctx, &deps.web, config)
    }
}

#[derive(Clone, Copy)]
enum Capability {
    Search,
    Fetch,
}
impl Capability {
    fn subject(self) -> &'static str {
        match self {
            Self::Search => "Search",
            Self::Fetch => "Fetch",
        }
    }
    fn prefix(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Fetch => "fetch",
        }
    }
}
#[derive(Clone, Copy)]
enum Rule {
    AccountConversation,
    OfficialConversation,
    OtherConversation,
    Account,
    ApiKey,
}
#[derive(Clone)]
struct OwnSettings {
    base_url: Option<String>,
    key_ref: Option<String>,
    billing: Billing,
}
struct Target {
    auth: DeepSeekWebAuth,
    guidance: DeepSeekWebGuidance,
}

fn apply(ctx: &Context, web: &Service<Web>, config: Cfg<Config>) -> Result<(), BoxError> {
    let client = transport_client(ctx)?;
    let search_ctx = ctx.clone();
    let search_config = config.clone();
    let search_client = client.clone();
    let ready_search_ctx = ctx.clone();
    let ready_search_config = config.clone();
    let search = DeepSeekSearchApiProvider::with_ready_auth(
        move || {
            let snapshot = search_config.get();
            let target = resolve_current_target(
                &search_ctx,
                Capability::Search,
                own(&snapshot, Capability::Search),
            )?;
            Ok(DeepSeekSearchApiOptions {
                client: search_client.clone(),
                auth: target.auth,
                guidance: target.guidance,
                count: snapshot.count.0,
                requests_per_second: snapshot.search_requests_per_second.0,
                original_question: snapshot.send_original_question.0.then(|| {
                    let context = search_ctx.clone();
                    Arc::new(move || latest_user_question(&context))
                        as Arc<dyn Fn() -> Option<String> + Send + Sync>
                }),
            })
        },
        move |provider| {
            let snapshot = ready_search_config.get();
            resolve_target(
                &ready_search_ctx,
                Capability::Search,
                own(&snapshot, Capability::Search),
                provider,
            )
            .map(|target| target.auth)
        },
    );
    let fetch_ctx = ctx.clone();
    let fetch_config = config.clone();
    let fetch_client = client;
    let ready_fetch_ctx = ctx.clone();
    let ready_fetch_config = config.clone();
    let fetch = DeepSeekFetchApiProvider::with_ready_auth(
        move || {
            let snapshot = fetch_config.get();
            let target = resolve_current_target(
                &fetch_ctx,
                Capability::Fetch,
                own(&snapshot, Capability::Fetch),
            )?;
            Ok(DeepSeekFetchApiOptions {
                client: fetch_client.clone(),
                auth: target.auth,
                guidance: target.guidance,
            })
        },
        move |provider| {
            let snapshot = ready_fetch_config.get();
            resolve_target(
                &ready_fetch_ctx,
                Capability::Fetch,
                own(&snapshot, Capability::Fetch),
                provider,
            )
            .map(|target| target.auth)
        },
    );
    web.register_search_provider(ctx, Arc::new(search))?;
    web.register_fetch_provider(ctx, Arc::new(fetch))?;
    Ok(())
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

fn own(config: &Config, capability: Capability) -> OwnSettings {
    match capability {
        Capability::Search => OwnSettings {
            base_url: non_empty(config.search_base_url.0.clone()),
            key_ref: non_empty(config.search_api_key_env.0.clone()),
            billing: config.search_billing.0,
        },
        Capability::Fetch => OwnSettings {
            base_url: non_empty(config.fetch_base_url.0.clone()),
            key_ref: non_empty(config.fetch_api_key_env.0.clone()),
            billing: config.fetch_billing.0,
        },
    }
}

fn current_provider(ctx: &Context) -> Option<String> {
    ctx.get::<Agents>()
        .and_then(|agents| agents.current_initiator().ok().flatten())
        .and_then(|agent| agent.session().request_context())
        .and_then(|value| {
            value
                .get("provider")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
}

fn latest_user_question(ctx: &Context) -> Option<String> {
    let agent = ctx.get::<Agents>()?.current_initiator().ok().flatten()?;
    agent
        .session()
        .derive_messages()
        .ok()?
        .iter()
        .rev()
        .find_map(|message| {
            (message.get("role").and_then(serde_json::Value::as_str) == Some("user")
                && message
                    .pointer("/source/kind")
                    .and_then(serde_json::Value::as_str)
                    == Some("user"))
            .then(|| {
                message
                    .get("content")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|block| {
                        block.get("type").and_then(serde_json::Value::as_str) == Some("text")
                    })
                    .filter_map(|block| block.get("text").and_then(serde_json::Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        })
}

fn resolve_current_target(
    ctx: &Context,
    capability: Capability,
    own: OwnSettings,
) -> Result<Target, BoxError> {
    resolve_target(ctx, capability, own, current_provider(ctx).as_deref())
}

fn resolve_target(
    ctx: &Context,
    capability: Capability,
    own: OwnSettings,
    provider: Option<&str>,
) -> Result<Target, BoxError> {
    let custom = own.base_url.is_some() || own.key_ref.is_some();
    let guidance = guidance(capability, custom);
    if custom {
        let reference = credential_ref(own.key_ref.as_deref().unwrap_or("DEEPSEEK_API_KEY"))?;
        let json = serde_json::Value::String(reference.as_str().to_owned()).to_string();
        return Ok(Target {
            auth: DeepSeekWebAuth {
                sources: vec![api_key_source(
                    ctx,
                    own.base_url.as_deref().unwrap_or(DEEPSEEK_WEB_API_BASE_URL),
                    reference,
                )],
                missing_credential_message: Some(format!(
                    "DeepSeek {} API sends the API key under {json} to its own endpoint, and no key is configured for {json}. Guide the user to export {json} in the launching environment, or to correct {NAME}.{}ApiKeyEnv.",
                    capability.subject(),
                    capability.prefix()
                )),
            },
            guidance,
        });
    }
    let rule = match own.billing {
        Billing::Account => Rule::Account,
        Billing::ApiKey => Rule::ApiKey,
        Billing::Auto if provider == Some(ACCOUNT_PROVIDER) => Rule::AccountConversation,
        Billing::Auto if provider == Some(OFFICIAL_PROVIDER) => Rule::OfficialConversation,
        Billing::Auto => Rule::OtherConversation,
    };
    let sources = rule_sources(ctx, capability, rule)?;
    let reference = sources
        .iter()
        .find_map(|source| match source {
            DeepSeekWebAuthSource::ApiKey {
                key: DeepSeekWebApiKey::Referenced { reference, .. },
                ..
            } => Some(reference.clone()),
            _ => None,
        })
        .unwrap_or_else(deep_seek_api_key_ref);
    Ok(Target {
        auth: DeepSeekWebAuth {
            sources,
            missing_credential_message: Some(missing_message(capability, rule, &reference)),
        },
        guidance,
    })
}

fn guidance(capability: Capability, custom: bool) -> DeepSeekWebGuidance {
    DeepSeekWebGuidance {
        subject: capability.subject(),
        plugin: NAME,
        custom,
        base_url_field: format!("{}BaseURL", capability.prefix()),
        api_key_env_field: format!("{}ApiKeyEnv", capability.prefix()),
    }
}

fn rule_sources(
    ctx: &Context,
    capability: Capability,
    rule: Rule,
) -> Result<Vec<DeepSeekWebAuthSource>, BoxError> {
    let account = route_config(ctx, ACCOUNT_PROVIDER, ACCOUNT_PACKAGE);
    let sources = match rule {
        Rule::AccountConversation if account.is_none() => vec![official_source(ctx, capability)?],
        Rule::AccountConversation => vec![
            account_source(ctx, capability, account.as_deref())?,
            official_source(ctx, capability)?,
        ],
        Rule::OfficialConversation | Rule::ApiKey => vec![official_source(ctx, capability)?],
        Rule::OtherConversation => vec![
            official_source(ctx, capability)?,
            account_source(ctx, capability, account.as_deref())?,
        ],
        Rule::Account => vec![account_source(ctx, capability, account.as_deref())?],
    };
    Ok(sources)
}

fn official_source(
    ctx: &Context,
    capability: Capability,
) -> Result<DeepSeekWebAuthSource, BoxError> {
    let Some(raw) = route_config(ctx, OFFICIAL_PROVIDER, OFFICIAL_PACKAGE) else {
        return Ok(api_key_source(
            ctx,
            DEEPSEEK_WEB_API_BASE_URL,
            deep_seek_api_key_ref(),
        ));
    };
    let config = match serde_json::from_value::<OfficialConfig>((*raw).clone()) {
        Ok(config) => config,
        Err(error) => {
            return Ok(misconfigured(
                capability,
                OFFICIAL_PROVIDER,
                &error.to_string(),
            ));
        }
    };
    match resolve_adapter_options(&plain_options(&config), Some(&launch_environment_of(ctx))) {
        Ok(resolved) => Ok(api_key_source(
            ctx,
            &tools_base(&resolved.base_url)?,
            resolved.api_key_env,
        )),
        Err(error) => Ok(misconfigured(
            capability,
            OFFICIAL_PROVIDER,
            &error.to_string(),
        )),
    }
}

fn account_source(
    ctx: &Context,
    capability: Capability,
    raw: Option<&serde_json::Value>,
) -> Result<DeepSeekWebAuthSource, BoxError> {
    let base_url = match shim::resolve_account_base(raw, &launch_environment_of(ctx)) {
        Ok(base_url) => tools_base(&base_url)?,
        Err(error) => return Ok(misconfigured(capability, ACCOUNT_PROVIDER, &error)),
    };
    let resolve_ctx = ctx.clone();
    let reject_ctx = ctx.clone();
    Ok(DeepSeekWebAuthSource::Account {
        base_url,
        resolve_token: Arc::new(move |endpoint| {
            let context = resolve_ctx.clone();
            Box::pin(async move { resolve_deep_seek_account_token(&context, &endpoint).await })
        }),
        reject_token: Arc::new(move |token| {
            let context = reject_ctx.clone();
            Box::pin(async move {
                reject_deep_seek_account_token(&context, &token).await;
                Ok(())
            })
        }),
    })
}

fn api_key_source(
    ctx: &Context,
    base_url: &str,
    reference: CredentialRef,
) -> DeepSeekWebAuthSource {
    let configured_ctx = ctx.clone();
    let resolve_ctx = ctx.clone();
    let configured_ref = reference.clone();
    let resolve_ref = reference.clone();
    DeepSeekWebAuthSource::ApiKey {
        base_url: base_url.to_owned(),
        key: DeepSeekWebApiKey::Referenced {
            reference,
            configured: Arc::new(move || {
                let context = configured_ctx.clone();
                let reference = configured_ref.clone();
                Box::pin(async move { deep_seek_api_key_configured(&context, &reference).await })
            }),
            resolve: Arc::new(move || {
                let context = resolve_ctx.clone();
                let reference = resolve_ref.clone();
                Box::pin(async move { resolve_deep_seek_api_key(&context, &reference).await })
            }),
        },
    }
}

fn route_config(ctx: &Context, provider: &str, package: &str) -> Option<Arc<serde_json::Value>> {
    let llm = ctx.get::<Llm>()?;
    let loader = ctx.get::<LoaderKey>()?;
    if !llm.list_providers().iter().any(|row| row.id == provider) {
        return None;
    }
    let directory = llm
        .list_configurable_providers()
        .into_iter()
        .find(|row| row.provider == provider)?;
    if !directory.settings_path.is_empty() {
        return None;
    }
    let entry = route_entry(loader.entries(), &directory.settings_ns, package)?;
    let fiber = entry.fiber?;
    (fiber.state() == FiberState::Active)
        .then(|| fiber.effective_config())
        .flatten()
}

fn route_entry(entries: Vec<EntryInfo>, settings_ns: &str, package: &str) -> Option<EntryInfo> {
    entries
        .into_iter()
        .find(|entry| entry.options.id == settings_ns && entry.options.name == package)
}

fn misconfigured(capability: Capability, route: &str, error: &str) -> DeepSeekWebAuthSource {
    let provider = if route == ACCOUNT_PROVIDER {
        "DeepSeek account model provider"
    } else {
        "DeepSeek API-key model provider"
    };
    let credential = if route == ACCOUNT_PROVIDER {
        "the DeepSeek account sign-in"
    } else {
        "that provider's API key"
    };
    DeepSeekWebAuthSource::Misconfigured {
        message: format!(
            "DeepSeek {} API sends {credential} to the origin of the {provider}, and that provider's settings do not resolve: {error}. {} did not send the request. Guide the user to correct that provider's settings on the Models page or in the deployment configuration, or to set {NAME}.{}BaseURL and {NAME}.{}ApiKeyEnv so {} uses its own endpoint.",
            capability.subject(),
            capability.subject(),
            capability.prefix(),
            capability.prefix(),
            capability.subject()
        ),
    }
}

fn missing_message(capability: Capability, rule: Rule, reference: &CredentialRef) -> String {
    let subject = capability.subject();
    let reference = serde_json::Value::String(reference.as_str().to_owned()).to_string();
    let add_key = format!(
        "add the DeepSeek API key on the Models page or export {reference} in the launching environment"
    );
    let billing = format!("{NAME}.{}Billing", capability.prefix());
    let settings = "on the Web search and fetch settings page";
    match rule {
        Rule::AccountConversation => format!(
            "DeepSeek {subject} API uses the DeepSeek account sign-in of this conversation, then the DeepSeek API key, and neither is available: no signed-in DeepSeek account is available, and no key is configured for {reference}. Guide the user to sign in to DeepSeek, or to {add_key}."
        ),
        Rule::OfficialConversation => format!(
            "DeepSeek {subject} API uses the DeepSeek API key, as this conversation's model provider does, and no key is configured for {reference}. Guide the user to {add_key}, or to sign in to DeepSeek and set {billing} to \"account\" {settings}."
        ),
        Rule::OtherConversation => format!(
            "DeepSeek {subject} API uses the DeepSeek API key, then the DeepSeek account sign-in, and neither is available: no key is configured for {reference}, and no signed-in DeepSeek account is available. Guide the user to {add_key}, or to sign in to DeepSeek."
        ),
        Rule::Account => format!(
            "DeepSeek {subject} API bills only the signed-in DeepSeek account, and no signed-in DeepSeek account is available. {subject} did not send the request. Guide the user to sign in to DeepSeek, or to set {billing} to \"api-key\" or \"auto\" {settings}."
        ),
        Rule::ApiKey => format!(
            "DeepSeek {subject} API bills only the DeepSeek API key, and no key is configured for {reference}. Guide the user to {add_key}, or to set {billing} to \"account\" or \"auto\" {settings}."
        ),
    }
}

fn tools_base(messages_base_url: &str) -> Result<String, url::ParseError> {
    Url::parse(messages_base_url)
        .map(|url| format!("{}{TOOLS_PATH}", url.origin().ascii_serialization()))
}
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

fn deep_seek_api_key_ref() -> CredentialRef {
    credential_ref("DEEPSEEK_API_KEY")
        .unwrap_or_else(|_| unreachable!("constant credential reference is valid"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoh_kernel::Kernel;
    use hoh_launch_environment::{
        LaunchEnvironment, LaunchEnvironmentLayerInput, LaunchEnvironmentSource,
        create_launch_environment_snapshot,
    };
    use hoh_loader::{Entry, EntryStatus};
    use hoh_web::{WebFetchRequest, WebRuntime, WebSearchRequest};
    use indexmap::IndexMap;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    #[test]
    fn config_defaults_and_removed_fields() {
        let config: Config =
            serde_json::from_value(serde_json::json!({"baseURL":"ignored","apiKey":"ignored"}))
                .unwrap();
        assert_eq!(config.count.0, 10);
        assert_eq!(config.search_requests_per_second.0, 5);
        assert_eq!(config.search_billing.0, Billing::Auto);
        assert_eq!(config.fetch_billing.0, Billing::Auto);
        assert_eq!(config.search_base_url.0, None);
    }
    #[test]
    fn billing_rejects_unknown_value() {
        assert!(
            serde_json::from_value::<Config>(serde_json::json!({"searchBilling":"session"}))
                .is_err()
        );
    }

    #[test]
    fn config_rejects_invalid_numeric_bounds() {
        assert!(serde_json::from_value::<Config>(serde_json::json!({"count":0})).is_err());
        assert!(serde_json::from_value::<Config>(serde_json::json!({"count":51})).is_err());
        assert!(
            serde_json::from_value::<Config>(serde_json::json!({"searchRequestsPerSecond":0}))
                .is_err()
        );
    }

    #[test]
    fn config_schema_keeps_count_and_rate_bounds() {
        let schema = serde_json::to_value(schemars::schema_for!(Config)).unwrap();
        assert_eq!(schema["properties"]["count"]["minimum"], 1);
        assert_eq!(schema["properties"]["count"]["maximum"], 50);
        assert_eq!(
            schema["properties"]["searchRequestsPerSecond"]["minimum"],
            1
        );
    }

    #[tokio::test]
    async fn invalid_own_credential_ref_is_reported_without_panicking() {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let error = resolve_target(
            &ctx,
            Capability::Search,
            OwnSettings {
                base_url: None,
                key_ref: Some("bad key".into()),
                billing: Billing::Auto,
            },
            None,
        )
        .err()
        .expect("invalid credential ref");
        assert!(error.to_string().contains("must match"));
        kernel.shutdown().await;
    }

    #[tokio::test]
    #[ignore = "requires loopback bind, which the writer sandbox forbids"]
    async fn transport_client_uses_proxy_from_launch_snapshot() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        let proxy_task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = vec![0_u8; 16 * 1024];
            let length = stream.read(&mut bytes).await.unwrap();
            bytes.truncate(length);
            let body = b"{\"ok\":true}";
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        String::from_utf8_lossy(body)
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            String::from_utf8(bytes).unwrap()
        });

        let kernel = Kernel::start();
        let ctx = kernel.root();
        let snapshot = create_launch_environment_snapshot(&[LaunchEnvironmentLayerInput {
            source: LaunchEnvironmentSource::ProjectEnv,
            path: Some(".env".into()),
            values: IndexMap::from([("HTTP_PROXY".into(), proxy)]),
        }]);
        ctx.provide::<LaunchEnvironment>(Arc::new(snapshot))
            .await
            .unwrap();
        let request = DeepSeekWebRequest {
            client: transport_client(&ctx).unwrap(),
            path: "/search".into(),
            body: serde_json::json!({"query":"q"}),
            headers: Default::default(),
            api_key_headers: None,
            decode: Arc::new(Ok),
            guidance: guidance(Capability::Search, true),
            record_request: None,
            wait_to_send: None,
        };
        let result = dispatch_deep_seek_web(
            &DeepSeekWebAuth {
                sources: vec![DeepSeekWebAuthSource::ApiKey {
                    base_url: "http://dsk-probe.invalid/v1/tools".into(),
                    key: DeepSeekWebApiKey::Literal("key".into()),
                }],
                missing_credential_message: None,
            },
            &request,
            None,
        )
        .await
        .unwrap();
        assert_eq!(result, serde_json::json!({"ok":true}));
        let wire = proxy_task.await.unwrap();
        assert!(wire.starts_with("POST http://dsk-probe.invalid/v1/tools/search HTTP/1.1"));
        kernel.shutdown().await;
    }
    #[test]
    fn tools_base_uses_only_messages_origin() {
        assert_eq!(
            tools_base("https://internal.deepseek.test/anthropic").unwrap(),
            "https://internal.deepseek.test/v1/tools"
        );
        assert_eq!(
            tools_base("http://[::1]:9000/messages").unwrap(),
            "http://[::1]:9000/v1/tools"
        );
        assert!(tools_base("not a URL").is_err());
    }

    #[tokio::test]
    async fn billing_rules_preserve_source_order_and_all_missing_diagnostics() {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let kinds = |sources: Vec<DeepSeekWebAuthSource>| {
            sources
                .iter()
                .map(|source| match source {
                    DeepSeekWebAuthSource::Account { .. } => "account",
                    DeepSeekWebAuthSource::ApiKey { .. } => "api-key",
                    DeepSeekWebAuthSource::Misconfigured { .. } => "misconfigured",
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            kinds(rule_sources(&ctx, Capability::Search, Rule::AccountConversation).unwrap()),
            vec!["api-key"]
        );
        assert_eq!(
            kinds(rule_sources(&ctx, Capability::Search, Rule::OfficialConversation).unwrap()),
            vec!["api-key"]
        );
        assert_eq!(
            kinds(rule_sources(&ctx, Capability::Search, Rule::OtherConversation).unwrap()),
            vec!["api-key", "account"]
        );
        assert_eq!(
            kinds(rule_sources(&ctx, Capability::Search, Rule::Account).unwrap()),
            vec!["account"]
        );
        assert_eq!(
            kinds(rule_sources(&ctx, Capability::Search, Rule::ApiKey).unwrap()),
            vec!["api-key"]
        );

        let reference = deep_seek_api_key_ref();
        let messages = [
            missing_message(Capability::Search, Rule::AccountConversation, &reference),
            missing_message(Capability::Search, Rule::OfficialConversation, &reference),
            missing_message(Capability::Search, Rule::OtherConversation, &reference),
            missing_message(Capability::Search, Rule::Account, &reference),
            missing_message(Capability::Search, Rule::ApiKey, &reference),
        ];
        assert!(
            messages[0].contains("account sign-in of this conversation, then the DeepSeek API key")
        );
        assert!(messages[1].contains("as this conversation's model provider does"));
        assert!(messages[2].contains("API key, then the DeepSeek account sign-in"));
        assert!(messages[3].contains("bills only the signed-in DeepSeek account"));
        assert!(messages[4].contains("bills only the DeepSeek API key"));
        assert!(
            messages
                .iter()
                .all(|message| message.contains("DeepSeek Search API"))
        );
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn default_and_misconfigured_route_sources_match_source_contract() {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let official = official_source(&ctx, Capability::Fetch).unwrap();
        assert!(matches!(
            official,
            DeepSeekWebAuthSource::ApiKey { base_url, .. }
                if base_url == DEEPSEEK_WEB_API_BASE_URL
        ));
        let account = account_source(&ctx, Capability::Search, None).unwrap();
        assert!(matches!(
            account,
            DeepSeekWebAuthSource::Account { base_url, .. }
                if base_url.ends_with("/v1/tools")
        ));
        let malformed = account_source(
            &ctx,
            Capability::Search,
            Some(&serde_json::json!({"models":[]})),
        )
        .unwrap();
        assert!(matches!(
            malformed,
            DeepSeekWebAuthSource::Misconfigured { message }
                if message.contains("models is no longer configurable")
        ));
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn empty_launch_environment_key_is_missing_and_is_read_per_operation() {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let snapshot = create_launch_environment_snapshot(&[LaunchEnvironmentLayerInput {
            source: LaunchEnvironmentSource::ProjectEnv,
            path: Some(".env".into()),
            values: IndexMap::from([("DEEPSEEK_API_KEY".into(), String::new())]),
        }]);
        ctx.provide::<LaunchEnvironment>(Arc::new(snapshot))
            .await
            .unwrap();
        let source = api_key_source(&ctx, DEEPSEEK_WEB_API_BASE_URL, deep_seek_api_key_ref());
        let auth = DeepSeekWebAuth {
            sources: vec![source],
            missing_credential_message: Some("missing".into()),
        };
        for _ in 0..2 {
            let request = DeepSeekWebRequest {
                client: transport_client(&ctx).unwrap(),
                path: "/search".into(),
                body: serde_json::json!({}),
                headers: Default::default(),
                api_key_headers: None,
                decode: Arc::new(Ok),
                guidance: guidance(Capability::Search, false),
                record_request: None,
                wait_to_send: None,
            };
            let error = dispatch_deep_seek_web(&auth, &request, None)
                .await
                .unwrap_err();
            assert_eq!(
                hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
                "WEB_PROVIDER_CREDENTIAL_MISSING"
            );
        }
        kernel.shutdown().await;
    }

    #[test]
    fn route_entry_uses_first_matching_loader_row_even_when_inactive() {
        let row = |id: &str| EntryInfo {
            id: id.into(),
            name: "row".into(),
            group: None,
            disabled: false,
            options: Entry {
                id: "llm-deepseek".into(),
                name: OFFICIAL_PACKAGE.into(),
                config: serde_json::json!({"row":id}),
                disabled: serde_json::Value::Null,
                group: false,
                inject: None,
                isolate: None,
                intercept: None,
            },
            fiber: None,
            status: EntryStatus::Unresolved,
        };
        let selected = route_entry(
            vec![row("first"), row("second")],
            "llm-deepseek",
            OFFICIAL_PACKAGE,
        )
        .unwrap();
        assert_eq!(selected.id, "first");
    }

    #[tokio::test]
    async fn plugin_registers_both_provider_descriptions() {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let web = ctx.plugin::<WebRuntime>(serde_json::json!({})).unwrap();
        web.settled().await.unwrap();
        let plugin = ctx.plugin::<WebDeepSeekApi>(serde_json::json!({})).unwrap();
        plugin.settled().await.unwrap();
        let providers = ctx.get::<Web>().unwrap().list_providers();
        assert_eq!(providers.search[0].id, DEEPSEEK_SEARCH_API_PROVIDER_ID);
        assert_eq!(providers.fetch[0].id, DEEPSEEK_FETCH_API_PROVIDER_ID);
        assert!(providers.search[0].description.is_some());
        assert!(providers.fetch[0].description.is_some());
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn plugin_fiber_disposal_unregisters_both_providers() {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let web = ctx
            .plugin::<WebRuntime>(serde_json::json!({
                "searchProvider": DEEPSEEK_SEARCH_API_PROVIDER_ID,
                "fetchProvider": DEEPSEEK_FETCH_API_PROVIDER_ID
            }))
            .unwrap();
        web.settled().await.unwrap();
        let plugin = ctx.plugin::<WebDeepSeekApi>(serde_json::json!({})).unwrap();
        plugin.settled().await.unwrap();
        plugin.dispose().await;
        let providers = ctx.get::<Web>().unwrap().list_providers();
        assert!(providers.search.is_empty());
        assert!(providers.fetch.is_empty());
        let search_error = ctx
            .get::<Web>()
            .unwrap()
            .search(
                WebSearchRequest {
                    query: "q".into(),
                    max_results: None,
                },
                None,
            )
            .await
            .unwrap_err();
        let fetch_error = ctx
            .get::<Web>()
            .unwrap()
            .fetch(
                WebFetchRequest {
                    url: "https://page.test".into(),
                },
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(
            hoh_error::HarnessError::of(search_error.as_ref())
                .unwrap()
                .code(),
            "WEB_PROVIDER_CONFIGURED_MISSING"
        );
        assert_eq!(
            hoh_error::HarnessError::of(fetch_error.as_ref())
                .unwrap()
                .code(),
            "WEB_PROVIDER_CONFIGURED_MISSING"
        );
        kernel.shutdown().await;
    }
}
