//! Provider selection for the independent search and fetch capabilities of `web`.
pub mod types;
pub use types::*;

use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Disposer, ListenerFailure, Plugin, Undo, Volatile};
use hoh_timeout::AbortSignal;
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, MutexGuard};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Each operation reads the current snapshot; changing either field retains the service.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(extend("default" = serde_json::json!({})))]
pub struct WebRuntimeConfig {
    #[serde(default, skip_serializing_if = "provider_unset")]
    pub search_provider: Volatile<Option<String>>,
    #[serde(default, skip_serializing_if = "provider_unset")]
    pub fetch_provider: Volatile<Option<String>>,
}
fn provider_unset(value: &Volatile<Option<String>>) -> bool {
    value.0.is_none()
}

type Registry<P> = Arc<Mutex<IndexMap<String, Arc<P>>>>;

#[derive(Clone)]
pub struct WebRuntime {
    search_providers: Registry<dyn WebSearchProvider>,
    fetch_providers: Registry<dyn WebFetchProvider>,
    config: Cfg<WebRuntimeConfig>,
    context: Context,
    search_provider_env: Option<String>,
    fetch_provider_env: Option<String>,
}

hoh_kernel::service!(pub Web: WebRuntime = "web");

impl Plugin for WebRuntime {
    const NAME: &'static str = "WebRuntime";
    type Config = WebRuntimeConfig;
    type Deps = ();

    async fn apply(ctx: Context, _: (), config: Cfg<Self::Config>) -> Result<(), BoxError> {
        let runtime = Self::new(&ctx, config);
        ctx.provide::<Web>(Arc::new(runtime)).await?;
        Ok(())
    }
}

impl WebRuntime {
    pub fn new(ctx: &Context, config: Cfg<WebRuntimeConfig>) -> Self {
        Self::with_environment(
            ctx,
            config,
            std::env::var_os("DSH_WEB_SEARCH_PROVIDER").map(|s| s.to_string_lossy().into_owned()),
            std::env::var_os("DSH_WEB_FETCH_PROVIDER").map(|s| s.to_string_lossy().into_owned()),
        )
    }

    /// Explicit constructor inputs for embedding and tests without process environment mutation.
    /// The normal plugin snapshots both environment variables in `new`.
    pub fn with_environment(
        ctx: &Context,
        config: Cfg<WebRuntimeConfig>,
        search_provider_env: Option<String>,
        fetch_provider_env: Option<String>,
    ) -> Self {
        Self {
            search_providers: Arc::default(),
            fetch_providers: Arc::default(),
            config,
            context: ctx.clone(),
            search_provider_env,
            fetch_provider_env,
        }
    }

    pub fn register_search_provider(
        &self,
        ctx: &Context,
        provider: Arc<dyn WebSearchProvider>,
    ) -> Result<Disposer, BoxError> {
        self.register_provider(ctx, &self.search_providers, provider)
    }

    pub fn register_fetch_provider(
        &self,
        ctx: &Context,
        provider: Arc<dyn WebFetchProvider>,
    ) -> Result<Disposer, BoxError> {
        self.register_provider(ctx, &self.fetch_providers, provider)
    }

