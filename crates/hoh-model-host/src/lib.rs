//! Window-engine wiring for an OpenAI-compatible model and its credential.
//!
//! The API key lives only in [`hoh_credentials::Credentials`]. This crate keeps the
//! non-secret model address/name and the first-run rules, and publishes the window RPC handlers.

use hoh_credentials::{Credentials, credential_ref};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_llm::{
    GenerateOptions, Llm, LlmAdapter, LlmFuture, LlmModelInfo, LlmProviderInfo,
    LlmResolvedModelInfo, LlmStream, PreparedAdapterCall,
};
use hoh_llm_openai::{
    ModelSettings as OpenAiModelSettings, OpenAiAdapter, OpenAiConfig, ProbeOptions,
};
use hoh_protocol::{
    Empty, InternetMode, ModelKeySetParams, ModelProbeParams, ModelProbeResult, ModelSettings,
    Permissions, SECRETARY_PROPOSE_METHOD, Settings, SettingsRulesSetParams, SettingsSetParams,
    SetupInternetMode, Theme, ViewSettings,
};
use hoh_sdk_jsonrpc_server::HohRpcMethods;
use hoh_timeout::AbortSignal;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    fmt,
    sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard},
};
use tokio::sync::{Semaphore, watch};

mod secretary;

pub const MODEL_API_KEY_REF: &str = "model_api_key";
pub const DEFAULT_PROVIDER: &str = "openai-compatible";
pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8080/v1";
pub const DEFAULT_MODEL: &str = "local";

fn default_provider() -> String {
    DEFAULT_PROVIDER.into()
}
fn default_provider_name() -> String {
    "OpenAI-compatible".into()
}
fn default_base_url() -> String {
    DEFAULT_BASE_URL.into()
}
fn default_model() -> String {
    DEFAULT_MODEL.into()
}
fn default_connect_timeout_ms() -> u64 {
    10_000
}
fn default_stream_idle_timeout_ms() -> u64 {
    300_000
}

/// Open configuration. It deliberately has no API-key field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
#[schemars(default)]
pub struct ModelHostConfig {
    pub provider: String,
    pub provider_name: String,
    pub base_url: String,
    pub model: String,
    pub connect_timeout_ms: u64,
    pub stream_idle_timeout_ms: u64,
}

impl Default for ModelHostConfig {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            provider_name: default_provider_name(),
            base_url: default_base_url(),
            model: default_model(),
            connect_timeout_ms: default_connect_timeout_ms(),
            stream_idle_timeout_ms: default_stream_idle_timeout_ms(),
        }
    }
}

fn boxed(message: impl Into<String>) -> BoxError {
    message.into().into()
}

fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn normalize_model_settings(settings: ModelSettings) -> Result<ModelSettings, BoxError> {
    if settings.model.trim().is_empty() {
        return Err(boxed("model settings require a non-empty model"));
    }
    let base_url = OpenAiConfig::new(&settings.base_url)
        .normalized_base_url()
        .map_err(|error| boxed(error.to_string()))?;
    Ok(ModelSettings {
        base_url,
        model: settings.model,
    })
}

fn default_settings(model: ModelSettings) -> Settings {
    // GLUE-SHIM(settings): `hoh-settings` is empty, so there is no shared settings aggregator.
    // Only `model` and, through `hoh/settings/rules/set`, `security.internet`/`security.folders`
    // are live; the other `security` fields and `view` stay at these defaults and `settings/set`
    // rejects them until the composition owner mounts the aggregator.
    Settings {
        model,
        security: Permissions {
            internet: InternetMode::Ask,
            domains: Vec::new(),
            folders: Vec::new(),
            write_files: false,
            programs: false,
            program_network: Vec::new(),
            act_as_user: false,
        },
        view: ViewSettings {
            theme: Theme::System,
            font_size: 16,
        },
    }
}

