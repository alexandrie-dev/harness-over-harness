use crate::{
    Config, NAME, ResolvedDeepSeekOptions,
    config::{ListingSettings, plain_options, resolve_adapter_options},
    listing::{
        CatalogRefusal, ListedCatalog, ModelListing, refusal_error, request_model_listing,
        resolve_listed_catalog,
    },
    shim,
};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service, VolatileUpdate};
use hoh_llm::{
    Llm, LlmAdapter, LlmConfigurableProvider, LlmFuture, LlmModelInfo, LlmProviderInfo,
    LlmResolvedModelInfo, LlmStream, ModelsUpdated, PreparedAdapterCall, ResolvedRetryPolicy,
};
use hoh_timeout::{AbortController, AbortSignal};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};
use tokio::sync::{Notify, mpsc, watch};

const PROVIDER: &str = "deepseek-official";

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Default)]
struct CacheState {
    scope: Option<String>,
    listing: Option<ModelListing>,
    settled_at: Option<Instant>,
    inflight: Option<watch::Receiver<bool>>,
    request: Option<Arc<AbortController>>,
    generation: u64,
    failing: bool,
}

struct ListingView {
    authenticated: bool,
    listing: Option<ModelListing>,
}

struct OfficialConnection {
    connection: ResolvedDeepSeekOptions,
    offered: Vec<crate::DeepSeekCatalogModel>,
    refusals: HashMap<String, CatalogRefusal>,
    badges: HashMap<String, hoh_llm::LlmModelBadge>,
}

struct DeepSeekRoute {
    ctx: Context,
    config: Cfg<Config>,
    cache: Arc<Mutex<CacheState>>,
    reported: Mutex<Option<ModelListing>>,
    refresh_wakeup: Arc<Notify>,
    loader: Arc<ListingLoader>,
    load_sender: mpsc::UnboundedSender<LoadJob>,
}

type ListingLoad = Pin<Box<dyn Future<Output = Result<ModelListing, BoxError>> + Send>>;
type ListingLoader =
    dyn Fn(String, String, f64, AbortSignal) -> ListingLoad + Send + Sync + 'static;

#[cfg(test)]
#[derive(Clone)]
struct ListingLoaderTag(Arc<ListingLoader>);

struct LoadJob {
    generation: u64,
    scope: String,
    settings: ListingSettings,
    credential: String,
    request: Arc<AbortController>,
    sender: watch::Sender<bool>,
}

impl DeepSeekRoute {
    fn options(&self) -> Result<ResolvedDeepSeekOptions, BoxError> {
        resolve_adapter_options(
            &plain_options(&self.config.get()),
            Some(&hoh_launch_environment::launch_environment_of(&self.ctx)),
        )
        .map_err(|error| Box::new(error) as BoxError)
    }

    async fn present_api_key(
        &self,
        connection: &ResolvedDeepSeekOptions,
    ) -> Result<Option<String>, BoxError> {
        shim::resolve_deepseek_api_key(&self.ctx, &connection.api_key_env).await
    }

