//! Declarative agent capability sets, activation and session binding.

pub mod composition_inventory;
pub mod definition;
pub mod display;
pub mod mount;
pub mod preset;
pub mod session;
mod shim;
pub mod types;

pub use composition_inventory::{
    AgentPresetComposition, AgentPresetCompositionRow, AgentPresetInspection, AgentPresetModule,
    CompositionRowEnablement, DisabledExpressionEvaluator, active_composition_modules,
    definition_composition, mounted_composition_rows,
};
pub use definition::{PresetDefinition, entry_list_problem};
pub use display::{is_built_in_preset, preset_display_text};
pub use mount::{
    PresetMount, PresetTree, RowAudit, audit_rows, leaked_services, mount_preset,
    mount_preset_with_runtime, service_for_mount,
};
pub use preset::{AgentPreset, Config};
pub use session::agent_preset_projection_definition;
pub use shim::PresetRuntime;
pub use types::*;

use feruca::{Collator, Locale, Tailoring};
use futures_util::FutureExt;
use hoh_agent::{Agent, Agents};
use hoh_error::{BoxError, HarnessError};
use hoh_kernel::{Cfg, Context, Disposer, Inject, Plugin, Service, Undo};
use hoh_loader::LoaderKey;
use hoh_scope::{
    CreateScopeOptions, Scope, ScopeKey, ScopeParentBinding, bind_scope_parent, create_scope,
    scope_of, scope_parent_of,
};
use hoh_session::{Session, SessionEventEmitted, SessionId};
use hoh_session_projection::{ProjectionHandle, SessionProjections};
use hoh_tools::ToolsChange;
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex, MutexGuard, Weak,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio::sync::Notify;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct RegistryError(pub String);