/// The adapter built for one address and key. It owns the HTTP client (connections, certificate
/// verifier) and the learned output-limit spelling, so it is reused until either input changes.
struct CachedAdapter {
    base_url: String,
    api_key: Option<String>,
    adapter: OpenAiAdapter,
}

/// The part of the settings that outlives a restart: what the composition owner saves.
#[derive(Clone, Debug, PartialEq)]
pub struct PersistedSettings {
    pub model: ModelSettings,
    /// `None` until the first run has chosen the rules.
    pub rules: Option<SettingsRulesSetParams>,
}

fn rules_of(settings: &Settings) -> Option<SettingsRulesSetParams> {
    let internet = match settings.security.internet {
        InternetMode::Ask => SetupInternetMode::Ask,
        InternetMode::Public => SetupInternetMode::Public,
        InternetMode::Deny => return None,
    };
    let folder = settings.security.folders.first()?.clone();
    Some(SettingsRulesSetParams { internet, folder })
}

fn persisted_of(settings: &Settings) -> PersistedSettings {
    PersistedSettings {
        model: settings.model.clone(),
        rules: rules_of(settings),
    }
}

struct Inner {
    ctx: Context,
    config: ModelHostConfig,
    settings: RwLock<Settings>,
    adapter: Mutex<Option<CachedAdapter>>,
    changes: watch::Sender<PersistedSettings>,
    secretary_slot: Semaphore,
}

/// Live model host. Its Debug representation contains no credential material.
#[derive(Clone)]
pub struct ModelHost {
    inner: Arc<Inner>,
}

impl fmt::Debug for ModelHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelHost")
            .field("provider", &self.inner.config.provider)
            .field("settings", &self.settings())
            .field("credential", &MODEL_API_KEY_REF)
            .finish()
    }
}

impl ModelHost {
    pub fn new(ctx: Context, mut config: ModelHostConfig) -> Result<Self, BoxError> {
        ctx.ensure_active()?;
        if config.provider.trim().is_empty() {
            return Err(boxed("model host provider must be non-empty"));
        }
        if config.provider_name.trim().is_empty() {
            return Err(boxed("model host providerName must be non-empty"));
        }
        if config.connect_timeout_ms == 0 || config.stream_idle_timeout_ms == 0 {
            return Err(boxed("model host timeouts must be positive"));
        }
        let model = normalize_model_settings(ModelSettings {
            base_url: config.base_url.clone(),
            model: config.model.clone(),
        })?;
        config.base_url.clone_from(&model.base_url);
        config.model.clone_from(&model.model);
        let settings = default_settings(model);
        Ok(Self {
            inner: Arc::new(Inner {
                ctx,
                config,
                changes: watch::channel(persisted_of(&settings)).0,
                settings: RwLock::new(settings),
                adapter: Mutex::new(None),
                secretary_slot: Semaphore::new(1),
            }),
        })
    }

    pub fn provider(&self) -> &str {
        &self.inner.config.provider
    }

    pub fn settings(&self) -> Settings {
        read(&self.inner.settings).clone()
    }

    fn credentials(&self) -> Result<Service<Credentials>, BoxError> {
        self.inner.ctx.ensure_active()?;
        self.inner
            .ctx
            .get::<Credentials>()
            .ok_or_else(|| boxed("model credential service is unavailable"))
    }

    async fn resolve_key(&self) -> Result<Option<String>, BoxError> {
        let credentials = self.credentials()?;
        let reference = credential_ref(MODEL_API_KEY_REF)?;
        let resolved = credentials
            .resolve(&reference)
            .await
            .map_err(|_| boxed("model credential could not be resolved"))?;
        self.inner.ctx.ensure_active()?;
        resolved
            .map(|resolved| {
                hoh_llm::assert_usable_api_key(&resolved.value, "hoh-model-host", MODEL_API_KEY_REF)
                    .map_err(Into::into)
            })
            .transpose()
    }