    async fn view(
        &self,
        connection: &ResolvedDeepSeekOptions,
        settings: &ListingSettings,
        signal: Option<&AbortSignal>,
        notify_auth_loss: bool,
        wait_for_first: bool,
    ) -> Result<ListingView, BoxError> {
        let credential = self.present_api_key(connection).await?;
        if signal.is_some_and(AbortSignal::aborted) {
            return Err(signal
                .and_then(AbortSignal::reason)
                .expect("aborted signal")
                .into());
        }
        let Some(credential) = credential else {
            let was_authenticated = {
                let mut state = lock(&self.cache);
                let was_authenticated = state.scope.is_some();
                Self::rescope(&mut state, None);
                was_authenticated
            };
            if was_authenticated && notify_auth_loss {
                self.ctx.emit(&ModelsUpdated {
                    provider: PROVIDER.into(),
                });
            }
            return Ok(ListingView {
                authenticated: false,
                listing: None,
            });
        };
        let digest = Sha256::digest(credential.as_bytes());
        let scope = format!(
            "{}\n{}",
            settings.url,
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let (first, stale) = {
            let mut state = lock(&self.cache);
            if state.scope.as_deref() != Some(&scope) {
                Self::rescope(&mut state, Some(scope.clone()));
            }
            let first = state.settled_at.is_none();
            let stale = !first
                && (settings.max_age_ms == 0.0
                    || state.settled_at.is_some_and(|at| {
                        at.elapsed().as_secs_f64() * 1000.0 >= settings.max_age_ms
                    }));
            (first, stale)
        };
        if first || stale {
            let mut completion = self.ensure_load(&scope, settings, &credential)?;
            if first && wait_for_first {
                let wait = async {
                    while !*completion.borrow() {
                        completion.changed().await.map_err(|_| {
                            Box::new(hoh_error::HarnessError::new(
                                "DeepSeek model listing load ended without a result",
                                "TRANSPORT",
                            )) as BoxError
                        })?;
                    }
                    Ok::<(), BoxError>(())
                };
                match signal {
                    Some(signal) => tokio::select! {
                        biased;
                        reason = signal.cancelled() => return Err(reason.into()),
                        result = wait => result?,
                    },
                    None => wait.await?,
                }
            }
        }
        let state = lock(&self.cache);
        Ok(ListingView {
            authenticated: true,
            listing: state.listing.clone(),
        })
    }

    fn rescope(state: &mut CacheState, scope: Option<String>) {
        if let Some(request) = state.request.take() {
            request.abort();
        }
        state.scope = scope;
        state.listing = None;
        state.settled_at = None;
        state.inflight = None;
        state.generation = state.generation.wrapping_add(1);
        state.failing = false;
    }

    fn ensure_load(
        &self,
        scope: &str,
        settings: &ListingSettings,
        credential: &str,
    ) -> Result<watch::Receiver<bool>, BoxError> {
        let (completion, generation, request) = {
            let mut state = lock(&self.cache);
            if let Some(completion) = &state.inflight {
                return Ok(completion.clone());
            }
            state.generation = state.generation.wrapping_add(1);
            let generation = state.generation;
            let request = Arc::new(AbortController::new());
            let (sender, completion) = watch::channel(false);
            state.inflight = Some(completion.clone());
            state.request = Some(request.clone());
            ((sender, completion), generation, request)
        };
        let (sender, receiver) = completion;
        if self
            .load_sender
            .send(LoadJob {
                generation,
                scope: scope.to_owned(),
                settings: settings.clone(),
                credential: credential.to_owned(),
                request,
                sender,
            })
            .is_err()
        {
            let mut state = lock(&self.cache);
            if state.generation == generation {
                state.inflight = None;
                state.request = None;
            }
            return Err(Box::new(hoh_error::HarnessError::new(
                "DeepSeek model listing driver is unavailable",
                "TRANSPORT",
            )));
        }
        Ok(receiver)
    }

    async fn run_load(&self, job: LoadJob, lifecycle: AbortSignal) {
        let request_signal = AbortSignal::any([job.request.signal(), lifecycle]);
        let loaded = (self.loader)(
            job.settings.url.clone(),
            job.credential,
            job.settings.timeout_ms,
            request_signal.clone(),
        )
        .await;
        let failure = loaded.as_ref().err().map(ToString::to_string);
        let mut changed = false;
        let mut diagnostic: Option<(bool, String)> = None;
        {
            let mut state = lock(&self.cache);
            if state.generation == job.generation && state.scope.as_deref() == Some(&job.scope) {
                state.settled_at = Some(Instant::now());
                state.inflight = None;
                state.request = None;
                match loaded {
                    Ok(listing) => {
                        changed = state.listing.as_ref() != Some(&listing);
                        diagnostic = Some((
                            false,
                            format!(
                                "[{NAME}] model listing from {} loaded {} model(s), skipped {} row(s)",
                                job.settings.url,
                                listing.entries.len(),
                                listing.skipped
                            ),
                        ));
                        state.listing = Some(listing);
                        state.failing = false;
                    }
                    Err(_) => {
                        if !request_signal.aborted() && !state.failing {
                            let fallback = if state.listing.is_some() {
                                "keeping the previous catalog"
                            } else {
                                "using the bundled catalog"
                            };
                            diagnostic = Some((
                                true,
                                format!(
                                    "[{NAME}] model listing from {} failed ({}); {fallback}",
                                    job.settings.url,
                                    failure.as_deref().unwrap_or("unknown failure")
                                ),
                            ));
                        }
                        state.failing = true;
                    }
                }
            }
        }
        if let Some((warning, message)) = diagnostic {
            if warning {
                self.ctx.logger().warn(message);
            } else {
                self.ctx.logger().info(message);
            }
        }
        if changed {
            self.ctx.emit(&ModelsUpdated {
                provider: PROVIDER.into(),
            });
        }
        job.sender.send_replace(true);
        self.refresh_wakeup.notify_one();
    }

    fn connection_of(
        &self,
        mut connection: ResolvedDeepSeekOptions,
        listing: Option<&ModelListing>,
    ) -> OfficialConnection {
        let ListedCatalog {
            offered,
            resolution,
            refusals,
            badges,
            problems,
        } = resolve_listed_catalog(listing, &connection.models, connection.max_tokens);
        connection.models = resolution;
        if listing != lock(&self.reported).as_ref() {
            *lock(&self.reported) = listing.cloned();
            for problem in problems {
                self.ctx.logger().warn(format_args!("[{NAME}] {problem}"));
            }
        }
        OfficialConnection {
            connection,
            offered,
            refusals,
            badges,
        }
    }

    async fn connected(
        &self,
        signal: Option<&AbortSignal>,
    ) -> Result<OfficialConnection, BoxError> {
        let connection = self.options()?;
        let Some(settings) = connection.listing.clone() else {
            return Ok(self.connection_of(connection, None));
        };
        match self.view(&connection, &settings, signal, false, true).await {
            Ok(view) => Ok(self.connection_of(connection, view.listing.as_ref())),
            Err(_error) if !signal.is_some_and(AbortSignal::aborted) => {
                Ok(self.connection_of(connection, None))
            }
            Err(error) => Err(error),
        }
    }

    fn invalidate(&self) {
        let mut state = lock(&self.cache);
        let scope = state.scope.clone();
        Self::rescope(&mut state, scope);
        self.refresh_wakeup.notify_one();
    }

    fn next_refresh_delay(&self) -> Option<Duration> {
        let options = self.options().ok()?;
        let settings = options.listing?;
        if settings.max_age_ms == 0.0 {
            return None;
        }
        let state = lock(&self.cache);
        if state.inflight.is_some() {
            return None;
        }
        let settled_at = state.settled_at?;
        drop(state);
        let age = settled_at.elapsed().as_secs_f64() * 1000.0;
        Some(Duration::from_millis(
            (settings.max_age_ms - age).max(1.0) as u64
        ))
    }

    async fn background_refresh(&self) {
        let options = match self.options() {
            Ok(options) => options,
            Err(error) => {
                self.ctx.logger().warn(format_args!(
                    "[{NAME}] model listing refresh failed ({error}); keeping the previous catalog; check listing configuration and credentials"
                ));
                lock(&self.cache).settled_at = Some(Instant::now());
                return;
            }
        };
        let Some(settings) = options.listing.clone() else {
            self.invalidate();
            return;
        };
        // This task is also the only load-job driver. Enqueue a first load and
        // return to its select loop instead of waiting for itself to run it.
        if let Err(error) = self.view(&options, &settings, None, true, false).await {
            let should_warn = {
                let mut state = lock(&self.cache);
                let should_warn = !state.failing;
                state.failing = true;
                state.settled_at = Some(Instant::now());
                should_warn
            };
            if should_warn {
                self.ctx.logger().warn(format_args!(
                    "[{NAME}] model listing refresh failed ({error}); keeping the previous catalog; check listing configuration and credentials"
                ));
            }
        }
    }
}

impl LlmAdapter for DeepSeekRoute {
    fn provider_info(&self, provider: &str) -> Result<LlmProviderInfo, BoxError> {
        Ok(LlmProviderInfo {
            id: provider.into(),
            name: "DeepSeek".into(),
        })
    }

