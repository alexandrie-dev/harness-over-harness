//! Port of @deepseek-ai/dsh-tools (packages/core/tools).
//! Tool definitions and scoped dispatch to plugin-owned executors.
// The error constructors keep the source's `new` names and return `HarnessError`; the callback types are
// the source's function-typed members and are used once each.
#![allow(clippy::new_ret_no_self, clippy::type_complexity)]

pub mod batch;
pub mod contracts;
pub mod error_codes;
pub mod errors;
pub mod events;
pub mod executor;
pub mod json_schema;
pub mod pipeline;
pub mod presentation;
pub mod schema;
pub mod shim;

use std::{
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex},
};

use futures_util::FutureExt;
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Disposer, Inject, Plugin, Service, Undo};
use hoh_llm::ToolSchema;
use hoh_scope::{
    AnonymousEntries, NamedEntries, ScopeKey, ScopeLayer, ScopedLayers, WeakScopeKey,
    scope_chain_of, scope_of, store::EffectOptions,
};
use hoh_system_prompt::{
    AssembleContext, PromptSection, PromptText, SystemPromptKey, ToolProviderResult,
};
use indexmap::{IndexMap, IndexSet};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use batch::execute_tool_batch;
pub use contracts::*;
pub use error_codes::{
    APPROVAL_CANCELLED, APPROVAL_ERROR_NAME, APPROVAL_REJECTED, APPROVAL_UNAVAILABLE,
    TOOL_CONTEXT_UNAVAILABLE,
};
pub use errors::ToolsError;
pub use events::{ToolsChange, ToolsExecute, ToolsPostExecute, ToolsPreExecute, ToolsResult};
pub use executor::*;
pub use hoh_agent::{Agent, ModelSelection};
pub use json_schema::{
    JsonSchemaError, JsonSchemaNode, assert_object_json_schema, assert_supported_json_schema,
    is_json_schema_record, is_plain_json_array, is_plain_json_record, validate_json_schema_value,
};
pub use pipeline::{
    ScheduledToolDispatch, ScheduledToolPreparation, ToolExecutionPipeline, ToolExecutionSource,
    tool_error_message,
};
pub use presentation::*;
pub use schema::*;

/// One restriction compiled at registration for repeated live-global lookup.
struct CompiledToolRestriction {
    allow: Option<std::collections::HashSet<String>>,
    deny: Option<std::collections::HashSet<String>>,
}

/// One registered definition and its step-time activation condition.
struct ToolRegistration {
    definition: Arc<ToolDefinition>,
    when: Option<WhenFn>,
}

/// One scope's complete registry view, derived in a single layer traversal.
struct ToolView {
    /// Visible definitions after restrictions, scoped shadowing, and inherited restrictions.
    visible: IndexMap<String, Arc<ToolDefinition>>,
    /// Registrations behind `visible`, keyed by the same names.
    registrations: IndexMap<String, Arc<ToolRegistration>>,
    /// Pre-restriction capability names used by prompt-order validation.
    known_names: IndexSet<String>,
    /// Current global names that a scoped restriction may name.
    restrictable_names: IndexSet<String>,
}

/// One scope's complete tool-registry contribution.
struct ToolLayer {
    tools: NamedEntries<Arc<ToolRegistration>>,
    restrictions: AnonymousEntries<Arc<CompiledToolRestriction>>,
}

impl ToolLayer {
    fn new(scoped: bool) -> Self {
        Self {
            tools: NamedEntries::new(move |name| {
                let message = if scoped {
                    format!("tool \"{name}\" is already registered in this scope")
                } else {
                    format!(
                        "tool \"{name}\" is already registered (for a per-agent variant, register through that agent's `agent.ctx` instead)"
                    )
                };
                Box::new(ToolsError::Error(message)) as BoxError
            }),
            restrictions: AnonymousEntries::new(),
        }
    }

    /// Whether every compiled restriction in this layer admits a global tool name.
    fn admits(&self, name: &str) -> bool {
        for filter in self.restrictions.values() {
            if filter
                .allow
                .as_ref()
                .is_some_and(|allow| !allow.contains(name))
                || filter.deny.as_ref().is_some_and(|deny| deny.contains(name))
            {
                return false;
            }
        }
        true
    }
}