    /// The adapter for the current address and key, with the model name of the same settings
    /// snapshot: a call prepared for one address must not go out with the name saved for another.
    async fn adapter(&self) -> Result<(OpenAiAdapter, String), BoxError> {
        self.inner.ctx.ensure_active()?;
        let ModelSettings { base_url, model } = self.settings().model;
        let api_key = self.resolve_key().await?;
        self.inner.ctx.ensure_active()?;
        let mut cache = self
            .inner
            .adapter
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(cached) = cache
            .as_ref()
            .filter(|cached| cached.base_url == base_url && cached.api_key == api_key)
        {
            return Ok((cached.adapter.clone(), model));
        }
        let mut config = OpenAiConfig::new(base_url.clone());
        config.api_key.clone_from(&api_key);
        config.provider_name = self.inner.config.provider_name.clone();
        config.model = OpenAiModelSettings::default();
        config.connect_timeout_ms = self.inner.config.connect_timeout_ms;
        config.stream_idle_timeout_ms = self.inner.config.stream_idle_timeout_ms;
        let adapter = OpenAiAdapter::new(config)?;
        *cache = Some(CachedAdapter {
            base_url,
            api_key,
            adapter: adapter.clone(),
        });
        Ok((adapter, model))
    }

    pub async fn set_key(&self, params: ModelKeySetParams) -> Result<Empty, BoxError> {
        self.inner.ctx.ensure_active()?;
        // Validate the address even though it is approval context rather than persisted state.
        OpenAiConfig::new(&params.base_url)
            .normalized_base_url()
            .map_err(|error| boxed(error.to_string()))?;
        // The window shows this text: say what is wrong, never echo the key.
        if params.api_key.expose().trim().is_empty() {
            return Err(boxed("model credential must contain 1..8192 bytes"));
        }
        let normalized = hoh_llm::assert_usable_api_key(
            params.api_key.expose(),
            "hoh-model-host",
            MODEL_API_KEY_REF,
        )
        .map_err(|_| boxed("model credential contains characters an HTTP header cannot carry"))?;
        if normalized.len() > 8192 {
            return Err(boxed("model credential must contain 1..8192 bytes"));
        }
        let credentials = self.credentials()?;
        let reference = credential_ref(MODEL_API_KEY_REF)?;
        credentials
            .set(&reference, &normalized)
            .await
            // A provider is arbitrary code and may echo the input in its error chain.
            .map_err(|_| boxed("model credential could not be stored"))?;
        // No `ensure_active` here: the key is stored, reporting failure would mislead the window.
        Ok(Empty {})
    }

    /// An empty `model` asks for the list of models only (the window's first run does this): the
    /// one-token completion then goes to the first listed model, else to the configured one, and
    /// `GET /models` is requested once. Dropping the plugin aborts the requests in flight.
    pub async fn probe(&self, params: ModelProbeParams) -> Result<ModelProbeResult, BoxError> {
        self.inner.ctx.ensure_active()?;
        let key = self.resolve_key().await?;
        let signal = AbortSignal::from_token(self.inner.ctx.token());
        let options = ProbeOptions::default();
        let model = params.model.trim();
        let result = if model.is_empty() {
            hoh_llm_openai::probe_first_listed(
                &params.base_url,
                &self.settings().model.model,
                key.as_deref(),
                &options,
                Some(&signal),
            )
            .await
        } else {
            hoh_llm_openai::probe(
                &params.base_url,
                model,
                key.as_deref(),
                &options,
                Some(&signal),
            )
            .await
        };
        self.inner.ctx.ensure_active()?;
        Ok(ModelProbeResult {
            ok: result.ok,
            latency_ms: result.latency_ms,
            tool_calls: result.tool_calls,
            models: result.models,
        })
    }