    fn provider_retry_policy(
        &self,
        _provider: &str,
    ) -> Result<Option<ResolvedRetryPolicy>, BoxError> {
        Ok(Some(self.options()?.retry_policy))
    }

    fn list_models<'a>(&'a self, provider: &'a str) -> LlmFuture<'a, Vec<LlmModelInfo>> {
        Box::pin(async move {
            let connection = self.options()?;
            let Some(settings) = connection.listing.clone() else {
                if self.present_api_key(&connection).await?.is_none() {
                    return Ok(vec![]);
                }
                return Ok(connection
                    .models
                    .iter()
                    .map(|model| shim::catalog_model_info(provider, model, None))
                    .collect());
            };
            let view = self.view(&connection, &settings, None, false, true).await?;
            if !view.authenticated {
                return Ok(vec![]);
            }
            let resolved = self.connection_of(connection, view.listing.as_ref());
            Ok(resolved
                .offered
                .iter()
                .map(|model| {
                    shim::catalog_model_info(provider, model, resolved.badges.get(&model.id))
                })
                .collect())
        })
    }

    fn resolve_model<'a>(
        &'a self,
        provider: &'a str,
        model: &'a str,
        signal: Option<&'a AbortSignal>,
    ) -> LlmFuture<'a, LlmResolvedModelInfo> {
        Box::pin(async move {
            let connection = self.connected(signal).await?;
            Ok(shim::model_info(&connection.connection, provider, model))
        })
    }

    fn prepare_call(
        self: Arc<Self>,
        provider: String,
        model: String,
        signal: Option<AbortSignal>,
    ) -> LlmFuture<'static, PreparedAdapterCall> {
        Box::pin(async move {
            let connection = self.connected(signal.as_ref()).await?;
            if let Some(refusal) = connection.refusals.get(&model) {
                return Err(Box::new(refusal_error(&model, refusal)) as BoxError);
            }
            let model_info = shim::model_info(&connection.connection, &provider, &model);
            let prepared_connection = connection.connection;
            Ok(PreparedAdapterCall {
                model: model_info,
                dispatch: Arc::new(move |options| {
                    self.dispatch_with_connection(options, &prepared_connection)
                }),
            })
        })
    }

    fn stream(&self, options: hoh_llm::GenerateOptions) -> Result<LlmStream, BoxError> {
        let connection = self.options()?;
        let listing = connection
            .listing
            .as_ref()
            .and_then(|_| lock(&self.cache).listing.clone());
        let connection = self.connection_of(connection, listing.as_ref());
        if let Some(refusal) = connection.refusals.get(&options.model) {
            return Err(Box::new(refusal_error(&options.model, refusal)) as BoxError);
        }
        self.dispatch_with_connection(options, &connection.connection)
    }
}

impl DeepSeekRoute {
    fn dispatch_with_connection(
        &self,
        _options: hoh_llm::GenerateOptions,
        _connection: &ResolvedDeepSeekOptions,
    ) -> Result<LlmStream, BoxError> {
        Err(shim::unavailable_transport())
    }
}

#[derive(Inject)]
pub struct Deps {
    pub llm: Service<Llm>,
}

pub struct DeepSeekApiKey;

impl Plugin for DeepSeekApiKey {
    const NAME: &'static str = NAME;
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        apply(ctx, deps.llm, config).await
    }
}