fn error(message: impl Into<String>) -> BoxError {
    Box::new(RegistryError(message.into()))
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn panic_message(value: Box<dyn std::any::Any + Send>) -> String {
    value
        .downcast_ref::<&str>()
        .map(|value| (*value).to_owned())
        .or_else(|| value.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "event listener panicked".into())
}

struct Definition {
    config: PresetDefinition,
    generation: Mutex<Option<Arc<Generation>>>,
    broken: Mutex<Option<String>>,
    ready: AtomicBool,
    ready_notify: Notify,
}

impl Definition {
    fn new(config: PresetDefinition) -> Self {
        Self {
            config,
            generation: Mutex::new(None),
            broken: Mutex::new(None),
            ready: AtomicBool::new(false),
            ready_notify: Notify::new(),
        }
    }

    async fn wait_ready(&self) {
        while !self.ready.load(Ordering::Acquire) {
            let notified = self.ready_notify.notified();
            if self.ready.load(Ordering::Acquire) {
                break;
            }
            notified.await;
        }
    }

    fn finish(&self) {
        self.ready.store(true, Ordering::Release);
        self.ready_notify.notify_waiters();
    }
}

struct Generation {
    scope: Scope,
    key: ScopeKey,
    mount: PresetMount,
    users: AtomicUsize,
    retired: AtomicBool,
    collected: AtomicBool,
}

struct Binding {
    parent: ScopeParentBinding,
    generation: Arc<Generation>,
}

type ConfigReader = Arc<dyn Fn() -> Config + Send + Sync>;

struct Inner {
    owner: Context,
    runtime: PresetRuntime,
    loader: Option<Service<LoaderKey>>,
    agents: Option<Service<Agents>>,
    projection: Option<ProjectionHandle<Option<String>>>,
    config: ConfigReader,
    definitions: Mutex<IndexMap<String, Arc<Definition>>>,
    generations: Mutex<IndexMap<ScopeKey, Arc<Generation>>>,
    bindings: Mutex<IndexMap<ScopeKey, Binding>>,
    switches: Mutex<IndexMap<SessionId, Weak<tokio::sync::Mutex<()>>>>,
}

#[derive(Clone)]
pub struct AgentPresetRegistry {
    inner: Arc<Inner>,
}

hoh_kernel::service!(pub AgentPresets: AgentPresetRegistry = "agentPresets");

impl AgentPresetRegistry {
    /// Construct a registry against an explicit fixed Loader runtime.
    ///
    /// The native plugin uses the same constructor with live `Cfg` and services.
    pub fn new(owner: &Context, config: Config, runtime: PresetRuntime) -> Result<Self, BoxError> {
        Self::build(
            owner,
            Arc::new(move || config.clone()),
            runtime,
            None,
            None,
            None,
        )
    }

    /// Construct the fixed-interface registry with real selection services.
    pub fn new_with_services(
        owner: &Context,
        config: Config,
        runtime: PresetRuntime,
        agents: Service<Agents>,
        projection: ProjectionHandle<Option<String>>,
    ) -> Result<Self, BoxError> {
        Self::build(
            owner,
            Arc::new(move || config.clone()),
            runtime,
            None,
            Some(agents),
            Some(projection),
        )
    }

    fn build(
        owner: &Context,
        config: ConfigReader,
        runtime: PresetRuntime,
        loader: Option<Service<LoaderKey>>,
        agents: Option<Service<Agents>>,
        projection: Option<ProjectionHandle<Option<String>>>,
    ) -> Result<Self, BoxError> {
        let registry = Self {
            inner: Arc::new(Inner {
                owner: owner.clone(),
                runtime,
                loader,
                agents,
                projection,
                config,
                definitions: Mutex::new(IndexMap::new()),
                generations: Mutex::new(IndexMap::new()),
                bindings: Mutex::new(IndexMap::new()),
                switches: Mutex::new(IndexMap::new()),
            }),
        };
        Ok(registry)
    }

    pub fn default_id(&self) -> String {
        let config = (self.inner.config)();
        config.selected_default.0.unwrap_or(config.default)
    }

    pub fn preset_for_session(&self, session: &Session) -> Result<Option<String>, BoxError> {
        self.inner
            .projection
            .as_ref()
            .ok_or_else(|| error("agent preset projection is unavailable"))?
            .state_of(session)
            .map(|state| (*state).clone())
    }

    pub async fn register(
        &self,
        ctx: &Context,
        definition: PresetDefinition,
    ) -> Result<Disposer, BoxError> {
        if hoh_util_values::js_trim(&definition.id).is_empty() {
            return Err(error("Preset id must not be empty"));
        }
        let id = definition.id.clone();
        let record = Arc::new(Definition::new(definition));
        {
            let mut definitions = lock(&self.inner.definitions);
            if definitions.contains_key(&id) {
                return Err(error(format!("Duplicate agent preset: {id}")));
            }
            definitions.insert(id.clone(), record.clone());
        }
        if let Err(cause) = self.activate(record.clone()).await {
            let mut definitions = lock(&self.inner.definitions);
            if definitions
                .get(&id)
                .is_some_and(|current| Arc::ptr_eq(current, &record))
            {
                definitions.shift_remove(&id);
            }
            return Err(cause);
        }
        let registry = self.clone();
        let owned = record.clone();
        match ctx.effect("agentPresets.register()", move || {
            Undo::future(move || async move {
                registry.unregister(&id, &owned).await;
                Ok(())
            })
        }) {
            Ok(disposer) => Ok(disposer),
            Err(cause) => {
                self.unregister(&record.config.id, &record).await;
                Err(Box::new(cause))
            }
        }
    }

    async fn activate(&self, record: Arc<Definition>) -> Result<(), BoxError> {
        let key = ScopeKey::new();
        let scope = match create_scope(
            &self.inner.owner,
            key.clone(),
            CreateScopeOptions::default(),
        ) {
            Ok(scope) => scope,
            Err(cause) => {
                record.finish();
                return Err(cause);
            }
        };
        let mounted = mount_preset_with_runtime(
            &scope.ctx,
            &record.config.id,
            &record.config,
            &self.inner.runtime,
        )
        .await;
        match mounted {
            Ok(mount) => {
                let generation = Arc::new(Generation {
                    scope,
                    key: key.clone(),
                    mount,
                    users: AtomicUsize::new(0),
                    retired: AtomicBool::new(false),
                    collected: AtomicBool::new(false),
                });
                lock(&self.inner.generations).insert(key, generation.clone());
                *lock(&record.generation) = Some(generation);
            }
            Err(cause) => {
                let message = cause.to_string();
                *lock(&record.broken) = Some(message.clone());
                self.inner
                    .owner
                    .logger()
                    .warn(format_args!("agent preset {}: {message}", record.config.id));
                scope.dispose().await;
            }
        }
        record.finish();
        Ok(())
    }

    async fn unregister(&self, id: &str, record: &Arc<Definition>) {
        let removed = {
            let mut definitions = lock(&self.inner.definitions);
            definitions
                .get(id)
                .is_some_and(|current| Arc::ptr_eq(current, record))
                .then(|| definitions.shift_remove(id))
                .flatten()
        };
        if removed.is_none() {
            return;
        }
        record.wait_ready().await;
        let generation = lock(&record.generation).clone();
        if let Some(generation) = generation {
            generation.retired.store(true, Ordering::Release);
            self.collect(generation).await;
        }
    }

    async fn collect(&self, generation: Arc<Generation>) {
        if !generation.retired.load(Ordering::Acquire)
            || generation.users.load(Ordering::Acquire) != 0
            || generation.collected.swap(true, Ordering::AcqRel)
        {
            return;
        }
        {
            let mut generations = lock(&self.inner.generations);
            if generations
                .get(&generation.key)
                .is_some_and(|current| Arc::ptr_eq(current, &generation))
            {
                generations.shift_remove(&generation.key);
            }
        }
        generation.scope.dispose().await;
    }

    fn collect_without_waiting(&self, generation: Arc<Generation>) {
        if !generation.retired.load(Ordering::Acquire)
            || generation.users.load(Ordering::Acquire) != 0
            || generation.collected.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let mut generations = lock(&self.inner.generations);
        if generations
            .get(&generation.key)
            .is_some_and(|current| Arc::ptr_eq(current, &generation))
        {
            generations.shift_remove(&generation.key);
        }
        drop(generations);
        drop(generation.scope.dispose());
    }

    async fn diagnostic(&self, record: &Arc<Definition>) -> Option<String> {
        record.wait_ready().await;
        let generation = lock(&record.generation).clone();
        let Some(generation) = generation else {
            return lock(&record.broken).clone();
        };
        let mut audit = audit_rows(&generation.mount.tree).await;
        if !audit.pending.is_empty()
            && let Some(loader) = &self.inner.loader
        {
            loader.settled().await;
            audit = audit_rows(&generation.mount.tree).await;
        }
        let mut lines = audit.failed;
        lines.extend(audit.pending);
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    fn generation_for(&self, ctx: &Context) -> Option<Arc<Generation>> {
        let key = scope_of(ctx)?;
        let parent = scope_parent_of(&key)?;
        lock(&self.inner.generations).get(&parent).cloned()
    }

    pub async fn inspect_compositions(&self, ctx: Option<&Context>) -> Vec<AgentPresetInspection> {
        let generations: Vec<Arc<Generation>> = match ctx {
            Some(ctx) => self.generation_for(ctx).into_iter().collect(),
            None => lock(&self.inner.generations).values().cloned().collect(),
        };
        futures_util::future::join_all(generations.into_iter().map(|generation| async move {
            AgentPresetInspection {
                id: generation.mount.preset_id.clone(),
                modules: active_composition_modules(&generation.mount),
                leaked_services: leaked_services(&self.inner.owner, &generation.mount.fiber).await,
            }
        }))
        .await
    }

    pub async fn list(&self) -> Vec<AgentPreset> {
        let records: Vec<_> = lock(&self.inner.definitions).values().cloned().collect();
        let mut rows =
            futures_util::future::join_all(records.into_iter().map(|record| async move {
                let broken = self.diagnostic(&record).await;
                AgentPreset {
                    id: record.config.id.clone(),
                    name: record.config.name.clone(),
                    description: record.config.description.clone(),
                    order: record.config.order,
                    broken,
                }
            }))
            .await;
        let mut collator = Collator::new(Tailoring::Cldr(Locale::Root), false, false);
        rows.sort_by(|left, right| {
            let delta = left.order.unwrap_or(f64::INFINITY) - right.order.unwrap_or(f64::INFINITY);
            if delta < 0.0 {
                std::cmp::Ordering::Less
            } else if delta > 0.0 {
                std::cmp::Ordering::Greater
            } else {
                collator.collate(&left.id, &right.id)
            }
        });
        rows
    }

    /// Remote: agentPresets.list
    pub async fn remote_export_list(&self) -> AgentPresetRoster {
        let default_id = self.default_id();
        AgentPresetRoster {
            presets: self
                .list()
                .await
                .into_iter()
                .map(|row| AgentPresetRow {
                    is_default: row.id == default_id,
                    id: row.id,
                    name: row.name,
                    description: row.description,
                    order: row.order,
                    broken: row.broken,
                })
                .collect(),
        }
    }

    fn not_found(&self, wanted: String) -> HarnessError {
        let available = lock(&self.inner.definitions).keys().cloned().collect();
        HarnessError::named(
            "RemoteError",
            format!("Unknown agent preset: {wanted}"),
            "agent-preset/not-found",
        )
        .with_detail(AgentPresetNotFoundDetails {
            agent_preset: wanted,
            available,
        })
    }

    pub async fn resolve(&self, id: Option<&str>) -> Result<AgentPreset, HarnessError> {
        let wanted = id.map_or_else(|| self.default_id(), str::to_owned);
        let record = lock(&self.inner.definitions).get(&wanted).cloned();
        let Some(record) = record else {
            return Err(self.not_found(wanted));
        };
        let broken = self.diagnostic(&record).await;
        Ok(AgentPreset {
            id: wanted,
            name: None,
            description: None,
            order: None,
            broken,
        })
    }

    /// Remote: agentPresets.read
    pub fn read_document(&self, agent_preset: &str) -> Result<AgentPresetDocument, HarnessError> {
        let record = lock(&self.inner.definitions).get(agent_preset).cloned();
        let Some(record) = record else {
            return Err(self.not_found(agent_preset.to_owned()));
        };
        Ok(AgentPresetDocument {
            agent_preset: record.config.id.clone(),
            content: shim::stringify_entry_yaml(&record.config.plugins),
            name: record.config.name.clone(),
            description: record.config.description.clone(),
        })
    }

    async fn retain(&self, id: Option<&str>) -> Result<Arc<Generation>, BoxError> {
        let wanted = id.map_or_else(|| self.default_id(), str::to_owned);
        loop {
            let record = lock(&self.inner.definitions).get(&wanted).cloned();
            let Some(record) = record else {
                return Err(Box::new(self.not_found(wanted)));
            };
            let broken = self.diagnostic(&record).await;
            let definitions = lock(&self.inner.definitions);
            if !definitions
                .get(&wanted)
                .is_some_and(|current| Arc::ptr_eq(current, &record))
            {
                continue;
            }
            let generation = lock(&record.generation).clone();
            let Some(generation) = generation else {
                let reason = broken.unwrap_or_default();
                return Err(Box::new(
                    HarnessError::named("RemoteError", reason.clone(), "agent-preset/invalid")
                        .with_detail(AgentPresetInvalidDetails {
                            agent_preset: wanted,
                            reason,
                        }),
                ));
            };
            if let Some(reason) = broken {
                return Err(Box::new(
                    HarnessError::named("RemoteError", reason.clone(), "agent-preset/invalid")
                        .with_detail(AgentPresetInvalidDetails {
                            agent_preset: wanted,
                            reason,
                        }),
                ));
            }
            generation.users.fetch_add(1, Ordering::AcqRel);
            return Ok(generation);
        }
    }

    async fn release_generation(&self, generation: Arc<Generation>) {
        generation.users.fetch_sub(1, Ordering::AcqRel);
        self.collect(generation).await;
    }

    fn join(
        &self,
        ctx: &Context,
        key: ScopeKey,
        generation: Arc<Generation>,
    ) -> Result<(), BoxError> {
        let parent = bind_scope_parent(&key, generation.key.clone())?;
        generation.users.fetch_add(1, Ordering::AcqRel);
        let registry = self.clone();
        let generation_for_rollback = generation.clone();
        let key_for_effect = key.clone();
        match ctx.effect("agent-preset.binding", move || {
            lock(&registry.inner.bindings)
                .insert(key_for_effect.clone(), Binding { parent, generation });
            Undo::future(move || async move {
                let binding = lock(&registry.inner.bindings).shift_remove(&key_for_effect);
                if let Some(binding) = binding {
                    registry.release_generation(binding.generation).await;
                }
                Ok(())
            })
        }) {
            Ok(_) => Ok(()),
            Err(cause) => {
                generation_for_rollback.users.fetch_sub(1, Ordering::AcqRel);
                Err(Box::new(cause))
            }
        }
    }

    async fn bind(&self, ctx: &Context, generation: Arc<Generation>) -> Result<(), BoxError> {
        let key =
            scope_of(ctx).ok_or_else(|| error("Agent preset binding requires a scoped context"))?;
        let old = {
            let mut bindings = lock(&self.inner.bindings);
            if let Some(binding) = bindings.get_mut(&key) {
                if Arc::ptr_eq(&binding.generation, &generation) {
                    return Ok(());
                }
                binding.parent.rebind(generation.key.clone())?;
                generation.users.fetch_add(1, Ordering::AcqRel);
                Some(std::mem::replace(
                    &mut binding.generation,
                    generation.clone(),
                ))
            } else {
                None
            }
        };
        if let Some(old) = old {
            self.release_generation(old).await;
            Ok(())
        } else {
            self.join(ctx, key, generation)
        }
    }

    pub async fn mount(&self, ctx: &Context, id: Option<&str>) -> Result<AgentPreset, BoxError> {
        let generation = self.retain(id).await?;
        let result = self.bind(ctx, generation.clone()).await;
        let preset = AgentPreset {
            id: generation.mount.preset_id.clone(),
            name: None,
            description: None,
            order: None,
            broken: None,
        };
        self.release_generation(generation).await;
        result.map(|()| preset)
    }

    pub fn compose_from(
        &self,
        ctx: &Context,
        parent: &Context,
    ) -> Result<Option<String>, BoxError> {
        let Some(generation) = self.generation_for(parent) else {
            if let Some(registry) = parent.get::<AgentPresets>()
                && !Arc::ptr_eq(&registry.inner, &self.inner)
                && registry.composed_preset(parent).is_some()
            {
                return Err(error("Parent preset revision is unavailable"));
            }
            return Ok(None);
        };
        let key = scope_of(ctx).ok_or_else(|| error("Child preset binding requires a scope"))?;
        if lock(&self.inner.bindings).contains_key(&key) {
            return Err(error("Child already joined a preset"));
        }
        let id = generation.mount.preset_id.clone();
        self.join(ctx, key, generation)?;
        Ok(Some(id))
    }

    pub fn composed_preset(&self, ctx: &Context) -> Option<String> {
        self.generation_for(ctx)
            .map(|generation| generation.mount.preset_id.clone())
    }

    pub async fn service_for<K: hoh_kernel::ServiceKey>(
        &self,
        agent_ctx: &Context,
    ) -> Option<Service<K>> {
        let generation = self.generation_for(agent_ctx)?;
        service_for_mount::<K>(agent_ctx, &generation.mount).await
    }

    pub async fn recompose(&self, ctx: &Context, id: &str) -> Result<AgentPreset, BoxError> {
        let preset = self.mount(ctx, Some(id)).await?;
        if let Err(cause) = catch_unwind(AssertUnwindSafe(|| self.inner.owner.emit(&ToolsChange))) {
            self.inner.owner.logger().warn(format_args!(
                "Preset tools observer: {}",
                panic_message(cause)
            ));
        }
        Ok(preset)
    }

    pub async fn select(
        &self,
        agent: Arc<dyn Agent>,
        agent_preset: impl Into<String>,
    ) -> Result<String, BoxError> {
        let session_id = agent.id().clone();
        let switch = {
            let mut switches = lock(&self.inner.switches);
            match switches.get(&session_id).and_then(Weak::upgrade) {
                Some(switch) => switch,
                None => {
                    let switch = Arc::new(tokio::sync::Mutex::new(()));
                    switches.insert(session_id.clone(), Arc::downgrade(&switch));
                    switch
                }
            }
        };
        let turn = switch.lock().await;
        let result = AssertUnwindSafe(self.process_select(agent, agent_preset.into()))
            .catch_unwind()
            .await;
        drop(turn);
        {
            let mut switches = lock(&self.inner.switches);
            if switches.get(&session_id).is_some_and(|current| {
                Weak::ptr_eq(current, &Arc::downgrade(&switch)) && current.strong_count() == 1
            }) {
                switches.shift_remove(&session_id);
            }
        }
        match result {
            Ok(result) => result,
            Err(cause) => Err(error(format!(
                "agent preset selection panicked: {}",
                panic_message(cause)
            ))),
        }
    }

    async fn process_select(
        &self,
        agent: Arc<dyn Agent>,
        agent_preset: String,
    ) -> Result<String, BoxError> {
        let agents = self
            .inner
            .agents
            .as_ref()
            .ok_or_else(|| error("agents service is unavailable"))?;
        let boundary = agents.turn_boundary_of(agent.session())?;
        if boundary.open_turn_start_seq.is_some() || boundary.last_turn > 0 {
            return Err(Box::new(
                HarnessError::named(
                    "RemoteError",
                    "This session has already started",
                    "agent-preset/locked",
                )
                .with_detail(AgentPresetLockedDetails {
                    session_id: agent.id().clone(),
                    agent_preset,
                }),
            ));
        }
        let current = lock(&self.inner.definitions)
            .get(&agent_preset)
            .and_then(|record| lock(&record.generation).clone());
        if current.as_ref().is_some_and(|current| {
            self.generation_for(agent.ctx())
                .as_ref()
                .is_some_and(|joined| Arc::ptr_eq(current, joined))
        }) && self.preset_for_session(agent.session())?.as_deref() == Some(&agent_preset)
        {
            return Ok(agent_preset);
        }
        let preset = self.recompose(agent.ctx(), &agent_preset).await?;
        agent.session().append(
            "agent-preset/selected",
            json!({"agentPreset": preset.id}),
            None,
        )?;
        Ok(preset.id)
    }

    pub async fn acquire_scope(&self, id: Option<&str>) -> Result<PresetScopeLease, BoxError> {
        let generation = self.retain(id).await?;
        Ok(PresetScopeLease {
            key: generation.key.clone(),
            registry: self.clone(),
            generation,
            disposed: AtomicBool::new(false),
        })
    }

    pub async fn composition_inventory(&self) -> Vec<AgentPresetComposition> {
        let records: Vec<_> = lock(&self.inner.definitions).values().cloned().collect();
        futures_util::future::join_all(records.into_iter().map(|record| async move {
            let broken = self.diagnostic(&record).await;
            let generation = lock(&record.generation).clone();
            let rows = generation.map_or_else(
                || {
                    definition_composition(&record.config.plugins, &|_| {
                        Err(error("Inactive definition"))
                    })
                    .unwrap_or_default()
                },
                |generation| mounted_composition_rows(&generation.mount),
            );
            AgentPresetComposition {
                id: record.config.id.clone(),
                name: record.config.name.clone(),
                description: record.config.description.clone(),
                is_default: record.config.id == self.default_id(),
                broken,
                rows,
            }
        }))
        .await
    }
}

pub struct PresetScopeLease {
    pub key: ScopeKey,
    registry: AgentPresetRegistry,
    generation: Arc<Generation>,
    disposed: AtomicBool,
}

impl PresetScopeLease {
    pub async fn dispose(&self) {
        if self.disposed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.registry
            .release_generation(self.generation.clone())
            .await;
    }
}

impl Drop for PresetScopeLease {
    fn drop(&mut self) {
        if self.disposed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.generation.users.fetch_sub(1, Ordering::AcqRel);
        self.registry
            .collect_without_waiting(self.generation.clone());
    }
}

#[derive(Inject)]
pub struct AgentPresetRegistryDeps {
    loader: Service<LoaderKey>,
    projections: Service<SessionProjections>,
    agents: Service<Agents>,
}

impl Plugin for AgentPresetRegistry {
    const NAME: &'static str = "AgentPresetRegistry";
    type Config = Config;
    type Deps = AgentPresetRegistryDeps;

    async fn apply(
        ctx: Context,
        deps: AgentPresetRegistryDeps,
        config: Cfg<Config>,
    ) -> Result<(), BoxError> {
        let projection = deps
            .projections
            .register(&ctx, agent_preset_projection_definition())
            .await?;
        let read_config: ConfigReader = Arc::new(move || (*config.get()).clone());
        // SHIM(hoh-loader): use builtins until Loader exposes its active registry/env.
        let registry = Self::build(
            &ctx,
            read_config,
            PresetRuntime::fallback(),
            Some(deps.loader.clone()),
            Some(deps.agents.clone()),
            Some(projection),
        )?;
        {
            let owner = ctx.clone();
            ctx.on_contained::<SessionEventEmitted>(move |event| {
                if event.event.event_type() == "agent-preset/selected"
                    && let Some(agent_preset) = event
                        .event
                        .data()
                        .get("agentPreset")
                        .and_then(Value::as_str)
                {
                    owner.emit(&AgentPresetSelected {
                        session_id: event.session.id().clone(),
                        agent_preset: agent_preset.to_owned(),
                    });
                }
                Ok(())
            })?;
        }
        // SHIM(hoh-settings): configure({ auto: false }, fiber) waits for its accepted service API.
        ctx.provide::<AgentPresets>(Arc::new(registry)).await?;
        Ok(())
    }
}
