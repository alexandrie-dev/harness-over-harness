use crate::{
    adapter_failure::normalize_llm_failure, call_config::*, content::*, error::*, retry_policy::*,
    types::*,
};
use futures_util::{Stream, future::BoxFuture, stream::BoxStream};
use hoh_attachment::{Attachments, FileAttachmentRef, ImageAttachmentRef, ImageRequestTarget};
use hoh_error::BoxError;
use hoh_kernel::{Contained, Context, Disposer, Emit, ListenerFailure, Undo, Waterfall};
use hoh_timeout::AbortSignal;
use indexmap::{IndexMap, IndexSet};
use std::{
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard},
    task::{Context as TaskContext, Poll},
};

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
pub type LlmFuture<'a, T> = BoxFuture<'a, Result<T, BoxError>>;
pub type LlmDispatch = Arc<dyn Fn(GenerateOptions) -> Result<LlmStream, BoxError> + Send + Sync>;
type Cleanup = Box<dyn FnOnce() -> LlmFuture<'static, ()> + Send>;

/// Fallible stream with explicit asynchronous iterator-return semantics.
/// Drop releases the stream immediately; call close().await to observe asynchronous cleanup errors.
pub struct LlmStream {
    state: StreamState,
    cleanup: Option<Cleanup>,
}
enum StreamState {
    Direct(BoxStream<'static, Result<StreamChunk, BoxError>>),
    Deferred {
        future: Option<LlmFuture<'static, LlmStream>>,
        active: Option<Box<LlmStream>>,
        boundary: bool,
        signal: Option<AbortSignal>,
        done: bool,
    },
}
impl LlmStream {
    pub fn new(stream: impl Stream<Item = Result<StreamChunk, BoxError>> + Send + 'static) -> Self {
        Self {
            state: StreamState::Direct(Box::pin(stream)),
            cleanup: None,
        }
    }
    pub fn chunks(chunks: impl IntoIterator<Item = StreamChunk>) -> Self {
        let chunks: Vec<_> = chunks.into_iter().map(Ok).collect();
        Self::new(futures_util::stream::iter(chunks))
    }
    pub fn with_cleanup(
        mut self,
        cleanup: impl FnOnce() -> LlmFuture<'static, ()> + Send + 'static,
    ) -> Self {
        self.cleanup = Some(Box::new(cleanup));
        self
    }
    fn deferred(
        future: LlmFuture<'static, LlmStream>,
        boundary: bool,
        signal: Option<AbortSignal>,
    ) -> Self {
        Self {
            state: StreamState::Deferred {
                future: Some(future),
                active: None,
                boundary,
                signal,
                done: false,
            },
            cleanup: None,
        }
    }
    pub fn close(self) -> LlmFuture<'static, ()> {
        Box::pin(async move {
            let mut current = self;
            let mut cleanups = vec![];
            loop {
                if let Some(cleanup) = current.cleanup.take() {
                    cleanups.push(cleanup);
                }
                match current.state {
                    StreamState::Deferred {
                        active: Some(active),
                        ..
                    } => current = *active,
                    _ => break,
                }
            }
            for cleanup in cleanups.into_iter().rev() {
                cleanup().await?;
            }
            Ok(())
        })
    }
}
impl Stream for LlmStream {
    type Item = Result<StreamChunk, BoxError>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match &mut this.state {
            StreamState::Direct(stream) => stream.as_mut().poll_next(cx),
            StreamState::Deferred {
                future,
                active,
                boundary,
                signal,
                done,
            } => {
                if *done {
                    return Poll::Ready(None);
                }
                if let Some(future_inner) = future {
                    match future_inner.as_mut().poll(cx) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(Ok(stream)) => {
                            *active = Some(Box::new(stream));
                            *future = None;
                        }
                        Poll::Ready(Err(error)) => {
                            *done = true;
                            *future = None;
                            return Poll::Ready(Some(if *boundary {
                                Ok(adapter_failure_chunk(error.as_ref(), signal.as_ref()))
                            } else {
                                Err(error)
                            }));
                        }
                    }
                }
                let Some(stream) = active else {
                    *done = true;
                    return Poll::Ready(None);
                };
                match Pin::new(stream.as_mut()).poll_next(cx) {
                    Poll::Ready(Some(Err(error))) if *boundary => {
                        // An adapter iteration error is already completed: no iterator.return lookup.
                        *done = true;
                        *active = None;
                        Poll::Ready(Some(Ok(adapter_failure_chunk(
                            error.as_ref(),
                            signal.as_ref(),
                        ))))
                    }
                    Poll::Ready(None) => {
                        *done = true;
                        *active = None;
                        Poll::Ready(None)
                    }
                    other => other,
                }
            }
        }
    }
}
pub struct PreparedAdapterCall {
    pub model: LlmResolvedModelInfo,
    pub dispatch: LlmDispatch,
}
/// Public provider boundary; a replay adapter implements only stream().
pub trait LlmAdapter: Send + Sync + 'static {
    fn provider_info(&self, provider: &str) -> Result<LlmProviderInfo, BoxError> {
        Ok(LlmProviderInfo {
            id: provider.into(),
            name: provider.into(),
        })
    }
    fn provider_retry_policy(
        &self,
        _provider: &str,
    ) -> Result<Option<ResolvedRetryPolicy>, BoxError> {
        Ok(None)
    }
    fn image_request_pricing(
        &self,
        _provider: &str,
        _model: &str,
    ) -> Option<Arc<dyn LlmImageRequestPricing>> {
        None
    }
    fn image_request_target(
        &self,
        _provider: &str,
        _model: &str,
        image: &ImageAttachmentRef,
    ) -> ImageRequestTarget {
        ImageRequestTarget {
            width: image.width,
            height: image.height,
            max_bytes: image.bytes,
        }
    }
    fn list_models<'a>(&'a self, _provider: &'a str) -> LlmFuture<'a, Vec<LlmModelInfo>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn resolve_model<'a>(
        &'a self,
        provider: &'a str,
        model: &'a str,
        _signal: Option<&'a AbortSignal>,
    ) -> LlmFuture<'a, LlmResolvedModelInfo> {
        Box::pin(async move {
            Ok(LlmResolvedModelInfo {
                model: LlmModelInfo {
                    provider: provider.into(),
                    id: model.into(),
                    name: model.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
        })
    }
    fn prepare_call(
        self: Arc<Self>,
        provider: String,
        model: String,
        signal: Option<AbortSignal>,
    ) -> LlmFuture<'static, PreparedAdapterCall> {
        Box::pin(async move {
            let model = self
                .resolve_model(&provider, &model, signal.as_ref())
                .await?;
            Ok(PreparedAdapterCall {
                model,
                dispatch: Arc::new(move |options| self.stream(options)),
            })
        })
    }
    fn stream(&self, options: GenerateOptions) -> Result<LlmStream, BoxError>;
}
pub struct AdaptersUpdated;
hoh_kernel::event!(AdaptersUpdated = "llm/adapters-updated", Contained);
pub struct ModelsUpdated {
    pub provider: String,
}
hoh_kernel::event!(ModelsUpdated = "llm/models-updated", Emit);
/// The bound runtime replaces JavaScript's waterfall `this` argument.
pub struct LlmStreamEvent {
    pub runtime: LlmRuntime,
    options: Mutex<GenerateOptions>,
}
impl LlmStreamEvent {
    pub fn options(&self) -> GenerateOptions {
        let options = lock(&self.options);
        let mut copy = options.clone();
        copy.agent_loop_request = options.agent_loop_request;
        copy
    }
    pub fn update_options(
        &self,
        update: impl FnOnce(&mut GenerateOptions),
    ) -> Result<(), ValueError> {
        let mut options = lock(&self.options);
        if options.agent_loop_request {
            return Err(ValueError::Message(
                "Cannot assign to read only property of a loop-built LLM request".into(),
            ));
        }
        update(&mut options);
        Ok(())
    }
}
hoh_kernel::event!(LlmStreamEvent = "llm/stream", Waterfall<LlmStream>);
/// Structural fs seam until its owning crate exists. Resolve from the caller's current context.
#[derive(Clone)]
pub struct HostPathMapping(pub Arc<HostPathMapper>);
pub type HostPathMapper = dyn Fn(&str) -> Option<String> + Send + Sync;
pub type ModelDiscovery = Arc<
    dyn Fn(
            LlmModelDiscoveryRequest,
            Option<AbortSignal>,
        ) -> LlmFuture<'static, Vec<LlmDiscoveredModel>>
        + Send
        + Sync,