pub async fn apply(ctx: Context, llm: Service<Llm>, config: Cfg<Config>) -> Result<(), BoxError> {
    let initial = resolve_adapter_options(
        &plain_options(&config.get()),
        Some(&hoh_launch_environment::launch_environment_of(&ctx)),
    )
    .map_err(|error| Box::new(error) as BoxError)?;
    llm.register_configurable_providers(
        &ctx,
        vec![LlmConfigurableProvider {
            provider: PROVIDER.into(),
            display_name: "DeepSeek".into(),
            settings_ns: NAME.into(),
            settings_path: vec![],
            ..Default::default()
        }],
    )?;
    let loader: Arc<ListingLoader> = Arc::new(|url, credential, timeout_ms, signal| {
        Box::pin(async move {
            request_model_listing(&url, &credential, timeout_ms, Some(&signal)).await
        })
    });
    #[cfg(test)]
    let loader = ctx
        .tag::<ListingLoaderTag>()
        .map_or(loader, |tag| tag.0.clone());
    let (load_sender, mut load_receiver) = mpsc::unbounded_channel();
    let route = Arc::new(DeepSeekRoute {
        ctx: ctx.clone(),
        config: config.clone(),
        cache: Arc::new(Mutex::new(CacheState::default())),
        reported: Mutex::new(None),
        refresh_wakeup: Arc::new(Notify::new()),
        loader,
        load_sender,
    });
    let registration = llm.register_adapter(&ctx, vec![PROVIDER.into()], route.clone())?;
    ctx.spawn("llm-deepseek-api-key model listing", {
        let route = route.clone();
        move |cancel| async move {
            loop {
                match route.next_refresh_delay() {
                    Some(delay) => tokio::select! {
                        _ = cancel.cancelled() => { route.invalidate(); break },
                        job = load_receiver.recv() => match job {
                            Some(job) => route.run_load(job, AbortSignal::from_token(cancel.clone())).await,
                            None => break,
                        },
                        _ = route.refresh_wakeup.notified() => continue,
                        _ = tokio::time::sleep(delay) => route.background_refresh().await,
                    },
                    None => tokio::select! {
                        _ = cancel.cancelled() => { route.invalidate(); break },
                        job = load_receiver.recv() => match job {
                            Some(job) => route.run_load(job, AbortSignal::from_token(cancel.clone())).await,
                            None => break,
                        },
                        _ = route.refresh_wakeup.notified() => continue,
                    },
                }
            }
            Ok(())
        }
    })?;
    let following = Arc::new(Mutex::new(initial.listing.is_some()));
    let retry_policy = Arc::new(Mutex::new(initial.retry_policy));
    ctx.on::<VolatileUpdate>({
        let route = route.clone();
        let following = following.clone();
        let retry_policy = retry_policy.clone();
        move |_| {
            let options = match route.options() {
                Ok(options) => options,
                Err(error) => {
                    route.ctx.logger().warn(error);
                    return;
                }
            };
            let previous_policy = {
                let mut previous = lock(&retry_policy);
                if options.retry_policy != *previous {
                    Some(std::mem::replace(
                        &mut *previous,
                        options.retry_policy.clone(),
                    ))
                } else {
                    None
                }
            };
            if let Some(old_policy) = previous_policy
                && let Err(error) = registration.replace(vec![PROVIDER.into()])
            {
                let mut current = lock(&retry_policy);
                if *current == options.retry_policy {
                    *current = old_policy;
                }
                drop(current);
                route.ctx.logger().warn(error);
                return;
            }
            let next = options.listing.is_some();
            let mut previous = lock(&following);
            if next != *previous {
                route.invalidate();
            }
            *previous = next;
            route.refresh_wakeup.notify_one();
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listing::{ListedCapabilities, ListedFacts, ListingEntry, Requirement};
    use futures_util::StreamExt;
    use hoh_kernel::Kernel;
    use hoh_launch_environment::{
        LaunchEnvironment, LaunchEnvironmentLayerInput, LaunchEnvironmentSource,
        create_launch_environment_snapshot,
    };
    use hoh_llm::{
        GenerateOptions, LlmCallConfig, LlmModelBadge, LlmModelBadgeTone, LlmPlugin, StreamChunk,
    };
    use hoh_package_manifest::LocalizedText;
    use indexmap::IndexMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn listing(name: &str) -> ModelListing {
        ModelListing {
            entries: vec![ListingEntry {
                id: "deepseek-flash".into(),
                name: Some(name.into()),
                requirement: Requirement::None,
                capabilities: ListedCapabilities::Usable(ListedFacts {
                    input_modalities: vec!["text".into(), "image".into()],
                    context_window: Some(1_048_576),
                    max_output_tokens: None,
                    system_prompt_update: Some("in-history".into()),
                    tool_update: None,
                }),
                badge: None,
            }],
            skipped: 0,
        }
    }

    fn usable_entry(id: &str, name: &str) -> ListingEntry {
        ListingEntry {
            id: id.into(),
            name: Some(name.into()),
            requirement: Requirement::None,
            capabilities: ListedCapabilities::Usable(ListedFacts {
                input_modalities: vec!["text".into()],
                context_window: Some(1_048_576),
                max_output_tokens: None,
                system_prompt_update: None,
                tool_update: None,
            }),
            badge: None,
        }
    }

    fn badge(text: &str, tone: LlmModelBadgeTone) -> LlmModelBadge {
        LlmModelBadge {
            text: LocalizedText::Text(text.into()),
            tone,
        }
    }

    async fn host(
        loader: Arc<ListingLoader>,
        config: serde_json::Value,
    ) -> (Kernel, Context, hoh_kernel::Fiber) {
        let kernel = Kernel::start();
        let ctx = kernel.root().with_tag(ListingLoaderTag(loader));
        ctx.provide::<LaunchEnvironment>(Arc::new(create_launch_environment_snapshot(&[
            LaunchEnvironmentLayerInput {
                source: LaunchEnvironmentSource::Process,
                path: None,
                values: IndexMap::from_iter([
                    ("DEEPSEEK_API_KEY".into(), "fixture-api-key".into()),
                    ("ALT_KEY".into(), "alternate-api-key".into()),
                    ("MALFORMED_KEY".into(), "invalid\nheader".into()),
                ]),
            },
        ])))
        .await
        .unwrap();
        let llm = ctx.plugin::<LlmPlugin>(serde_json::json!({})).unwrap();
        llm.settled().await.unwrap();
        let provider = ctx.plugin::<DeepSeekApiKey>(config).unwrap();
        provider.settled().await.unwrap();
        (kernel, ctx, provider)
    }

    #[tokio::test]
    async fn caller_abort_returns_its_reason_while_single_flight_continues() {
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let loader: Arc<ListingLoader> = Arc::new({
            let calls = calls.clone();
            let started = started.clone();
            let release = release.clone();
            move |_, _, _, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                started.notify_one();
                let release = release.clone();
                Box::pin(async move {
                    release.notified().await;
                    Ok(listing("Loaded"))
                })
            }
        });
        let (kernel, ctx, _provider) = host(loader, serde_json::json!({})).await;
        let llm = ctx.get::<Llm>().unwrap();
        let caller = hoh_timeout::AbortController::new();
        let caller_signal = caller.signal();
        let pending = llm.resolve_model_info(PROVIDER, "deepseek-flash", Some(&caller_signal));
        tokio::pin!(pending);
        tokio::select! {
            _ = started.notified() => {}
            result = &mut pending => panic!("listing settled early: {result:?}"),
        }
        caller.abort_with(hoh_error::HarnessError::new("caller left", "TEST_ABORT"));
        assert_eq!(pending.await.unwrap_err().to_string(), "caller left");

        let second = llm.resolve_model_info(PROVIDER, "deepseek-flash", None);
        tokio::pin!(second);
        tokio::select! {
            _ = tokio::task::yield_now() => {}
            result = &mut second => panic!("shared listing settled early: {result:?}"),
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        release.notify_waiters();
        assert_eq!(second.await.unwrap().model.name, "Loaded");
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn official_listing_preserves_order_credential_facts_and_info_diagnostic() {
        let credentials = Arc::new(Mutex::new(Vec::new()));
        let loader: Arc<ListingLoader> = Arc::new({
            let credentials = credentials.clone();
            move |url, credential, _, _| {
                assert_eq!(url, "https://api.deepseek.com/models");
                lock(&credentials).push(credential);
                Box::pin(async move {
                    let mut flash = usable_entry("deepseek-flash", "Flash");
                    let ListedCapabilities::Usable(facts) = &mut flash.capabilities else {
                        unreachable!()
                    };
                    facts.input_modalities = vec!["text".into(), "image".into()];
                    facts.system_prompt_update = Some("in-history".into());
                    let mut v5 = usable_entry("deepseek-v5", "V5");
                    v5.requirement = Requirement::MinVersion("0.0.1".into());
                    Ok(ModelListing {
                        entries: vec![flash, v5, usable_entry("deepseek-v4-pro", "Pro")],
                        skipped: 0,
                    })
                })
            }
        });
        let (kernel, ctx, _provider) = host(loader, serde_json::json!({})).await;
        let llm = ctx.get::<Llm>().unwrap();
        assert_eq!(
            llm.list_models(PROVIDER)
                .await
                .unwrap()
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["deepseek-flash", "deepseek-v5", "deepseek-v4-pro"]
        );
        let resolved = llm
            .resolve_model_info(PROVIDER, "deepseek-flash", None)
            .await
            .unwrap();
        assert_eq!(resolved.context.unwrap().context_window, 1_048_576);
        assert_eq!(
            resolved.model.input_modalities,
            Some(vec!["text".into(), "image".into()])
        );
        assert_eq!(resolved.system_prompt_update.as_deref(), Some("in-history"));
        assert_eq!(resolved.tool_update.as_deref(), Some("addition-only"));
        assert_eq!(*lock(&credentials), ["fixture-api-key"]);
        let infos: Vec<_> = ctx
            .logger()
            .buffer()
            .into_iter()
            .filter(|record| {
                record
                    .message
                    .contains("loaded 3 model(s), skipped 0 row(s)")
            })
            .collect();
        assert_eq!(infos.len(), 1);
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn advertised_badges_exist_only_while_following_the_listing() {
        let calls = Arc::new(AtomicUsize::new(0));
        let loader: Arc<ListingLoader> = Arc::new({
            let calls = calls.clone();
            move |_, _, _, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    let mut flash = usable_entry("deepseek-flash", "Flash");
                    flash.badge = Some(Ok(badge("1017", LlmModelBadgeTone::Info)));
                    Ok(ModelListing {
                        entries: vec![flash, usable_entry("deepseek-v4-pro", "Pro")],
                        skipped: 0,
                    })
                })
            }
        });
        let (kernel, ctx, _provider) = host(loader.clone(), serde_json::json!({})).await;
        let llm = ctx.get::<Llm>().unwrap();
        let models = llm.list_models(PROVIDER).await.unwrap();
        assert_eq!(
            models[0].badge,
            Some(badge("1017", LlmModelBadgeTone::Info))
        );
        assert!(models[1].badge.is_none());
        assert!(
            llm.resolve_model_info(PROVIDER, "deepseek-flash", None)
                .await
                .unwrap()
                .model
                .badge
                .is_none()
        );
        kernel.shutdown().await;

        let (kernel, ctx, _provider) = host(
            loader,
            serde_json::json!({"models":[{"id":"custom","name":"Custom"}]}),
        )
        .await;
        let models = ctx
            .get::<Llm>()
            .unwrap()
            .list_models(PROVIDER)
            .await
            .unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "custom");
        assert!(models[0].badge.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn custom_catalog_and_other_endpoint_never_load_the_official_listing() {
        let calls = Arc::new(AtomicUsize::new(0));
        let loader: Arc<ListingLoader> = Arc::new({
            let calls = calls.clone();
            move |_, _, _, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move { Ok(listing("unexpected")) })
            }
        });
        let (kernel, ctx, _provider) = host(
            loader.clone(),
            serde_json::json!({"models":[{"id":"custom"}]}),
        )
        .await;
        let custom = ctx
            .get::<Llm>()
            .unwrap()
            .list_models(PROVIDER)
            .await
            .unwrap();
        assert_eq!(
            custom
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["custom"]
        );
        assert!(custom.iter().all(|model| model.badge.is_none()));
        kernel.shutdown().await;

        let (kernel, ctx, _provider) = host(
            loader,
            serde_json::json!({"baseURL":"http://127.0.0.1:9/anthropic"}),
        )
        .await;
        let builtin = ctx
            .get::<Llm>()
            .unwrap()
            .list_models(PROVIDER)
            .await
            .unwrap();
        assert_eq!(
            builtin
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["deepseek-flash", "deepseek-v4-pro"]
        );
        assert!(builtin.iter().all(|model| model.badge.is_none()));
        kernel.shutdown().await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn missing_credential_never_starts_a_listing_load_for_default_or_custom_catalogs() {
        let calls = Arc::new(AtomicUsize::new(0));
        let loader: Arc<ListingLoader> = Arc::new({
            let calls = calls.clone();
            move |_, _, _, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move { Ok(listing("unexpected")) })
            }
        });
        for config in [
            serde_json::json!({"apiKeyEnv":"MISSING_DEEPSEEK_API_KEY"}),
            serde_json::json!({
                "apiKeyEnv":"MISSING_DEEPSEEK_API_KEY",
                "models":[{"id":"custom"}]
            }),
        ] {
            let (kernel, ctx, _provider) = host(loader.clone(), config).await;
            assert_eq!(
                ctx.get::<Llm>()
                    .unwrap()
                    .list_models(PROVIDER)
                    .await
                    .unwrap(),
                []
            );
            kernel.shutdown().await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn listing_failure_uses_bundled_catalog_and_warns_once() {
        let loader: Arc<ListingLoader> = Arc::new(|_, _, _, _| {
            Box::pin(async move {
                Err(Box::new(hoh_error::HarnessError::new("down", "TRANSPORT")) as BoxError)
            })
        });
        let (kernel, ctx, _provider) = host(loader, serde_json::json!({})).await;
        let llm = ctx.get::<Llm>().unwrap();
        assert_eq!(
            llm.list_models(PROVIDER)
                .await
                .unwrap()
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["deepseek-flash", "deepseek-v4-pro"]
        );
        let warnings: Vec<_> = ctx
            .logger()
            .buffer()
            .into_iter()
            .filter(|record| record.message.contains("using the bundled catalog"))
            .collect();
        assert_eq!(warnings.len(), 1);
        kernel.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn stale_view_returns_immediately_while_refresh_publishes_change() {
        let calls = Arc::new(AtomicUsize::new(0));
        let refresh_started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let loader: Arc<ListingLoader> = Arc::new({
            let calls = calls.clone();
            let refresh_started = refresh_started.clone();
            let release = release.clone();
            move |_, _, _, _| {
                let call = calls.fetch_add(1, Ordering::SeqCst);
                let refresh_started = refresh_started.clone();
                let release = release.clone();
                Box::pin(async move {
                    if call == 0 {
                        Ok(listing("First"))
                    } else {
                        refresh_started.notify_one();
                        release.notified().await;
                        Ok(listing("Updated"))
                    }
                })
            }
        });
        let (kernel, ctx, _provider) = host(loader, serde_json::json!({"modelsMaxAgeMs":10})).await;
        let llm = ctx.get::<Llm>().unwrap();
        assert_eq!(llm.list_models(PROVIDER).await.unwrap()[0].name, "First");
        tokio::time::advance(Duration::from_millis(10)).await;
        refresh_started.notified().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let stale = llm.list_models(PROVIDER);
        tokio::pin!(stale);
        tokio::select! {
            result = &mut stale => assert_eq!(result.unwrap()[0].name, "First"),
            _ = tokio::time::sleep(Duration::from_millis(1)) => panic!("stale read blocked on refresh"),
        }
        release.notify_waiters();
        for _ in 0..10 {
            if llm.list_models(PROVIDER).await.unwrap()[0].name == "Updated" {
                kernel.shutdown().await;
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("refreshed listing was not published")
    }

    #[tokio::test(start_paused = true)]
    async fn background_refresh_with_a_new_credential_does_not_deadlock_the_driver() {
        let calls = Arc::new(AtomicUsize::new(0));
        let credentials = Arc::new(Mutex::new(Vec::new()));
        let loader: Arc<ListingLoader> = Arc::new({
            let calls = calls.clone();
            let credentials = credentials.clone();
            move |_, credential, _, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                lock(&credentials).push(credential);
                Box::pin(async move { Ok(listing("Loaded")) })
            }
        });
        let (kernel, ctx, provider) = host(loader, serde_json::json!({"modelsMaxAgeMs":10})).await;
        let llm = ctx.get::<Llm>().unwrap();
        assert_eq!(llm.list_models(PROVIDER).await.unwrap()[0].name, "Loaded");
        assert!(
            provider
                .commit_volatile(serde_json::json!({
                    "modelsMaxAgeMs":10,
                    "apiKeyEnv":"ALT_KEY"
                }))
                .await
                .unwrap()
        );
        tokio::time::advance(Duration::from_millis(10)).await;
        let models = tokio::time::timeout(Duration::from_secs(1), llm.list_models(PROVIDER))
            .await
            .expect("refresh driver must service its queued first load")
            .unwrap();
        assert_eq!(models[0].name, "Loaded");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            *lock(&credentials),
            ["fixture-api-key", "alternate-api-key"]
        );
        tokio::time::timeout(Duration::from_secs(1), kernel.shutdown())
            .await
            .expect("shutdown must not wait on a self-deadlocked refresh");
    }

    #[tokio::test]
    async fn resolution_describes_refused_models_but_prepare_call_admits_them() {
        let calls = Arc::new(AtomicUsize::new(0));
        let loader: Arc<ListingLoader> = Arc::new({
            let calls = calls.clone();
            move |_, _, _, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    Ok(ModelListing {
                        entries: vec![
                            usable_entry("deepseek-flash", "Flash"),
                            ListingEntry {
                                id: "deepseek-v4-pro".into(),
                                name: Some("Pro".into()),
                                requirement: Requirement::None,
                                capabilities: ListedCapabilities::Unusable(
                                    "declares no api_capabilities.anthropic_messages".into(),
                                ),
                                badge: None,
                            },
                            ListingEntry {
                                id: "deepseek-v5".into(),
                                name: Some("V5".into()),
                                requirement: Requirement::None,
                                capabilities: ListedCapabilities::Usable(ListedFacts {
                                    input_modalities: vec!["text".into()],
                                    context_window: None,
                                    max_output_tokens: None,
                                    system_prompt_update: None,
                                    tool_update: None,
                                }),
                                badge: None,
                            },
                            ListingEntry {
                                id: "deepseek-v5-next".into(),
                                name: Some("V5 Next".into()),
                                requirement: Requirement::MinVersion("99.0.0-alpha.1".into()),
                                capabilities: ListedCapabilities::Usable(ListedFacts {
                                    input_modalities: vec!["text".into()],
                                    context_window: None,
                                    max_output_tokens: None,
                                    system_prompt_update: None,
                                    tool_update: None,
                                }),
                                badge: None,
                            },
                        ],
                        skipped: 0,
                    })
                })
            }
        });
        let (kernel, ctx, _provider) = host(loader, serde_json::json!({})).await;
        let llm = ctx.get::<Llm>().unwrap();
        assert_eq!(
            llm.list_models(PROVIDER)
                .await
                .unwrap()
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["deepseek-flash"]
        );
        for model in ["deepseek-v4-pro", "deepseek-v5", "deepseek-v5-next"] {
            assert_eq!(
                llm.resolve_model_info(PROVIDER, model, None)
                    .await
                    .unwrap()
                    .model
                    .id,
                model
            );
        }
        for (model, code) in [
            ("deepseek-v4-pro", "MODEL_UNSUPPORTED"),
            ("deepseek-v5", "MODEL_UNSUPPORTED"),
            ("deepseek-v5-next", "CLIENT_UPGRADE_REQUIRED"),
        ] {
            let result = llm
                .prepare_call(
                    &ctx,
                    LlmCallConfig {
                        provider: PROVIDER.into(),
                        model: model.into(),
                        ..Default::default()
                    },
                    None,
                    None,
                )
                .await;
            let error = match result {
                Ok(_) => panic!("{model} should be refused during admission"),
                Err(error) => error,
            };
            assert_eq!(
                hoh_error::HarnessError::of(error.as_ref()).unwrap().code(),
                code
            );
        }
        let mut direct = llm.stream(
            &ctx,
            GenerateOptions {
                provider: PROVIDER.into(),
                model: "deepseek-v4-pro".into(),
                ..Default::default()
            },
        );
        let direct_chunk = direct.next().await.unwrap().unwrap();
        let StreamChunk::Finish { reason, .. } = direct_chunk else {
            panic!("direct stream admission failure should finish the stream")
        };
        assert_eq!(reason.failure().unwrap().code, "MODEL_UNSUPPORTED");
        llm.prepare_call(
            &ctx,
            LlmCallConfig {
                provider: PROVIDER.into(),
                model: "unlisted-pass-through".into(),
                ..Default::default()
            },
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let problems: Vec<_> = ctx
            .logger()
            .buffer()
            .into_iter()
            .filter(|record| {
                record.message
                    == "[llm-deepseek-api-key] model \"deepseek-v4-pro\" declares no api_capabilities.anthropic_messages"
            })
            .collect();
        assert_eq!(problems.len(), 1);
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn malformed_replacement_key_does_not_reuse_the_previous_listing_refusal() {
        let calls = Arc::new(AtomicUsize::new(0));
        let loader: Arc<ListingLoader> = Arc::new({
            let calls = calls.clone();
            move |_, _, _, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    let mut next = usable_entry("deepseek-v5-next", "V5 Next");
                    next.requirement = Requirement::MinVersion("99.0.0-alpha.1".into());
                    Ok(ModelListing {
                        entries: vec![usable_entry("deepseek-flash", "Flash"), next],
                        skipped: 0,
                    })
                })
            }
        });
        let (kernel, ctx, provider) = host(loader, serde_json::json!({})).await;
        let llm = ctx.get::<Llm>().unwrap();
        let refused = llm
            .prepare_call(
                &ctx,
                LlmCallConfig {
                    provider: PROVIDER.into(),
                    model: "deepseek-v5-next".into(),
                    ..Default::default()
                },
                None,
                None,
            )
            .await;
        let Err(refused) = refused else {
            panic!("the original listing should refuse the future model")
        };
        assert_eq!(
            hoh_error::HarnessError::of(refused.as_ref())
                .unwrap()
                .code(),
            "CLIENT_UPGRADE_REQUIRED"
        );
        assert!(
            provider
                .commit_volatile(serde_json::json!({"apiKeyEnv":"MALFORMED_KEY"}))
                .await
                .unwrap()
        );
        llm.prepare_call(
            &ctx,
            LlmCallConfig {
                provider: PROVIDER.into(),
                model: "deepseek-v5-next".into(),
                ..Default::default()
            },
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn prepared_dispatch_keeps_its_admitted_connection_generation() {
        let calls = Arc::new(AtomicUsize::new(0));
        let loader: Arc<ListingLoader> = Arc::new({
            let calls = calls.clone();
            move |_, _, _, _| {
                let call = calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    let flash = if call == 0 {
                        usable_entry("deepseek-flash", "Flash")
                    } else {
                        ListingEntry {
                            id: "deepseek-flash".into(),
                            name: Some("Flash".into()),
                            requirement: Requirement::None,
                            capabilities: ListedCapabilities::Unusable(
                                "declares no api_capabilities.anthropic_messages".into(),
                            ),
                            badge: None,
                        }
                    };
                    Ok(ModelListing {
                        entries: vec![flash],
                        skipped: 0,
                    })
                })
            }
        });
        let (kernel, ctx, provider) = host(loader, serde_json::json!({})).await;
        let llm = ctx.get::<Llm>().unwrap();
        let prepared = llm
            .prepare_call(
                &ctx,
                LlmCallConfig {
                    provider: PROVIDER.into(),
                    model: "deepseek-flash".into(),
                    ..Default::default()
                },
                None,
                None,
            )
            .await
            .unwrap();
        assert!(
            provider
                .commit_volatile(serde_json::json!({"apiKeyEnv":"ALT_KEY"}))
                .await
                .unwrap()
        );
        assert_eq!(llm.list_models(PROVIDER).await.unwrap(), []);
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        let request = GenerateOptions::default().with_call_config(&prepared.config);
        let mut stream = prepared.stream(request).unwrap();
        let chunk = stream.next().await.unwrap().unwrap();
        let StreamChunk::Finish { reason, .. } = chunk else {
            panic!("transport failure should finish the prepared stream")
        };
        assert_eq!(reason.failure().unwrap().code, "TRANSPORT");
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn returning_to_the_builtin_catalog_reloads_the_listing_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let loader: Arc<ListingLoader> = Arc::new({
            let calls = calls.clone();
            move |_, _, _, _| {
                let call = calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    let mut entries = vec![
                        usable_entry("deepseek-flash", "Flash"),
                        usable_entry("deepseek-v4-pro", "Pro"),
                    ];
                    if call > 0 {
                        let mut v5 = usable_entry("deepseek-v5", "V5");
                        v5.requirement = Requirement::MinVersion("0.0.1".into());
                        entries.push(v5);
                    }
                    Ok(ModelListing {
                        entries,
                        skipped: 0,
                    })
                })
            }
        });
        let (kernel, ctx, provider) = host(loader, serde_json::json!({})).await;
        let llm = ctx.get::<Llm>().unwrap();
        assert_eq!(llm.list_models(PROVIDER).await.unwrap().len(), 2);
        assert!(
            provider
                .commit_volatile(serde_json::json!({"models":[{"id":"custom"}]}))
                .await
                .unwrap()
        );
        assert_eq!(llm.list_models(PROVIDER).await.unwrap()[0].id, "custom");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(
            provider
                .commit_volatile(serde_json::json!({}))
                .await
                .unwrap()
        );
        assert_eq!(
            llm.list_models(PROVIDER)
                .await
                .unwrap()
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["deepseek-flash", "deepseek-v4-pro", "deepseek-v5"]
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn idle_refresh_replaces_then_clears_a_badge_and_notifies_each_change() {
        let calls = Arc::new(AtomicUsize::new(0));
        let loader: Arc<ListingLoader> = Arc::new({
            let calls = calls.clone();
            move |_, _, _, _| {
                let call = calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    let mut flash = usable_entry("deepseek-flash", "Flash");
                    flash.badge = match call {
                        0 => Some(Ok(badge("New", LlmModelBadgeTone::Info))),
                        1 => Some(Ok(badge("Updated", LlmModelBadgeTone::Success))),
                        _ => None,
                    };
                    Ok(ModelListing {
                        entries: vec![flash],
                        skipped: 0,
                    })
                })
            }
        });
        let (kernel, ctx, _provider) = host(loader, serde_json::json!({"modelsMaxAgeMs":50})).await;
        let changes = Arc::new(AtomicUsize::new(0));
        ctx.on::<ModelsUpdated>({
            let changes = changes.clone();
            move |event| {
                assert_eq!(event.provider, PROVIDER);
                changes.fetch_add(1, Ordering::SeqCst);
            }
        })
        .unwrap();
        let llm = ctx.get::<Llm>().unwrap();
        assert_eq!(
            llm.list_models(PROVIDER).await.unwrap()[0].badge,
            Some(badge("New", LlmModelBadgeTone::Info))
        );
        assert_eq!(changes.load(Ordering::SeqCst), 1);

        for (expected_change, expected_badge) in [
            (2, Some(badge("Updated", LlmModelBadgeTone::Success))),
            (3, None),
        ] {
            tokio::time::timeout(Duration::from_secs(1), async {
                while changes.load(Ordering::SeqCst) < expected_change {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .expect("idle refresh should publish the catalog change");
            assert_eq!(changes.load(Ordering::SeqCst), expected_change);
            assert_eq!(
                llm.list_models(PROVIDER).await.unwrap()[0].badge,
                expected_badge
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        kernel.shutdown().await;
    }

    #[tokio::test]
    async fn rejected_volatile_configuration_warns_once_and_keeps_following_listing() {
        let calls = Arc::new(AtomicUsize::new(0));
        let loader: Arc<ListingLoader> = Arc::new({
            let calls = calls.clone();
            move |_, _, _, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move { Ok(listing("Listed")) })
            }
        });
        let (kernel, ctx, provider) = host(loader, serde_json::json!({})).await;
        let llm = ctx.get::<Llm>().unwrap();
        assert_eq!(llm.list_models(PROVIDER).await.unwrap()[0].name, "Listed");
        assert!(
            provider
                .commit_volatile(serde_json::json!({
                    "thinking":"disabled",
                    "reasoningEffort":"high"
                }))
                .await
                .unwrap()
        );
        let warnings: Vec<_> = ctx
            .logger()
            .buffer()
            .into_iter()
            .filter(|record| {
                record
                    .message
                    .contains("only reasoningEffort \"off\" can be configured")
            })
            .collect();
        assert_eq!(warnings.len(), 1);
        assert!(
            llm.list_models(PROVIDER)
                .await
                .unwrap_err()
                .to_string()
                .contains("only reasoningEffort \"off\" can be configured")
        );
        assert!(
            provider
                .commit_volatile(serde_json::json!({}))
                .await
                .unwrap()
        );
        assert_eq!(llm.list_models(PROVIDER).await.unwrap()[0].name, "Listed");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        kernel.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn background_credential_loss_publishes_models_updated() {
        let loader: Arc<ListingLoader> =
            Arc::new(|_, _, _, _| Box::pin(async move { Ok(listing("Authenticated")) }));
        let (kernel, ctx, provider) = host(loader, serde_json::json!({"modelsMaxAgeMs":10})).await;
        let changes = Arc::new(AtomicUsize::new(0));
        let observed = changes.clone();
        ctx.on::<ModelsUpdated>(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        let llm = ctx.get::<Llm>().unwrap();
        assert_eq!(llm.list_models(PROVIDER).await.unwrap().len(), 1);
        assert_eq!(changes.load(Ordering::SeqCst), 1);
        assert!(
            provider
                .commit_volatile(serde_json::json!({
                    "modelsMaxAgeMs":10,
                    "apiKeyEnv":"MISSING_DEEPSEEK_API_KEY"
                }))
                .await
                .unwrap()
        );
        tokio::time::advance(Duration::from_millis(10)).await;
        for _ in 0..10 {
            if changes.load(Ordering::SeqCst) == 2 {
                assert!(llm.list_models(PROVIDER).await.unwrap().is_empty());
                kernel.shutdown().await;
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("credential loss did not publish a model update")
    }
}