impl ScopeLayer for ToolLayer {
    fn is_empty(&self) -> bool {
        self.tools.is_empty() && self.restrictions.is_empty()
    }
}

struct GuardLayer {
    guards: AnonymousEntries<ToolGuard>,
}

impl ScopeLayer for GuardLayer {
    fn is_empty(&self) -> bool {
        self.guards.is_empty()
    }
}

struct Registration {
    executor: Mutex<Option<Arc<dyn ToolExecutor>>>,
}

#[derive(Default)]
struct ExecutorTable {
    root: Option<Arc<Registration>>,
    // The source keys a WeakMap: a dead scope's entry is pruned, a live scope keeps its entry.
    scoped: HashMap<WeakScopeKey, Arc<Registration>>,
}

impl ExecutorTable {
    /// Drop the entries of scopes whose last key handle is gone.
    fn prune_dead(&mut self) {
        self.scoped.retain(|key, _| key.upgrade().is_some());
    }
}

struct ToolsInner {
    system_prompt: Service<SystemPromptKey>,
    layers: ScopedLayers<ToolLayer>,
    guards: ScopedLayers<GuardLayer>,
    executors: Mutex<ExecutorTable>,
}

/// Concrete tool registry and scoped executor dispatcher. Methods that create an effect take the
/// caller's context first: the registration is an effect of that context and its scope decides the layer.
#[derive(Clone)]
pub struct ToolsService(Arc<ToolsInner>);

hoh_kernel::service!(pub Tools: ToolsService = "tools");

/// The plugin has no settings. Like the source's object schema it ignores unrelated fields, and it
/// rejects the removed `mode` and `maxParallelSubCalls` fields (`z.never()` in the source).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ToolsConfig {
    /// Removed. Mount a native or PTC tool executor instead.
    #[serde(default, skip_serializing, deserialize_with = "reject_removed")]
    #[schemars(schema_with = "removed_field")]
    pub mode: RemovedField,
    /// Moved to the PTC tool executor configuration.
    #[serde(
        default,
        rename = "maxParallelSubCalls",
        skip_serializing,
        deserialize_with = "reject_removed"
    )]
    #[schemars(schema_with = "removed_field")]
    pub max_parallel_sub_calls: RemovedField,
}

/// The value of a removed configuration field: it can only be absent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RemovedField;

fn reject_removed<'de, D: serde::Deserializer<'de>>(_: D) -> Result<RemovedField, D::Error> {
    Err(serde::de::Error::custom(
        "this field was removed from the tools configuration",
    ))
}

fn removed_field(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::Schema::from(false)
}

/// Mount dependencies of [`ToolRuntime`].
#[derive(Inject)]
pub struct Deps {
    system_prompt: Service<SystemPromptKey>,
}

/// The tool registry plugin (`ToolsService` / `ToolRuntime` of the source).
pub struct ToolRuntime;