    /// The first-run rules of the window: the internet mode and the one working folder. Written
    /// into `security.internet` / `security.folders` of the settings, as the window stub does.
    pub fn set_rules(&self, params: SettingsRulesSetParams) -> Result<Empty, BoxError> {
        self.inner.ctx.ensure_active()?;
        // Trimmed only to refuse a blank folder: the path itself is stored as the user allowed it.
        let folder = params.folder;
        if folder.trim().is_empty() || folder.len() > 4096 || folder.contains('\0') {
            return Err(boxed("rules folder must contain 1..4096 bytes and no NUL"));
        }
        let internet = match params.internet {
            SetupInternetMode::Ask => InternetMode::Ask,
            SetupInternetMode::Public => InternetMode::Public,
        };
        let mut settings = write(&self.inner.settings);
        settings.security.internet = internet;
        settings.security.folders = vec![folder];
        self.publish(&settings);
        Ok(Empty {})
    }

    /// What `set_rules` stored, `None` until the first run has chosen. The composition owner
    /// saves this next to the model settings and replays it with `set_rules` after a restart.
    pub fn rules(&self) -> Option<SettingsRulesSetParams> {
        rules_of(&read(&self.inner.settings))
    }

    fn publish(&self, settings: &Settings) {
        let persisted = persisted_of(settings);
        self.inner.changes.send_if_modified(|current| {
            let changed = *current != persisted;
            if changed {
                *current = persisted;
            }
            changed
        });
    }

    /// Subscribe to changes of what must outlive a restart (model address/name, first-run rules).
    /// The receiver holds the current value at once and is notified after each `settings/set` or
    /// `rules/set` that really changed it; several quick changes may be seen as one (`watch`).
    pub fn subscribe(&self) -> watch::Receiver<PersistedSettings> {
        self.inner.changes.subscribe()
    }

    pub fn get_settings(&self) -> Result<Settings, BoxError> {
        self.inner.ctx.ensure_active()?;
        Ok(self.settings())
    }

    pub fn set_settings(&self, params: SettingsSetParams) -> Result<Settings, BoxError> {
        self.inner.ctx.ensure_active()?;
        if params.security.is_some() || params.view.is_some() {
            return Err(boxed(
                "model host cannot apply non-model settings before settings aggregation is mounted",
            ));
        }
        let Some(model) = params.model else {
            return Ok(self.settings());
        };
        let model = normalize_model_settings(model)?;
        self.inner.ctx.ensure_active()?;
        let mut settings = write(&self.inner.settings);
        settings.model = model;
        self.publish(&settings);
        Ok(settings.clone())
    }

    /// Run a model-host RPC method. Host approval of the privileged ones
    /// (`hoh/model/key/set`, `hoh/model/probe` with an address, `hoh/settings/set` with `model` or
    /// `security`, `hoh/settings/rules/set`) is the SDK server's job; it must have granted it
    /// before the table calls the handler.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, BoxError> {
        self.inner.ctx.ensure_active()?;
        let value = match method {
            METHOD_PROBE => serde_json::to_value(self.probe(parse(method, params)?).await?)?,
            SECRETARY_PROPOSE_METHOD => self.propose_as_secretary(params).await?,
            METHOD_KEY_SET => serde_json::to_value(self.set_key(parse(method, params)?).await?)?,
            METHOD_SETTINGS_GET => {
                let _: Empty = parse(method, params)?;
                serde_json::to_value(self.get_settings()?)?
            }
            METHOD_RULES_SET => serde_json::to_value(self.set_rules(parse(method, params)?)?)?,
            METHOD_SETTINGS_SET => {
                serde_json::to_value(self.set_settings(parse(method, params)?)?)?
            }
            _ => return Err(boxed(format!("model host does not serve {method}"))),
        };
        Ok(value)
    }
}

fn parse<T: DeserializeOwned>(method: &str, value: Value) -> Result<T, BoxError> {
    // serde's text quotes the offending value, and a key must never reach an error message.
    serde_json::from_value(value).map_err(|_| boxed(format!("invalid {method} params")))
}