    fn register_provider<P: Provider + ?Sized + 'static>(
        &self,
        ctx: &Context,
        store: &Registry<P>,
        provider: Arc<P>,
    ) -> Result<Disposer, BoxError> {
        let id = provider.info().id;
        // Source checks duplicates before asking the caller context to create an effect.
        if lock(store).contains_key(&id) {
            return Err(duplicate_provider(&id).into());
        }
        let store_undo = store.clone();
        let runtime = self.clone();
        ctx.try_effect::<BoxError>("web.registerProvider()", |undos| {
            {
                let mut store = lock(store);
                if store.contains_key(&id) {
                    return Err(duplicate_provider(&id).into());
                }
                store.insert(id.clone(), provider);
            }
            undos.push(Undo::sync(move || {
                lock(&store_undo).shift_remove(&id);
                runtime.emit_providers_changed();
            }))?;
            self.emit_providers_changed();
            Ok(())
        })
    }

    /// Remote: web.listProviders
    pub fn list_providers(&self) -> WebProviderList {
        let search: Vec<_> = lock(&self.search_providers).values().cloned().collect();
        let fetch: Vec<_> = lock(&self.fetch_providers).values().cloned().collect();
        WebProviderList {
            search: search.iter().map(|p| p.info()).collect(),
            fetch: fetch.iter().map(|p| p.info()).collect(),
        }
    }

    fn emit_providers_changed(&self) {
        let logger = self.context.logger().named("web");
        // Raw Cordis dispatch does not apply a registrant's receiver filter.
        self.context
            .root()
            .dispatch(WebProvidersChanged)
            .invoke(move |failure| {
                let error = match failure {
                    ListenerFailure::Threw(error) | ListenerFailure::Rejected(error) => error,
                };
                logger.warn("web: a web/providers-changed listener failed");
                logger.warn(error);
            });
    }

    fn select_search_provider(&self) -> Result<Arc<dyn WebSearchProvider>, BoxError> {
        let config = self.config.get();
        resolve_provider(
            config
                .search_provider
                .0
                .as_deref()
                .or(self.search_provider_env.as_deref()),
            &self.search_providers,
        )
    }

    fn select_fetch_provider(&self) -> Result<Arc<dyn WebFetchProvider>, BoxError> {
        let config = self.config.get();
        resolve_provider(
            config
                .fetch_provider
                .0
                .as_deref()
                .or(self.fetch_provider_env.as_deref()),
            &self.fetch_providers,
        )
    }

    pub async fn search(
        &self,
        request: WebSearchRequest,
        signal: Option<&AbortSignal>,
    ) -> Result<WebSearchResult, BoxError> {
        let max_results = request.max_results;
        let mut result = self
            .select_search_provider()?
            .search(request, signal)
            .await?;
        if let Some(max_results) = max_results
            && result.sources.len() as u64 > max_results
        {
            // The comparison proves max_results fits usize on this platform.
            result.sources.truncate(max_results as usize);
            result.truncated = true;
        }
        Ok(result)
    }

    pub async fn fetch(
        &self,
        request: WebFetchRequest,
        signal: Option<&AbortSignal>,
    ) -> Result<WebFetchResult, BoxError> {
        self.select_fetch_provider()?.fetch(request, signal).await
    }

    pub async fn search_ready(&self, llm_provider: Option<&str>) -> Result<bool, BoxError> {
        let Ok(provider) = self.select_search_provider() else {
            return Ok(true);
        };
        provider.ready(llm_provider).await
    }

    pub async fn fetch_ready(&self, llm_provider: Option<&str>) -> Result<bool, BoxError> {
        let Ok(provider) = self.select_fetch_provider() else {
            return Ok(true);
        };
        provider.ready(llm_provider).await
    }
}

fn duplicate_provider(id: &str) -> hoh_error::HarnessError {
    WebError::new(
        format!("a web provider with id \"{id}\" is already registered"),
        "WEB_DUPLICATE_PROVIDER",
    )
}

trait Provider: Send + Sync {
    fn info(&self) -> WebProviderInfo;
    fn available(&self) -> Result<bool, BoxError>;
}
impl Provider for dyn WebSearchProvider {
    fn info(&self) -> WebProviderInfo {
        WebSearchProvider::info(self)
    }
    fn available(&self) -> Result<bool, BoxError> {
        WebSearchProvider::available(self)
    }
}
impl Provider for dyn WebFetchProvider {
    fn info(&self) -> WebProviderInfo {
        WebFetchProvider::info(self)
    }
    fn available(&self) -> Result<bool, BoxError> {
        WebFetchProvider::available(self)
    }
}

fn resolve_provider<P: Provider + ?Sized>(
    configured_id: Option<&str>,
    providers: &Registry<P>,
) -> Result<Arc<P>, BoxError> {
    if let Some(id) = configured_id {
        let provider = lock(providers).get(id).cloned().ok_or_else(|| {
            WebError::new(
                format!("configured web provider \"{id}\" is not registered"),
                "WEB_PROVIDER_CONFIGURED_MISSING",
            )
        })?;
        if !provider.available()? {
            return Err(WebError::new(
                format!("configured web provider \"{id}\" is registered but unavailable"),
                "WEB_PROVIDER_CONFIGURED_UNAVAILABLE",
            )
            .into());
        }
        return Ok(provider);
    }
    let snapshot: Vec<_> = lock(providers).values().cloned().collect();
    let mut usable = Vec::new();
    for provider in snapshot {
        if provider.available()? {
            usable.push(provider);
        }
    }
    if usable.is_empty() {
        return Err(WebError::new(
            "no usable web provider is registered",
            "WEB_PROVIDER_UNAVAILABLE",
        )
        .into());
    }
    if usable.len() > 1 {
        let ids = usable
            .iter()
            .map(|p| p.info().id)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(WebError::new(
            format!(
                "multiple usable web providers are registered ({ids}); configure one explicitly"
            ),
            "WEB_PROVIDER_AMBIGUOUS",
        )
        .into());
    }
    // The empty and multiple-provider branches have returned.
    Ok(usable.pop().expect("exactly one usable provider"))
}