impl Plugin for ToolRuntime {
    const NAME: &'static str = "ToolsService";
    type Config = ToolsConfig;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, _config: Cfg<ToolsConfig>) -> Result<(), BoxError> {
        let service = ToolsService::new(&ctx, deps.system_prompt.clone())?;
        let provider = service.clone();
        deps.system_prompt
            .tools(&ctx, move |assemble| provider.describe_for(assemble))?;
        ctx.provide::<Tools>(Arc::new(service)).await?;
        Ok(())
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Emit `tools/change` unfiltered; a listener that panics makes the registration fail.
fn notify_change(notifier: &Context) -> Result<(), BoxError> {
    catch_unwind(AssertUnwindSafe(|| notifier.emit(&ToolsChange))).map_err(errors::panic_error)
}

/// Invoke and await one activation condition; a panic while calling it or while polling its future is
/// the failure a throwing or rejecting condition is in the source.
async fn evaluate_condition(
    when: &WhenFn,
    context: &ToolActivationContext,
) -> Result<bool, BoxError> {
    let future = errors::catch_panic(|| when(context.clone()))?;
    AssertUnwindSafe(future)
        .catch_unwind()
        .await
        .map_err(errors::panic_error)?
}

fn same_executor(a: &Arc<dyn ToolExecutor>, b: &Arc<dyn ToolExecutor>) -> bool {
    std::ptr::eq(Arc::as_ptr(a).cast::<()>(), Arc::as_ptr(b).cast::<()>())
}

impl ToolsService {
    fn new(ctx: &Context, system_prompt: Service<SystemPromptKey>) -> Result<Self, BoxError> {
        // `emit('tools/change')` reaches every listener whatever dispatch filter `ctx` carries.
        let notifier = ctx.root();
        let layers = ScopedLayers::new(
            |scope: Option<&ScopeKey>| Ok(ToolLayer::new(scope.is_some())),
            move || notify_change(&notifier),
        )?;
        let guards = ScopedLayers::new(
            |_: Option<&ScopeKey>| {
                Ok(GuardLayer {
                    guards: AnonymousEntries::new(),
                })
            },
            || Ok(()),
        )?;
        Ok(Self(Arc::new(ToolsInner {
            system_prompt,
            layers,
            guards,
            executors: Mutex::new(ExecutorTable::default()),
        })))
    }

    /// Register globally or in the calling agent scope. Scoped tools shadow globals; duplicates within
    /// one layer and the reserved `run_code` name fail.
    pub fn register(
        &self,
        ctx: &Context,
        definition: ToolDefinition,
        options: ToolRegistrationOptions,
    ) -> Result<Disposer, BoxError> {
        let name = definition.name.clone();
        assert_supported_json_schema(&definition.output.schema)?;
        if let Some(timeout_ms) = definition.timeout_ms
            && (!timeout_ms.is_finite() || timeout_ms <= 0.0)
        {
            return Err(Box::new(ToolsError::TypeError(format!(
                "tool \"{name}\" timeoutMs must be a positive finite number"
            ))));
        }
        // Reserved unconditionally: any agent may select a code mode for itself, so a name free to take
        // under the deployment default would become a collision the moment a preset mounted.
        if name == RUN_CODE_NAME {
            return Err(Box::new(ToolsError::Error(format!(
                "tool name \"{RUN_CODE_NAME}\" is reserved for the PTC mode presentation transport and cannot be registered or shadowed"
            ))));
        }
        let registration = Arc::new(ToolRegistration {
            definition: Arc::new(definition),
            when: options.when,
        });
        self.0.layers.effect(
            ctx,
            |layer| layer.tools.insert(name, registration),
            EffectOptions::new("tools.register()"),
        )
    }

    /// Restrict global tools for the calling agent scope. Empty filters, unknown names, scope-local
    /// names, and reserved transport names fail. Restrictions intersect; scoped registrations remain visible.
    pub fn restrict(&self, ctx: &Context, filter: ToolRestriction) -> Result<Disposer, BoxError> {
        let Some(scope) = scope_of(ctx) else {
            return Err(Box::new(ToolsError::Error(
                "tools.restrict() requires a scoped context (agent.ctx): a context-global restriction would mask every agent — deny the tool for the intended agent instead".into(),
            )));
        };
        if filter.allow.is_none() && filter.deny.is_none() {
            return Err(Box::new(ToolsError::Error(
                "tools.restrict({}) is a no-op: pass `allow` and/or `deny` (an empty filter is almost always a materialized-empty-config bug)".into(),
            )));
        }
        let compiled = CompiledToolRestriction {
            allow: filter
                .allow
                .as_ref()
                .map(|names| names.iter().cloned().collect()),
            deny: filter
                .deny
                .as_ref()
                .map(|names| names.iter().cloned().collect()),
        };
        let named: Vec<&String> = filter
            .allow
            .iter()
            .flatten()
            .chain(filter.deny.iter().flatten())
            .collect();
        if named.iter().any(|name| name.as_str() == RUN_CODE_NAME) {
            return Err(Box::new(ToolsError::Error(format!(
                "tools.restrict() cannot name reserved PTC mode presentation transport \"{RUN_CODE_NAME}\"; restrict end-capability tools instead"
            ))));
        }
        let known = self.view(Some(&scope)).restrictable_names;
        let unknown: Vec<&&String> = named
            .iter()
            .filter(|name| !known.contains(name.as_str()))
            .collect();
        if !unknown.is_empty() {
            let mut sorted: Vec<&String> = known.iter().collect();
            sorted.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            let known_text = if sorted.is_empty() {
                "(none)".to_owned()
            } else {
                sorted
                    .iter()
                    .map(|name| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            return Err(Box::new(ToolsError::Error(format!(
                "tools.restrict() names unknown global tool{} {}; known global tools: {}",
                if unknown.len() > 1 { "s" } else { "" },
                unknown
                    .iter()
                    .map(|name| format!("\"{name}\""))
                    .collect::<Vec<_>>()
                    .join(", "),
                known_text
            ))));
        }
        let compiled = Arc::new(compiled);
        self.0.layers.effect(
            ctx,
            move |layer| Ok(layer.restrictions.append(compiled)),
            EffectOptions::new("tools.restrict()"),
        )
    }

    /// Resolve visible tools and pre-restriction names. Restrictions across the scope chain intersect
    /// over inherited global and ancestor tools; the viewing scope's own registrations remain exempt.
    /// Nearer registrations shadow farther ones.
    fn view(&self, scope: Option<&ScopeKey>) -> ToolView {
        let layers = self.0.layers.chain_layers(scope);
        let own = self.0.layers.peek(scope);
        let mut inherited: IndexMap<String, Arc<ToolRegistration>> =
            self.0.layers.global.tools.entries().collect();
        for layer in &layers {
            if own.as_ref().is_some_and(|own| Arc::ptr_eq(own, layer)) {
                continue;
            }
            for (name, registration) in layer.tools.entries() {
                inherited.insert(name, registration);
            }
        }
        let mut view = ToolView {
            visible: IndexMap::new(),
            registrations: IndexMap::new(),
            known_names: IndexSet::new(),
            restrictable_names: IndexSet::new(),
        };
        for (name, registration) in inherited {
            view.known_names.insert(name.clone());
            view.restrictable_names.insert(name.clone());
            if layers.iter().all(|layer| layer.admits(&name)) {
                view.visible
                    .insert(name.clone(), registration.definition.clone());
                view.registrations.insert(name, registration);
            }
        }
        if let Some(own) = own {
            for (name, registration) in own.tools.entries() {
                view.known_names.insert(name.clone());
                view.visible
                    .insert(name.clone(), registration.definition.clone());
                view.registrations.insert(name, registration);
            }
        }
        view
    }

    /// Look up a tool as one scope sees it (scoped shadows global; a restricted-away global reads as
    /// absent). Presenters pass the calling agent so the rendered card matches the definition that
    /// actually executed. `tool_set` is the step admission; omit only to inspect registered definitions.
    pub fn get(
        &self,
        name: &str,
        scope: Option<&ScopeKey>,
        tool_set: Option<&ResolvedToolSet>,
    ) -> Option<Arc<ToolDefinition>> {
        match tool_set {
            None => self.view(scope).visible.get(name).cloned(),
            Some(set) => set.view(scope).get(name).cloned(),
        }
    }

    /// Resolve the definition a call may execute. With a step's admitted set, the set decides; without
    /// one, conditional registrations are unavailable.
    pub fn executable(
        &self,
        name: &str,
        scope: Option<&ScopeKey>,
        tool_set: Option<&ResolvedToolSet>,
    ) -> Option<Arc<ToolDefinition>> {
        if let Some(set) = tool_set {
            return set.view(scope).get(name).cloned();
        }
        let view = self.view(scope);
        match view.registrations.get(name) {
            Some(registration) if registration.when.is_none() => view.visible.get(name).cloned(),
            _ => None,
        }
    }

    /// Resolve conditions once for this step without changing shared registrations. Cancellation or a
    /// rejected condition publishes no partial selection. Removed registrations cannot be restored by a
    /// late condition result.
    pub async fn resolve(
        &self,
        context: ToolActivationContext,
    ) -> Result<ResolvedToolSet, BoxError> {
        context.signal.throw_if_aborted()?;
        let agent_scope = context.agent.scope_key();
        let view = self.view(Some(&agent_scope));
        let mut admitted: IndexMap<String, Arc<ToolDefinition>> = IndexMap::new();
        for (name, definition) in &view.visible {
            let when = view
                .registrations
                .get(name)
                .and_then(|registration| registration.when.clone());
            let enabled = match when {
                None => true,
                Some(when) => match evaluate_condition(&when, &context).await {
                    Ok(enabled) => enabled,
                    Err(cause) => {
                        context.signal.throw_if_aborted()?;
                        return Err(Box::new(ToolActivationError::new(name, cause)));
                    }
                },
            };
            context.signal.throw_if_aborted()?;
            if enabled {
                admitted.insert(name.clone(), definition.clone());
            }
        }
        let tools = self.clone();
        let initial = view.registrations;
        Ok(ResolvedToolSet::new(move |scope| {
            let mut visible = IndexMap::new();
            if scope != Some(&agent_scope) {
                return visible;
            }
            let current = tools.view(Some(&agent_scope));
            for (name, definition) in &admitted {
                let same_definition = current
                    .visible
                    .get(name)
                    .is_some_and(|visible| Arc::ptr_eq(visible, definition));
                let same_registration = match (current.registrations.get(name), initial.get(name)) {
                    (Some(now), Some(then)) => Arc::ptr_eq(now, then),
                    (None, None) => true,
                    _ => false,
                };
                if same_definition && same_registration {
                    visible.insert(name.clone(), definition.clone());
                }
            }
            visible
        }))
    }

    /// Read the visible capability definitions for descriptions and program bindings: after scoped
    /// shadowing and restrictions, limited to the step's admission when given.
    pub fn definitions(
        &self,
        scope: Option<&ScopeKey>,
        tool_set: Option<&ResolvedToolSet>,
    ) -> Vec<Arc<ToolDefinition>> {
        match tool_set {
            None => self.view(scope).visible.values().cloned().collect(),
            Some(set) => set.view(scope).values().cloned().collect(),
        }
    }

    /// Project visible definitions onto the allowlisted model-facing schema fields, excluding execution
    /// and presentation callbacks. One deep-cloned schema per visible tool.
    pub fn schemas(
        &self,
        scope: Option<&ScopeKey>,
        tool_set: Option<&ResolvedToolSet>,
    ) -> Result<Vec<ToolSchema>, BoxError> {
        self.definitions(scope, tool_set)
            .iter()
            .map(|definition| Ok(project_tool_schema(definition, true)?))
            .collect()
    }

    /// The pre-restriction capability names of one scope's view.
    pub fn known_names(&self, scope: Option<&ScopeKey>) -> Vec<String> {
        self.view(scope).known_names.into_iter().collect()
    }

    /// The prompt-assembly provider: the selected executor describes the visible definitions.
    fn describe_for(&self, assemble: &AssembleContext) -> Result<ToolProviderResult, BoxError> {
        let scope = assemble.scope.as_ref();
        let executor = self.select_executor(scope)?;
        let definitions = self.definitions(scope, assemble.extension::<ResolvedToolSet>());
        let mut description = executor.describe(&definitions)?;
        if description.known_names.is_none() {
            description.known_names = Some(self.known_names(scope));
        }
        Ok(description)
    }

    /// Provide the executor for the calling scope; an unscoped contribution is the deployment default.
    /// Rendering any executor-owned section requires the viewing scope's selection to remain available.
    /// Unloading leaves the selection unavailable until it is replaced.
    pub fn set_executor(
        &self,
        ctx: &Context,
        executor: Arc<dyn ToolExecutor>,
    ) -> Result<Disposer, BoxError> {
        let scope = scope_of(ctx);
        let record = Arc::new(Registration {
            executor: Mutex::new(Some(executor.clone())),
        });
        // The source's check and insertion form one synchronous step: reserve the scope under one lock so
        // two registrations cannot both pass the check. `previous` is the unavailable selection a failed
        // setup must restore.
        let previous = {
            let mut table = lock(&self.0.executors);
            table.prune_dead();
            let existing = match &scope {
                None => table.root.clone(),
                Some(scope) => table.scoped.get(&scope.downgrade()).cloned(),
            };
            if existing
                .as_ref()
                .is_some_and(|record| lock(&record.executor).is_some())
            {
                return Err(Box::new(ToolsError::Error(
                    "Only one tool executor may be registered in each scope".into(),
                )));
            }
            match &scope {
                None => table.root = Some(record.clone()),
                Some(scope) => {
                    table.scoped.insert(scope.downgrade(), record.clone());
                }
            }
            existing
        };
        let service = self.clone();
        let disposer = ctx.try_effect("tools.setExecutor()", |undos| -> Result<(), BoxError> {
            for section in executor.sections() {
                let text = section.text.clone();
                let service = service.clone();
                let owner = executor.clone();
                let provider = PromptText::provider(move |assemble: &AssembleContext| {
                    let scope = assemble.scope.as_ref();
                    if !same_executor(&service.select_executor(scope)?, &owner) {
                        return Ok(String::new());
                    }
                    Ok(match &text {
                        ToolPromptText::Static(text) => text.clone(),
                        ToolPromptText::Provider(text) => text(
                            &service.definitions(scope, assemble.extension::<ResolvedToolSet>()),
                        ),
                    })
                });
                let disposer = self.0.system_prompt.section(
                    ctx,
                    PromptSection {
                        name: section.name.clone(),
                        order: section.order,
                        text: provider,
                        interpolate: section.interpolate,
                        complete: false,
                    },
                )?;
                undos.push(disposer)?;
            }
            let record = record.clone();
            undos.push(Undo::sync(move || {
                // Keep an unavailable selection so a program cannot fall through to a different executor.
                *lock(&record.executor) = None;
            }))?;
            Ok(())
        });
        if disposer.is_err() {
            // The setup failed after the reservation: give the scope back as it was.
            let mut table = lock(&self.0.executors);
            let held = match &scope {
                None => table.root.as_ref(),
                Some(scope) => table.scoped.get(&scope.downgrade()),
            };
            if held.is_some_and(|held| Arc::ptr_eq(held, &record)) {
                match (&scope, previous) {
                    (None, previous) => table.root = previous,
                    (Some(scope), Some(previous)) => {
                        table.scoped.insert(scope.downgrade(), previous);
                    }
                    (Some(scope), None) => {
                        table.scoped.remove(&scope.downgrade());
                    }
                }
            }
        }
        disposer
    }

    /// Resolve the nearest scoped provider, or the deployment default; fails when it is missing or unloaded.
    fn select_executor(&self, scope: Option<&ScopeKey>) -> Result<Arc<dyn ToolExecutor>, BoxError> {
        let record = {
            let mut table = lock(&self.0.executors);
            table.prune_dead();
            scope_chain_of(scope)
                .iter()
                .find_map(|key| table.scoped.get(&key.downgrade()).cloned())
                .or_else(|| table.root.clone())
        };
        let executor = record.and_then(|record| lock(&record.executor).clone());
        executor.ok_or_else(|| Box::new(ToolExecutorUnavailableError::new()) as BoxError)
    }

    /// Delegate a raw call batch to its Agent's provider or the deployment default. Select at dispatch;
    /// accepted batches settle through that provider even if it unloads.
    pub async fn execute(&self, input: ToolBatchInput) -> Result<ToolBatchResult, BoxError> {
        let scope = input.agent.as_ref().map(|agent| agent.scope_key());
        let executor = self.select_executor(scope.as_ref())?;
        executor.execute(input).await
    }

    /// Register a monotonic denial in the calling scope, evaluated after extensible pre-execute policy.
    pub fn guard(&self, ctx: &Context, guard: ToolGuard) -> Result<Disposer, BoxError> {
        self.0.guards.effect(
            ctx,
            move |layer| Ok(layer.guards.append(guard)),
            EffectOptions::new("tools.guard()"),
        )
    }

    /// Check global and inherited guards after pre-execute policy and approval resolve. Returns the
    /// first denial with its optional structured identity, or `None` when no guard denies.
    pub fn check_guards(&self, exec: &ToolExecution) -> Result<Option<ToolGuardDenial>, BoxError> {
        let scope = exec.agent.as_ref().map(|agent| agent.scope_key());
        let mut layers = vec![self.0.guards.global.clone()];
        layers.extend(self.0.guards.chain_layers(scope.as_ref()));
        for layer in layers {
            for guard in layer.guards.values() {
                if let Some(denial) = errors::catch_panic(|| guard(exec))?? {
                    return Ok(Some(denial));
                }
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod canonical_tests;
#[cfg(test)]
mod retention_tests;