pub const METHOD_PROBE: &str = "hoh/model/probe";
pub const METHOD_KEY_SET: &str = "hoh/model/key/set";
pub const METHOD_SETTINGS_GET: &str = "hoh/settings/get";
pub const METHOD_SETTINGS_SET: &str = "hoh/settings/set";
pub const METHOD_RULES_SET: &str = "hoh/settings/rules/set";
const METHODS: &[&str] = &[
    METHOD_PROBE,
    SECRETARY_PROPOSE_METHOD,
    METHOD_KEY_SET,
    METHOD_SETTINGS_GET,
    METHOD_SETTINGS_SET,
    METHOD_RULES_SET,
];

hoh_kernel::service!(pub ModelHostKey: ModelHost = "modelHost");

#[derive(Clone)]
struct HostedOpenAiAdapter {
    host: Arc<ModelHost>,
}

impl fmt::Debug for HostedOpenAiAdapter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostedOpenAiAdapter")
            .field("host", &self.host)
            .finish()
    }
}

impl LlmAdapter for HostedOpenAiAdapter {
    fn provider_info(&self, provider: &str) -> Result<LlmProviderInfo, BoxError> {
        self.host.inner.ctx.ensure_active()?;
        Ok(LlmProviderInfo {
            id: provider.into(),
            name: self.host.inner.config.provider_name.clone(),
        })
    }

    fn list_models<'a>(&'a self, provider: &'a str) -> LlmFuture<'a, Vec<LlmModelInfo>> {
        Box::pin(async move {
            let (adapter, _) = self.host.adapter().await?;
            adapter.list_models(provider).await
        })
    }

    fn resolve_model<'a>(
        &'a self,
        provider: &'a str,
        model: &'a str,
        signal: Option<&'a hoh_timeout::AbortSignal>,
    ) -> LlmFuture<'a, LlmResolvedModelInfo> {
        Box::pin(async move {
            let (adapter, _) = self.host.adapter().await?;
            adapter.resolve_model(provider, model, signal).await
        })
    }

    fn prepare_call(
        self: Arc<Self>,
        provider: String,
        model: String,
        signal: Option<hoh_timeout::AbortSignal>,
    ) -> LlmFuture<'static, PreparedAdapterCall> {
        Box::pin(async move {
            let (adapter, saved_model) = self.host.adapter().await?;
            let adapter = Arc::new(adapter);
            let mut prepared = adapter.prepare_call(provider, model, signal).await?;
            // The window announces a placeholder model in `initialize` (docs/window.md, native.rs)
            // and the session keeps that route; the name that goes on the wire is the saved setting
            // taken together with the address this call was prepared for, not read again at send time.
            let dispatch = prepared.dispatch.clone();
            prepared.dispatch = Arc::new(move |mut options| {
                options.model.clone_from(&saved_model);
                dispatch(options)
            });
            Ok(prepared)
        })
    }

    fn stream(&self, _options: GenerateOptions) -> Result<LlmStream, BoxError> {
        Err(boxed(
            "model host adapter must be called through prepare_call so credentials can be resolved",
        ))
    }
}

#[derive(Inject)]
pub struct Deps {
    llm: Service<Llm>,
    _credentials: Service<Credentials>,
    methods: Service<HohRpcMethods>,
}

pub struct ModelHostPlugin;

impl Plugin for ModelHostPlugin {
    const NAME: &'static str = "model-host";
    type Config = ModelHostConfig;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Self::Config>) -> Result<(), BoxError> {
        let host = Arc::new(ModelHost::new(ctx.clone(), config.get().as_ref().clone())?);
        deps.llm.register_adapter(
            &ctx,
            vec![host.provider().into()],
            Arc::new(HostedOpenAiAdapter { host: host.clone() }),
        )?;
        for &method in METHODS {
            let host = host.clone();
            deps.methods.register(&ctx, method, move |params| {
                let host = host.clone();
                async move { host.call(method, params).await }
            })?;
        }
        ctx.provide::<ModelHostKey>(host).await?;
        Ok(())
    }
}