>;
#[derive(Clone)]
struct AdapterRegistration {
    adapter: Arc<dyn LlmAdapter>,
    provider: LlmProviderInfo,
    retry_policy: ResolvedRetryPolicy,
}
#[derive(Default)]
struct Registry {
    adapters: IndexMap<String, AdapterRegistration>,
    directory: IndexMap<String, LlmConfigurableProvider>,
    discoveries: IndexMap<String, ModelDiscovery>,
}
#[derive(Clone, Default)]
pub struct LlmRuntime {
    registry: Arc<Mutex<Registry>>,
}
impl LlmRuntime {
    pub fn new() -> Self {
        Self::default()
    }
    fn emit_adapters_updated(ctx: &Context) {
        let logger = ctx.logger();
        ctx.dispatch(AdaptersUpdated).invoke(move |failure| {
            let error = match failure {
                ListenerFailure::Threw(error) | ListenerFailure::Rejected(error) => error,
            };
            logger.warn("llm: an llm/adapters-updated listener failed");
            logger.warn(error);
        });
    }
    pub fn register_adapter(
        &self,
        ctx: &Context,
        providers: Vec<String>,
        adapter: Arc<dyn LlmAdapter>,
    ) -> Result<AdapterRegistrationHandle, BoxError> {
        ctx.ensure_active()?;
        if providers.is_empty() {
            return Err(crate::error::llm_error(
                "an adapter must register at least one provider",
                BuiltinLlmFailureCode::InvalidAdapter,
            )
            .into());
        }
        let held = Arc::new(Mutex::new(HeldRoutes::default()));
        let runtime = self.clone();
        let held_undo = held.clone();
        let notify = ctx.clone();
        let disposer = ctx.try_effect::<BoxError>("llm.registerAdapter()", |undos| {
            self.replace_routes(ctx, &held, &adapter, providers)?;
            undos.push(Undo::sync(move || {
                {
                    let mut held = lock(&held_undo);
                    held.released = true;
                    let mut state = lock(&runtime.registry);
                    for provider in &held.routes {
                        state.adapters.shift_remove(provider);
                    }
                    held.routes.clear();
                }
                Self::emit_adapters_updated(&notify);
            }))?;
            Ok(())
        })?;
        Ok(AdapterRegistrationHandle {
            runtime: self.clone(),
            ctx: ctx.clone(),
            adapter,
            held,
            disposer: Mutex::new(Some(disposer)),
        })
    }
    fn replace_routes(
        &self,
        ctx: &Context,
        held: &Mutex<HeldRoutes>,
        adapter: &Arc<dyn LlmAdapter>,
        providers: Vec<String>,
    ) -> Result<(), BoxError> {
        if lock(held).released {
            return Err(crate::error::llm_error(
                "a disposed adapter registration cannot replace its routes",
                BuiltinLlmFailureCode::RegistrationDisposed,
            )
            .into());
        }
        let mut prepared = Vec::new();
        let mut unique = IndexSet::new();
        for provider in &providers {
            if provider.is_empty() {
                return Err(crate::error::llm_error(
                    "adapter provider names must be non-empty",
                    BuiltinLlmFailureCode::InvalidAdapter,
                )
                .into());
            }
            let owned = lock(held).routes.contains(provider);
            let registered_elsewhere =
                lock(&self.registry).adapters.contains_key(provider) && !owned;
            if !unique.insert(provider.clone()) || registered_elsewhere {
                return Err(crate::error::llm_error(
                    format!("an adapter for provider \"{provider}\" is already registered"),
                    BuiltinLlmFailureCode::DuplicateAdapter,
                )
                .into());
            }
            let info = adapter.provider_info(provider)?;
            if info.id != *provider || info.name.is_empty() {
                return Err(crate::error::llm_error(format!("adapter metadata for provider \"{provider}\" must preserve its id and have a non-empty name"),BuiltinLlmFailureCode::InvalidAdapter).into());
            }
            let policy = adapter
                .provider_retry_policy(provider)?
                .unwrap_or(resolve_retry_policy(
                    None,
                    &format!("llm: provider \"{provider}\" retryPolicy"),
                )?);
            prepared.push(AdapterRegistration {
                adapter: adapter.clone(),
                provider: info,
                retry_policy: policy,
            });
        }
        {
            let mut held = lock(held);
            if held.released {
                return Err(crate::error::llm_error(
                    "a disposed adapter registration cannot replace its routes",
                    BuiltinLlmFailureCode::RegistrationDisposed,
                )
                .into());
            }
            let mut state = lock(&self.registry);
            for p in &providers {
                if state.adapters.contains_key(p) && !held.routes.contains(p) {
                    return Err(crate::error::llm_error(
                        format!("an adapter for provider \"{p}\" is already registered"),
                        BuiltinLlmFailureCode::DuplicateAdapter,
                    )
                    .into());
                }
            }
            for p in &held.routes {
                state.adapters.shift_remove(p);
            }
            held.routes.clear();
            for registration in prepared {
                held.routes.insert(registration.provider.id.clone());
                state
                    .adapters
                    .insert(registration.provider.id.clone(), registration);
            }
        }
        Self::emit_adapters_updated(ctx);
        Ok(())
    }
    /// Remote: llm.listProviders
    pub fn list_providers(&self) -> Vec<LlmProviderInfo> {
        lock(&self.registry)
            .adapters
            .values()
            .map(|r| r.provider.clone())
            .collect()
    }
    pub fn register_configurable_providers(
        &self,
        ctx: &Context,
        entries: Vec<LlmConfigurableProvider>,
    ) -> Result<DirectoryRegistrationHandle, BoxError> {
        ctx.ensure_active()?;
        if entries.is_empty() {
            return Err(crate::error::llm_error(
                "a configurable-provider registration must declare at least one provider",
                BuiltinLlmFailureCode::InvalidDirectory,
            )
            .into());
        }
        let held = Arc::new(Mutex::new(HeldDirectory::default()));
        let runtime = self.clone();
        let held_undo = held.clone();
        let notify = ctx.clone();
        let disposer =
            ctx.try_effect::<BoxError>("llm.registerConfigurableProviders()", |undos| {
                self.replace_directory(ctx, &held, entries)?;
                undos.push(Undo::sync(move || {
                    {
                        let mut held = lock(&held_undo);
                        held.released = true;
                        let mut state = lock(&runtime.registry);
                        for entry in &held.entries {
                            state.directory.shift_remove(&entry.provider);
                        }
                        held.entries.clear();
                    }
                    Self::emit_adapters_updated(&notify);
                }))?;
                Ok(())
            })?;
        Ok(DirectoryRegistrationHandle {
            runtime: self.clone(),
            ctx: ctx.clone(),
            held,
            disposer: Mutex::new(Some(disposer)),
        })
    }
    fn replace_directory(
        &self,
        ctx: &Context,
        held: &Mutex<HeldDirectory>,
        entries: Vec<LlmConfigurableProvider>,
    ) -> Result<(), BoxError> {
        {
            let mut held = lock(held);
            if held.released {
                return Err(crate::error::llm_error(
                    "this configurable-provider registration was disposed",
                    BuiltinLlmFailureCode::RegistrationDisposed,
                )
                .into());
            }
            let mut state = lock(&self.registry);
            let mut seen = IndexSet::new();
            for entry in &entries {
                if entry.provider.is_empty()
                    || entry.display_name.is_empty()
                    || entry.settings_ns.is_empty()
                {
                    return Err(crate::error::llm_error("configurable providers need a non-empty provider, displayName, and settingsNs",BuiltinLlmFailureCode::InvalidDirectory).into());
                }
                if entry.settings_path.iter().any(String::is_empty) {
                    return Err(crate::error::llm_error(
                        format!(
                            "configurable provider \"{}\" has an empty settingsPath segment",
                            entry.provider
                        ),
                        BuiltinLlmFailureCode::InvalidDirectory,
                    )
                    .into());
                }
                if !seen.insert(entry.provider.clone())
                    || (state.directory.contains_key(&entry.provider)
                        && !held.entries.iter().any(|e| e.provider == entry.provider))
                {
                    return Err(crate::error::llm_error(
                        format!(
                            "configurable provider \"{}\" is already declared",
                            entry.provider
                        ),
                        BuiltinLlmFailureCode::DuplicateDirectory,
                    )
                    .into());
                }
            }
            for entry in &held.entries {
                state.directory.shift_remove(&entry.provider);
            }
            for entry in &entries {
                state
                    .directory
                    .insert(entry.provider.clone(), entry.clone());
            }
            held.entries = entries;
        }
        Self::emit_adapters_updated(ctx);
        Ok(())
    }
    /// Remote: llm.listConfigurableProviders
    pub fn list_configurable_providers(&self) -> Vec<LlmConfigurableProvider> {
        lock(&self.registry).directory.values().cloned().collect()
    }
    pub fn register_model_discovery(
        &self,
        ctx: &Context,
        settings_ns: String,
        discover: ModelDiscovery,
    ) -> Result<Disposer, BoxError> {
        let runtime = self.clone();
        ctx.try_effect::<BoxError>("llm.registerModelDiscovery()", move |undos| {
            if settings_ns.is_empty() {
                return Err(crate::error::llm_error(
                    "model discovery needs a non-empty settings namespace",
                    BuiltinLlmFailureCode::InvalidDiscovery,
                )
                .into());
            }
            {
                let mut state = lock(&runtime.registry);
                if state.discoveries.contains_key(&settings_ns) {
                    return Err(crate::error::llm_error(
                        format!("model discovery for \"{settings_ns}\" is already registered"),
                        BuiltinLlmFailureCode::DuplicateDiscovery,
                    )
                    .into());
                }
                state.discoveries.insert(settings_ns.clone(), discover);
            }
            undos.push(Undo::sync(move || {
                lock(&runtime.registry)
                    .discoveries
                    .shift_remove(&settings_ns);
            }))?;
            Ok(())
        })
    }
    pub async fn discover_models(
        &self,
        settings_ns: &str,
        request: LlmModelDiscoveryRequest,
        signal: Option<AbortSignal>,
    ) -> Result<Vec<LlmDiscoveredModel>, BoxError> {
        let discover = lock(&self.registry)
            .discoveries
            .get(settings_ns)
            .cloned()
            .ok_or_else(|| {
                crate::error::llm_error(
                    format!("no model discovery is registered for \"{settings_ns}\""),
                    BuiltinLlmFailureCode::NoDiscovery,
                )
            })?;
        if request.provider.as_deref().unwrap_or_default().is_empty()
            && request.base_url.as_deref().unwrap_or_default().is_empty()
        {
            return Err(crate::error::llm_error(
                "model discovery needs a provider route or a baseURL",
                BuiltinLlmFailureCode::InvalidDiscovery,
            )
            .into());
        }
        let models = discover(request, signal).await?;
        let mut seen = IndexSet::new();
        Ok(models
            .into_iter()
            .filter(|m| !m.id.is_empty() && seen.insert(m.id.clone()))
            .collect())
    }
    /// Remote: llm.discoverModels
    pub async fn remote_discover_models(
        &self,
        settings_ns: &str,
        request: LlmModelDiscoveryRequest,
        signal: AbortSignal,
    ) -> Result<Vec<LlmDiscoveredModel>, HarnessError> {
        let details = ModelDiscoveryRejectedDetails {
            settings_ns: settings_ns.into(),
            base_url: request.base_url.clone(),
        };
        self.discover_models(settings_ns, request, Some(signal))
            .await
            .map_err(|e| {
                HarnessError::named("RemoteError", e.to_string(), "llm/model-discovery-rejected")
                    .with_detail(details)
                    .with_cause(e)
            })
    }
    fn registration(&self, provider: &str) -> Result<AdapterRegistration, HarnessError> {
        lock(&self.registry)
            .adapters
            .get(provider)
            .cloned()
            .ok_or_else(|| {
                crate::error::llm_error(
                    format!("no adapter registered for provider \"{provider}\""),
                    BuiltinLlmFailureCode::NoAdapter,
                )
            })
    }
    pub fn provider_retry_policy(
        &self,
        provider: &str,
    ) -> Result<ResolvedRetryPolicy, HarnessError> {
        Ok(self.registration(provider)?.retry_policy)
    }
    pub fn image_request_pricing(
        &self,
        provider: &str,
        model: &str,
    ) -> Option<Arc<dyn LlmImageRequestPricing>> {
        let adapter = lock(&self.registry)
            .adapters
            .get(provider)
            .map(|r| r.adapter.clone())?;
        adapter.image_request_pricing(provider, model)
    }
    pub fn image_request_target(
        &self,
        provider: &str,
        model: &str,
        image: &ImageAttachmentRef,
    ) -> Result<ImageRequestTarget, HarnessError> {
        Ok(self
            .registration(provider)?
            .adapter
            .image_request_target(provider, model, image))
    }
    fn file_read_path(&self, ctx: &Context, reference: &FileAttachmentRef) -> Option<String> {
        let path = ctx.get::<Attachments>()?.file_host_path(reference).ok()??;
        ctx.tag::<HostPathMapping>()
            .and_then(|mapping| (mapping.0)(&path))
    }
    pub fn file_request_text(&self, ctx: &Context, reference: &FileAttachmentRef) -> String {
        file_handle_text(reference, self.file_read_path(ctx, reference).as_deref())
    }
    pub async fn list_models(&self, provider: &str) -> Result<Vec<LlmModelInfo>, BoxError> {
        let models = self
            .registration(provider)?
            .adapter
            .list_models(provider)
            .await?;
        let mut seen = IndexSet::new();
        for model in &models {
            if model.provider != provider
                || model.id.is_empty()
                || model.name.is_empty()
                || !seen.insert(&model.id)
            {
                return Err(crate::error::llm_error(format!("adapter returned invalid or duplicate model metadata for provider \"{provider}\""),BuiltinLlmFailureCode::InvalidCatalog).into());
            }
        }
        Ok(models)
    }
    pub async fn resolve_model_info(
        &self,
        provider: &str,
        model: &str,
        signal: Option<&AbortSignal>,
    ) -> Result<LlmResolvedModelInfo, BoxError> {
        let registration = self.registration(provider)?;
        let resolved = registration
            .adapter
            .resolve_model(provider, model, signal)
            .await?;
        normalize_model_info(provider, model, resolved).map_err(Into::into)
    }
    pub async fn resolve_call_config(
        &self,
        config: LlmCallConfig,
        signal: Option<&AbortSignal>,
    ) -> Result<LlmCallConfig, BoxError> {
        let info = self
            .resolve_model_info(&config.provider, &config.model, signal)
            .await?;
        resolve_call_with_info(config, &info).map_err(Into::into)
    }
    pub async fn prepare_call(
        &self,
        ctx: &Context,
        config: LlmCallConfig,
        signal: Option<AbortSignal>,
        configure: Option<&ConfigureCall<'_>>,
    ) -> Result<PreparedLlmCall, BoxError> {
        check_signal(signal.as_ref())?;
        let registration = self.registration(&config.provider)?;
        let adapter_call = registration
            .adapter
            .clone()
            .prepare_call(
                config.provider.clone(),
                config.model.clone(),
                signal.clone(),
            )
            .await?;
        check_signal(signal.as_ref())?;
        let model_info = normalize_model_info(&config.provider, &config.model, adapter_call.model)?;
        let proposed = if let Some(configure) = configure {
            LlmCallConfig {
                controls: configure(&config.controls, &model_info)?,
                ..config
            }
        } else {
            config
        };
        check_signal(signal.as_ref())?;
        let resolved = resolve_call_with_info(proposed.clone(), &model_info)?;
        let defaults = LlmCallConfigAdapterDefaults {
            reasoning_effort: (proposed.controls.reasoning_effort.is_none()
                && resolved.controls.reasoning_effort.is_some())
            .then_some(true),
            max_tokens: (proposed.controls.max_tokens.is_none()
                && resolved.controls.max_tokens.is_some())
            .then_some(true),
        };
        Ok(PreparedLlmCall {
            config: resolved.clone(),
            retry_policy: registration.retry_policy.clone(),
            context: model_info.context.clone(),
            input_modalities: model_info.model.input_modalities.clone(),
            system_prompt_update: model_info.system_prompt_update.clone(),
            tool_update: model_info.tool_update.clone(),
            adapter_defaults: defaults,
            runtime: self.clone(),
            ctx: ctx.clone(),
            dispatch: PreparedDispatch {
                registration,
                model_info,
                dispatch: adapter_call.dispatch,
                config: resolved.clone(),
            },
            dispatched: Mutex::new(false),
        })
    }
    fn for_adapter(
        &self,
        mut options: GenerateOptions,
        adapter: &Arc<dyn LlmAdapter>,
    ) -> GenerateOptions {
        for message in &mut options.messages {
            let RequestMessage::Durable(message_arc) = message else {
                continue;
            };
            let Message::Assistant(message) = message_arc.as_ref() else {
                continue;
            };
            if message.source.metadata.replay_state.is_none() {
                continue;
            }
            let same = lock(&self.registry)
                .adapters
                .get(&message.source.metadata.provider)
                .is_some_and(|r| Arc::ptr_eq(&r.adapter, adapter));
            if !same {
                let mut next = message.clone();
                next.source.metadata.replay_state = None;
                *message_arc = Arc::new(Message::Assistant(next));
                options.agent_loop_request = false;
            }
        }
        options
    }
    fn adapter_stream(
        &self,
        ctx: Context,
        options: GenerateOptions,
        prepared: Option<PreparedDispatch>,
    ) -> LlmStream {
        let runtime = self.clone();
        let signal = options.signal.clone();
        LlmStream::deferred(
            Box::pin(async move {
                let (registration, model_info, config, dispatch) = if let Some(prepared) = prepared
                {
                    (
                        prepared.registration,
                        prepared.model_info,
                        options.call_config(),
                        prepared.dispatch,
                    )
                } else {
                    let registration = runtime.registration(&options.provider)?;
                    let call = registration
                        .adapter
                        .clone()
                        .prepare_call(
                            options.provider.clone(),
                            options.model.clone(),
                            options.signal.clone(),
                        )
                        .await?;
                    let info = normalize_model_info(&options.provider, &options.model, call.model)?;
                    let config = resolve_call_with_info(options.call_config(), &info)?;
                    (registration, info, config, call.dispatch)
                };
                let config_changed = !call_config_equals(&options.call_config(), &config);
                let mut options = options.with_call_config(&config);
                if config_changed {
                    options.agent_loop_request = false;
                }
                let files = project_files_to_text(&options.messages, |reference| {
                    runtime.file_read_path(&ctx, reference)
                });
                if let std::borrow::Cow::Owned(messages) = files {
                    options.agent_loop_request = false;
                    options.messages = messages;
                }
                if model_info
                    .model
                    .input_modalities
                    .as_ref()
                    .is_some_and(|m| !m.iter().any(|m| m == "image"))
                    && let std::borrow::Cow::Owned(messages) =
                        project_images_for_text_model(&options.messages)
                {
                    options.agent_loop_request = false;
                    options.messages = messages;
                }
                let projected = project_tool_updates(
                    &options.messages,
                    options.tools.as_deref(),
                    model_info.tool_update.as_deref(),
                    options.tool_history.as_ref(),
                )?;
                if matches!(projected.messages, std::borrow::Cow::Owned(_))
                    || projected
                        .tools
                        .as_ref()
                        .is_some_and(|tools| matches!(tools, std::borrow::Cow::Owned(_)))
                {
                    options.agent_loop_request = false;
                }
                let messages = projected.messages.into_owned();
                let tools = projected.tools.map(|t| t.into_owned());
                options.messages = messages;
                options.tools = tools;
                dispatch(runtime.for_adapter(options, &registration.adapter))
            }),
            true,
            signal,
        )
    }
    pub fn stream(&self, ctx: &Context, options: GenerateOptions) -> LlmStream {
        self.stream_with_registration(ctx, options, None, None)
    }
    fn stream_with_registration(
        &self,
        ctx: &Context,
        options: GenerateOptions,
        prepared: Option<PreparedDispatch>,
        config: Option<LlmCallConfig>,
    ) -> LlmStream {
        let event = LlmStreamEvent {
            runtime: self.clone(),
            options: Mutex::new(options),
        };
        let runtime = self.clone();
        let tail_ctx = ctx.clone();
        let waterfall = ctx.waterfall(event, move |event| async move {
            let options = event.options();
            if config
                .as_ref()
                .is_some_and(|config| !call_config_equals(&options.call_config(), config))
            {
                return Ok(LlmStream::chunks([adapter_failure_chunk(
                    &crate::error::llm_error(
                        "prepared LLM call config changed before adapter dispatch",
                        BuiltinLlmFailureCode::InvalidPreparedCall,
                    ),
                    options.signal.as_ref(),
                )]));
            }
            Ok(runtime.adapter_stream(tail_ctx, options, prepared))
        });
        LlmStream::deferred(Box::pin(waterfall), false, None)
    }
}
fn check_signal(signal: Option<&AbortSignal>) -> Result<(), BoxError> {
    if let Some(signal) = signal {
        signal.throw_if_aborted()?;
    }
    Ok(())
}
fn adapter_failure_chunk(
    error: &(dyn std::error::Error + 'static),
    signal: Option<&AbortSignal>,
) -> StreamChunk {
    let failure = normalize_llm_failure(error);
    StreamChunk::finish(FinishReason::failed(
        if signal.is_some_and(AbortSignal::aborted) || failure.code == "ABORTED" {
            "aborted"
        } else {
            "error"
        },
        failure,
    ))
}
pub fn normalize_model_info(
    provider: &str,
    model: &str,
    mut resolved: LlmResolvedModelInfo,
) -> Result<LlmResolvedModelInfo, HarnessError> {
    let error = |description: &str, code| {
        crate::error::llm_error(
            format!("adapter returned {description} for provider \"{provider}\" model \"{model}\""),
            code,
        )
    };
    if resolved.model.provider != provider
        || resolved.model.id != model
        || resolved.model.name.is_empty()
    {
        return Err(error(
            "invalid exact model metadata",
            BuiltinLlmFailureCode::InvalidModelInfo,
        ));
    }
    if resolved
        .context
        .as_ref()
        .is_some_and(|c| c.context_window == 0)
    {
        return Err(error(
            "invalid context metadata",
            BuiltinLlmFailureCode::InvalidModelContext,
        ));
    }
    if resolved
        .system_prompt_update
        .as_ref()
        .is_some_and(|s| s != "in-history")
    {
        return Err(error(
            "invalid system prompt update mode",
            BuiltinLlmFailureCode::InvalidModelInfo,
        ));
    }
    if resolved
        .tool_update
        .as_ref()
        .is_some_and(|s| s != "in-history" && s != "addition-only")
    {
        return Err(error(
            "invalid tool update mode",
            BuiltinLlmFailureCode::InvalidModelInfo,
        ));
    }
    if resolved
        .default_max_tokens
        .is_some_and(|n| n == 0 || n > 9_007_199_254_740_991)
    {
        return Err(error(
            "invalid default maxTokens",
            BuiltinLlmFailureCode::InvalidModelMaxTokens,
        ));
    }
    resolved.model.badge = None;
    if let Some(reasoning) = &resolved.reasoning {
        if reasoning.efforts.is_empty() {
            return Err(error(
                "invalid reasoning metadata",
                BuiltinLlmFailureCode::InvalidModelReasoning,
            ));
        }
        let mut seen = IndexSet::new();
        for effort in &reasoning.efforts {
            if effort.id.as_str().is_empty() || effort.name.is_empty() || !seen.insert(&effort.id) {
                return Err(error(
                    "invalid or duplicate reasoning effort metadata",
                    BuiltinLlmFailureCode::InvalidModelReasoning,
                ));
            }
        }
        if reasoning
            .default_effort
            .as_ref()
            .is_some_and(|id| !seen.contains(id))
        {
            return Err(error(
                "an unknown default reasoning effort",
                BuiltinLlmFailureCode::InvalidModelReasoning,
            ));
        }
    }
    Ok(resolved)
}
fn resolve_call_with_info(
    mut config: LlmCallConfig,
    info: &LlmResolvedModelInfo,
) -> Result<LlmCallConfig, HarnessError> {
    if config.controls.max_tokens.is_none() {
        config.controls.max_tokens = info.default_max_tokens;
    }
    let requested = config.controls.reasoning_effort.clone();
    let effective = requested.or_else(|| {
        info.reasoning
            .as_ref()
            .and_then(|r| r.default_effort.clone())
    });
    if let Some(effective) = effective {
        if info
            .reasoning
            .as_ref()
            .is_none_or(|r| !r.efforts.iter().any(|e| e.id == effective))
        {
            return Err(crate::error::llm_error(
                format!(
                    "provider \"{}\" model \"{}\" does not support reasoning effort \"{effective}\"",
                    config.provider, config.model
                ),
                BuiltinLlmFailureCode::UnsupportedReasoningEffort,
            ));
        }
        config.controls.reasoning_effort = Some(effective);
    }
    Ok(config)
}
#[derive(Default)]
struct HeldRoutes {
    routes: IndexSet<String>,
    released: bool,
}
pub struct AdapterRegistrationHandle {
    runtime: LlmRuntime,
    ctx: Context,
    adapter: Arc<dyn LlmAdapter>,
    held: Arc<Mutex<HeldRoutes>>,
    disposer: Mutex<Option<Disposer>>,
}
impl AdapterRegistrationHandle {
    pub fn replace(&self, providers: Vec<String>) -> Result<(), BoxError> {
        self.runtime
            .replace_routes(&self.ctx, &self.held, &self.adapter, providers)
    }
    pub fn dispose(&self) {
        if let Some(disposer) = lock(&self.disposer).take() {
            drop(disposer.dispose());
        }
    }
}
#[derive(Default)]
struct HeldDirectory {
    entries: Vec<LlmConfigurableProvider>,
    released: bool,
}
pub struct DirectoryRegistrationHandle {
    runtime: LlmRuntime,
    ctx: Context,
    held: Arc<Mutex<HeldDirectory>>,
    disposer: Mutex<Option<Disposer>>,
}
impl DirectoryRegistrationHandle {
    pub fn replace(&self, entries: Vec<LlmConfigurableProvider>) -> Result<(), BoxError> {
        self.runtime
            .replace_directory(&self.ctx, &self.held, entries)
    }
    pub fn dispose(&self) {
        if let Some(disposer) = lock(&self.disposer).take() {
            drop(disposer.dispose());
        }
    }
}
#[derive(Clone)]
struct PreparedDispatch {
    registration: AdapterRegistration,
    model_info: LlmResolvedModelInfo,
    dispatch: LlmDispatch,
    config: LlmCallConfig,
}
pub struct PreparedLlmCall {
    pub config: LlmCallConfig,
    pub retry_policy: ResolvedRetryPolicy,
    pub context: Option<LlmModelContext>,
    pub input_modalities: Option<Vec<ModelModality>>,
    pub system_prompt_update: Option<SystemPromptUpdate>,
    pub tool_update: Option<ToolUpdate>,
    pub adapter_defaults: LlmCallConfigAdapterDefaults,
    runtime: LlmRuntime,
    ctx: Context,
    dispatch: PreparedDispatch,
    dispatched: Mutex<bool>,
}
impl PreparedLlmCall {
    pub fn stream(&self, options: GenerateOptions) -> Result<LlmStream, HarnessError> {
        let mut dispatched = lock(&self.dispatched);
        if *dispatched {
            return Err(crate::error::llm_error(
                "a prepared LLM call can only be dispatched once",
                BuiltinLlmFailureCode::InvalidPreparedCall,
            ));
        }
        if !call_config_equals(&options.call_config(), &self.dispatch.config) {
            return Err(crate::error::llm_error(
                "prepared LLM call config changed before adapter dispatch",
                BuiltinLlmFailureCode::InvalidPreparedCall,
            ));
        }
        *dispatched = true;
        Ok(self.runtime.stream_with_registration(
            &self.ctx,
            options,
            Some(self.dispatch.clone()),
            Some(self.dispatch.config.clone()),
        ))
    }
}
hoh_kernel::service!(pub Llm: LlmRuntime = "llm");
pub struct LlmPlugin;
impl hoh_kernel::Plugin for LlmPlugin {
    const NAME: &'static str = "llm";
    type Config = hoh_kernel::NoConfig;
    type Deps = ();
    async fn apply(
        ctx: Context,
        _deps: (),
        _config: hoh_kernel::Cfg<Self::Config>,
    ) -> Result<(), BoxError> {
        ctx.provide::<Llm>(Arc::new(LlmRuntime::new())).await?;
        Ok(())
    }
}
