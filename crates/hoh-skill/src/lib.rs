//! Invocation-neutral, scope-layered skill provider registry.
pub mod error;
pub use error::{SKILL_ERROR_CODES, SkillError, SkillErrorCode};

use futures_util::future::BoxFuture;
use hoh_error::{BoxError, HarnessError, SharedError};
use hoh_kernel::{Cfg, Contained, Context, Disposer, Event, ListenerFailure, Plugin};
use hoh_scope::store::{EffectOptions, EntryUndo};
use hoh_scope::{NamedEntries, ScopeKey, ScopeLayer, ScopedLayers, scope_chain_of, scope_of};
use hoh_timeout::{AbortController, AbortSignal};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize, Serializer, ser::SerializeMap};
use serde_json::{Value, json};
use std::{
    any::Any,
    collections::HashSet,
    future::Future,
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex, MutexGuard, Weak},
};

pub const BUNDLED_SKILL_RANK: u64 = 600;
const DEFAULT_COLLECT_CACHE_ENTRIES: f64 = 128.0;
const RUNTIME_PROVIDER: &str = "runtime";
const RUNTIME_RANK: u64 = 250;
pub type SkillSource = String;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SkillResourceBase {
    Directory { path: String },
    Url { url: String },
    Opaque { description: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInvocationPolicy {
    pub model_invocable: bool,
    pub user_invocable: bool,
}

// Raw provider fields survive until the source validation boundary. Sharing
// individual fields preserves nested identities through spread and projection.
#[derive(Clone, Default)]
struct Fields {
    values: IndexMap<String, Arc<Value>>,
    locator: Option<Arc<dyn Any + Send + Sync>>,
    raw: Option<Value>,
}
impl From<Value> for Fields {
    fn from(value: Value) -> Self {
        let Value::Object(object) = value else {
            return Self {
                raw: Some(value),
                ..Default::default()
            };
        };
        let values: IndexMap<_, _> = object
            .into_iter()
            .map(|(key, value)| (key, Arc::new(value)))
            .collect();
        let locator = values
            .get("locator")
            .map(|value| value.clone() as Arc<dyn Any + Send + Sync>);
        Self {
            values,
            locator,
            raw: None,
        }
    }
}
impl Serialize for Fields {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let Some(raw) = &self.raw {
            return raw.serialize(serializer);
        }
        let mut map = serializer.serialize_map(Some(self.values.len()))?;
        for (key, value) in &self.values {
            map.serialize_entry(key, value.as_ref())?;
        }
        map.end()
    }
}
macro_rules! skill_object {
    ($name:ident) => {
        #[derive(Clone, Default)]
        pub struct $name(Arc<Fields>);
        impl $name {
            pub fn from_value(value: Value) -> Self {
                Self(Arc::new(value.into()))
            }
            pub fn get(&self, key: &str) -> Option<&Value> {
                self.0.values.get(key).map(Arc::as_ref)
            }
            pub fn shared_field(&self, key: &str) -> Option<Arc<Value>> {
                self.0.values.get(key).cloned()
            }
            pub fn text(&self, key: &str) -> Option<&str> {
                self.get(key).and_then(Value::as_str)
            }
            pub fn ptr_eq(&self, other: &Self) -> bool {
                Arc::ptr_eq(&self.0, &other.0)
            }
            pub fn to_value(&self) -> Value {
                if let Some(raw) = &self.0.raw {
                    return raw.clone();
                }
                Value::Object(
                    self.0
                        .values
                        .iter()
                        .map(|(k, v)| (k.clone(), v.as_ref().clone()))
                        .collect(),
                )
            }
            pub fn with_field(mut self, key: impl Into<String>, value: Value) -> Self {
                let fields = Arc::make_mut(&mut self.0);
                fields.raw = None;
                let key = key.into();
                let value = Arc::new(value);
                if key == "locator" {
                    fields.locator = Some(value.clone());
                }
                fields.values.insert(key, value);
                self
            }
            pub fn with_shared_field(mut self, key: impl Into<String>, value: Arc<Value>) -> Self {
                let fields = Arc::make_mut(&mut self.0);
                fields.raw = None;
                let key = key.into();
                if key == "locator" {
                    fields.locator = Some(value.clone());
                }
                fields.values.insert(key, value);
                self
            }
        }
        impl From<Value> for $name {
            fn from(value: Value) -> Self {
                Self::from_value(value)
            }
        }
        impl SkillMetadata for $name {
            fn field(&self, key: &str) -> Option<&Value> {
                self.get(key)
            }
        }
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.to_value().fmt(f)
            }
        }
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.0.serialize(serializer)
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                Value::deserialize(deserializer).map(Self::from_value)
            }
        }
    };
}
/// Shared metadata access for the source's Pick<SkillSummary, 'invocation'>.
pub trait SkillMetadata {
    fn field(&self, key: &str) -> Option<&Value>;
}
impl<T: SkillMetadata + ?Sized> SkillMetadata for &T {
    fn field(&self, key: &str) -> Option<&Value> {
        (*self).field(key)
    }
}
skill_object!(SkillSummary);
skill_object!(SkillCandidate);
skill_object!(SkillDefinition);
skill_object!(SkillRegistration);
impl From<&SkillCandidate> for SkillDefinition {
    fn from(candidate: &SkillCandidate) -> Self {
        Self(Arc::new(candidate.0.as_ref().clone()))
    }
}
impl SkillCandidate {
    pub fn with_locator<T: Any + Send + Sync>(mut self, locator: Arc<T>) -> Self {
        Arc::make_mut(&mut self.0).locator = Some(locator);
        self
    }
    pub fn locator<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.0.locator.clone()?.downcast().ok()
    }
}

#[derive(Clone, Debug, Default)]
pub struct SkillViewOptions {
    pub cwd: Option<String>,
    pub signal: Option<AbortSignal>,
    pub scope: Option<ScopeKey>,
}
pub type SkillLookupOptions = SkillViewOptions;
#[derive(Clone, Debug)]
pub struct SkillInvocationSource {
    pub name: String,
}
impl SkillInvocationSource {
    pub const KIND: &'static str = "skill-invocation";
    pub const FORM: &'static str = "instructions";
}
impl From<SkillInvocationSource> for hoh_llm::MessageSource {
    fn from(source: SkillInvocationSource) -> Self {
        let mut result = Self::new(SkillInvocationSource::KIND);
        result
            .data
            .insert("name".into(), Value::String(source.name));
        result.data.insert(
            "form".into(),
            Value::String(SkillInvocationSource::FORM.into()),
        );
        result
    }
}
impl Serialize for SkillInvocationSource {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        hoh_llm::MessageSource::from(self.clone()).serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for SkillInvocationSource {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let source = hoh_llm::MessageSource::deserialize(deserializer)?;
        if source.kind != Self::KIND
            || source.data.get("form").and_then(Value::as_str) != Some(Self::FORM)
        {
            return Err(serde::de::Error::custom("invalid skill invocation source"));
        }
        let name = source
            .data
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| serde::de::Error::custom("skill invocation name must be a string"))?;
        Ok(Self { name: name.into() })
    }
}
pub fn is_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}
pub fn is_model_invocable(skill: &impl SkillMetadata) -> Result<bool, BoxError> {
    invocation_flag(skill.field("invocation"), "modelInvocable")
}
pub fn is_user_invocable(skill: &impl SkillMetadata) -> Result<bool, BoxError> {
    invocation_flag(skill.field("invocation"), "userInvocable")
}
fn invocation_flag(policy: Option<&Value>, key: &str) -> Result<bool, BoxError> {
    policy
        .and_then(|p| p.get(key))
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            NativeError::Type(format!(
                "Cannot read properties of undefined (reading '{key}')"
            ))
            .into()
        })
}
pub fn escape_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
fn escape_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}
pub fn render_skill_content(skill: &SkillDefinition) -> Result<String, BoxError> {
    check_receiver(&skill.0, "resourceBase")?;
    let hint = match skill.get("resourceBase") {
        None => format!(
            "Resources for this skill are managed by provider \"{}\".\nLoad referenced resources only as needed.",
            escape_text(replace_text(skill.get("provider"))?)
        ),
        Some(value) => match resource_base(value)? {
            SkillResourceBase::Directory { path } => format!(
                "Base directory for this skill: {}\nResolve relative paths mentioned by this skill against the base directory before using them. Load referenced resources only as needed.",
                escape_text(&path)
            ),
            SkillResourceBase::Url { url } => format!(
                "Base URL for this skill: {}\nResolve relative URLs mentioned by this skill against the base URL before using them. Load referenced resources only as needed.",
                escape_text(&url)
            ),
            SkillResourceBase::Opaque { description } => format!(
                "Resources for this skill: {}\nLoad referenced resources only as needed.",
                escape_text(&description)
            ),
        },
    };
    let name = replace_text(skill.get("name"))?;
    // Array.join renders absent/null elements as empty strings.
    let content = skill
        .get("content")
        .filter(|v| !v.is_null())
        .map(hoh_util_values::js_string)
        .unwrap_or_default();
    Ok(format!(
        "<skill_content name=\"{}\">\n<skill_resources>\n{hint}\n</skill_resources>\n\n<skill_instructions>\n{content}\n</skill_instructions>\n</skill_content>",
        escape_attr(name)
    ))
}
fn replace_text(value: Option<&Value>) -> Result<&str, BoxError> {
    match value {
        Some(Value::String(text)) => Ok(text),
        None => Err(NativeError::Type(
            "Cannot read properties of undefined (reading 'replaceAll')".into(),
        )
        .into()),
        Some(Value::Null) => Err(NativeError::Type(
            "Cannot read properties of null (reading 'replaceAll')".into(),
        )
        .into()),
        Some(_) => Err(NativeError::Type("value.replaceAll is not a function".into()).into()),
    }
}
fn resource_base(value: &Value) -> Result<SkillResourceBase, BoxError> {
    if value.is_null() {
        return Err(
            NativeError::Type("Cannot read properties of null (reading 'kind')".into()).into(),
        );
    }
    match value.get("kind").and_then(Value::as_str) {
        Some("directory") => Ok(SkillResourceBase::Directory {
            path: replace_text(value.get("path"))?.into(),
        }),
        Some("url") => Ok(SkillResourceBase::Url {
            url: replace_text(value.get("url"))?.into(),
        }),
        Some("opaque") => Ok(SkillResourceBase::Opaque {
            description: replace_text(value.get("description"))?.into(),
        }),
        _ => Err(NativeError::Error(format!(
            "unreachable variant in SkillResourceBase.kind: {}",
            hoh_util_values::js_json_stringify(value)
        ))
        .into()),
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SkillCatalogSnapshot {
    pub skills: Vec<SkillSummary>,
    pub complete: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkillProviderObservation {
    pub candidates: Vec<SkillCandidate>,
    pub complete: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SkillProviderOutput {
    Candidates(Vec<SkillCandidate>),
    Observation(SkillProviderObservation),
    Raw(Value),
}
impl From<Vec<SkillCandidate>> for SkillProviderOutput {
    fn from(candidates: Vec<SkillCandidate>) -> Self {
        Self::Candidates(candidates)
    }
}
pub trait SkillProvider: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn list(
        &self,
        options: Arc<SkillLookupOptions>,
    ) -> BoxFuture<'static, Result<SkillProviderOutput, BoxError>>;
    fn get(
        &self,
        candidate: SkillCandidate,
        options: Arc<SkillLookupOptions>,
    ) -> BoxFuture<'static, Result<Option<SkillDefinition>, BoxError>>;
}
#[derive(Clone)]
pub struct SkillProviderControl {
    pub signal: AbortSignal,
    invalidate: Arc<dyn Fn() + Send + Sync>,
}
impl SkillProviderControl {
    pub fn invalidate(&self) {
        (self.invalidate)();
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(
        default = "default_cache_entries",
        serialize_with = "serialize_js_number"
    )]
    pub collect_cache_max_entries: f64,
}
fn serialize_js_number<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
    if value.is_finite() && value.fract() == 0.0 && *value >= 0.0 && *value <= u64::MAX as f64 {
        serializer.serialize_u64(*value as u64)
    } else {
        serializer.serialize_f64(*value)
    }
}
fn default_cache_entries() -> f64 {
    DEFAULT_COLLECT_CACHE_ENTRIES
}
impl Default for Config {
    fn default() -> Self {
        Self {
            collect_cache_max_entries: default_cache_entries(),
        }
    }
}
#[derive(Debug, thiserror::Error)]
pub enum NativeError {
    #[error("{0}")]
    Error(String),
    #[error("{0}")]
    Type(String),
}
impl NativeError {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Error(_) => "Error",
            Self::Type(_) => "TypeError",
        }
    }
}
/// An explicit bridge for JavaScript throws that are not Error instances.
#[derive(Debug, thiserror::Error)]
pub enum SkillThrownValue {
    #[error("{}", hoh_util_values::js_string(.0))]
    Json(Value),
    #[error("[unrenderable thrown value]")]
    Unrenderable,
}
#[derive(Debug, thiserror::Error)]
enum RuntimeDuplicate {
    #[error("duplicate runtime registration")]
    Duplicate,
}
#[derive(Clone, Debug)]
pub struct SkillsChange;
impl Event for SkillsChange {
    const NAME: &'static str = "skills/change";
    type Mode = Contained;
}

struct RegisteredProvider {
    provider: Arc<dyn SkillProvider>,
    order: u64,
}
struct LayerData {
    providers: NamedEntries<Arc<RegisteredProvider>>,
    runtime: Mutex<IndexMap<String, SkillDefinition>>,
}
#[derive(Clone)]
struct SkillLayer(Arc<LayerData>);
impl SkillLayer {
    fn new(scoped: bool) -> Self {
        Self(Arc::new(LayerData {
            providers: NamedEntries::new(move |name| {
                NativeError::Error(format!(
                    "a skill provider named \"{name}\" is already registered{}",
                    if scoped { " in this scope" } else { "" }
                ))
                .into()
            }),
            runtime: Mutex::default(),
        }))
    }
}
impl ScopeLayer for SkillLayer {
    fn is_empty(&self) -> bool {
        self.0.providers.is_empty() && lock(&self.0.runtime).is_empty()
    }
}
#[derive(Clone)]
struct IndexedCandidate {
    candidate: SkillCandidate,
    provider: Option<Arc<RegisteredProvider>>,
    runtime: Option<SkillDefinition>,
    local_order: usize,
    layer: SkillLayer,
}
impl IndexedCandidate {
    fn rank(&self) -> f64 {
        self.candidate
            .get("rank")
            .and_then(Value::as_f64)
            .unwrap_or(RUNTIME_RANK as f64)
    }
    fn provider_order(&self) -> i128 {
        self.provider.as_ref().map_or(-1, |p| i128::from(p.order))
    }
}
#[derive(Clone, Hash, PartialEq, Eq)]
struct CacheKey {
    cwd: Option<String>,
    chain: Vec<ScopeKey>,
    revision: u64,
}
#[derive(Clone)]
struct CollectResult {
    entries: IndexMap<String, IndexedCandidate>,
    cacheable: bool,
}
#[derive(Default)]
struct State {
    cache: IndexMap<CacheKey, IndexMap<String, IndexedCandidate>>,
    revision: u64,
    next_provider_order: u64,
}
struct RegistryState {
    ctx: Context,
    data: Mutex<State>,
    cap: f64,
}
impl RegistryState {
    fn invalidate(&self) {
        {
            let mut state = lock(&self.data);
            state.revision += 1;
            state.cache.clear();
        }
        let logger = self.ctx.logger();
        self.ctx
            .root()
            .dispatch(SkillsChange)
            .invoke(move |failure| {
                let (kind, error) = match failure {
                    ListenerFailure::Threw(error) => ("threw", error),
                    ListenerFailure::Rejected(error) => ("rejected", error),
                };
                logger.warn(format!(
                    "skills/change listener {kind}: {}",
                    error_message(error.as_ref())
                ));
            });
    }
}

pub struct SkillRegistry {
    ctx: Context,
    state: Arc<RegistryState>,
    layers: ScopedLayers<SkillLayer>,
}
hoh_kernel::service!(pub Skills: SkillRegistry = "skills");
impl Plugin for SkillRegistry {
    const NAME: &'static str = "SkillRegistry";
    type Config = Config;
    type Deps = ();
    async fn apply(ctx: Context, _deps: (), config: Cfg<Config>) -> Result<(), BoxError> {
        let registry = Arc::new(Self::new(ctx.clone(), config.get().as_ref().clone())?);
        ctx.provide::<Skills>(registry).await?;
        Ok(())
    }
}
impl SkillRegistry {
    pub fn new(ctx: Context, config: Config) -> Result<Self, BoxError> {
        let cap = config.collect_cache_max_entries;
        if !cap.is_finite() || cap.fract() != 0.0 || cap < 1.0 {
            return Err(NativeError::Error(
                "skill: collectCacheMaxEntries must be an integer greater than or equal to 1"
                    .into(),
            )
            .into());
        }
        let state = Arc::new(RegistryState {
            ctx: ctx.clone(),
            data: Mutex::default(),
            cap,
        });
        let observer = state.clone();
        let layers = ScopedLayers::new(
            |scope| Ok(SkillLayer::new(scope.is_some())),
            move || {
                observer.invalidate();
                Ok(())
            },
        )?;
        Ok(Self { ctx, state, layers })
    }
    pub fn register_provider(
        &self,
        ctx: &Context,
        create: impl FnOnce(SkillProviderControl) -> Result<Arc<dyn SkillProvider>, BoxError>,
    ) -> Result<Disposer, BoxError> {
        type Active = Option<(Weak<LayerData>, String, Weak<RegisteredProvider>)>;
        let active: Arc<Mutex<Active>> = Arc::new(Mutex::new(None));
        let lifecycle = Arc::new(AbortController::new());
        let marker = active.clone();
        let state = Arc::downgrade(&self.state);
        let control = SkillProviderControl {
            signal: lifecycle.signal(),
            invalidate: Arc::new(move || {
                let current = lock(&marker).clone();
                if let Some((layer, name, expected)) = current
                    && let (Some(layer), Some(expected), Some(state)) =
                        (layer.upgrade(), expected.upgrade(), state.upgrade())
                    && layer.providers.get(&name).is_some_and(|registered| {
                        Arc::ptr_eq(&registered.provider, &expected.provider)
                    })
                {
                    state.invalidate();
                }
            }),
        };
        let result = (|| {
            let provider = create(control)?;
            let name = provider.name().to_owned();
            if name == RUNTIME_PROVIDER {
                return Err(NativeError::Error(
                    "\"runtime\" is reserved for runtime skill registrations".into(),
                )
                .into());
            }
            let order = {
                let mut data = lock(&self.state.data);
                let order = data.next_provider_order;
                data.next_provider_order += 1;
                order
            };
            let provider = Arc::new(RegisteredProvider { provider, order });
            let lifecycle = lifecycle.clone();
            self.layers.effect(
                ctx,
                move |layer| {
                    let undo = layer.0.providers.insert(name.clone(), provider.clone())?;
                    *lock(&active) = Some((
                        Arc::downgrade(&layer.0),
                        name.clone(),
                        Arc::downgrade(&provider),
                    ));
                    Ok(EntryUndo::new(move || {
                        *lock(&active) = None;
                        undo.undo();
                        lifecycle.abort_with(NativeError::Error(format!(
                            "skill provider \"{name}\" disposed"
                        )));
                    }))
                },
                EffectOptions::new("skills.registerProvider()"),
            )
        })();
        result.map_err(|error| {
            let shared = SharedError::new(error);
            lifecycle.abort_with(shared.clone());
            Box::new(shared) as BoxError
        })
    }
    /// None represents the source's no-op disposer for an ignored duplicate.
    pub fn register(
        &self,
        ctx: &Context,
        skill: SkillRegistration,
    ) -> Result<Option<Disposer>, BoxError> {
        validate_runtime(&skill)?;
        let name = skill.text("name").unwrap_or_default().to_owned();
        let scope = scope_of(ctx);
        let existing = if scope.is_none() {
            Some(self.layers.global.clone())
        } else {
            self.layers.peek(scope.as_ref())
        };
        if existing.is_some_and(|layer| lock(&layer.0.runtime).contains_key(&name)) {
            ctx.logger().warn(format!(
                "runtime skill \"{name}\" ignored because it is already registered"
            ));
            return Ok(None);
        }
        let mut fields = skill.0.as_ref().clone();
        for (key, default) in [
            (
                "invocation",
                json!({"modelInvocable":true,"userInvocable":true}),
            ),
            ("provider", json!(RUNTIME_PROVIDER)),
        ] {
            if fields.values.get(key).is_none_or(|v| v.is_null()) {
                fields.values.insert(key.into(), Arc::new(default));
            }
        }
        let definition = SkillDefinition(Arc::new(fields));
        let duplicate_name = name.clone();
        let result = self
            .layers
            .effect(
                ctx,
                move |layer| {
                    let mut runtime = lock(&layer.0.runtime);
                    if runtime.contains_key(&name) {
                        return Err(RuntimeDuplicate::Duplicate.into());
                    }
                    runtime.insert(name.clone(), definition);
                    drop(runtime);
                    let layer = layer.clone();
                    Ok(EntryUndo::new(move || {
                        lock(&layer.0.runtime).shift_remove(&name);
                    }))
                },
                EffectOptions::new("skills.register()"),
            )
            .map(Some);
        match result {
            Err(error) if error.downcast_ref::<RuntimeDuplicate>().is_some() => {
                ctx.logger().warn(format!(
                    "runtime skill \"{duplicate_name}\" ignored because it is already registered"
                ));
                Ok(None)
            }
            result => result,
        }
    }
    pub async fn list(
        &self,
        options: Arc<SkillViewOptions>,
    ) -> Result<Vec<SkillSummary>, BoxError> {
        Ok(self.snapshot(options).await?.skills)
    }
    pub async fn snapshot(
        &self,
        options: Arc<SkillViewOptions>,
    ) -> Result<SkillCatalogSnapshot, BoxError> {
        let result = self.collect(options).await?;
        let mut skills: Vec<_> = result
            .entries
            .values()
            .map(|entry| summary(&entry.candidate))
            .collect();
        skills.sort_by(compare_skill_summary);
        Ok(SkillCatalogSnapshot {
            skills,
            complete: result.cacheable,
        })
    }
    pub async fn get(
        &self,
        name: &str,
        options: Arc<SkillViewOptions>,
    ) -> Result<Option<SkillDefinition>, BoxError> {
        if !is_skill_name(name) {
            return Ok(None);
        }
        let collected = self.collect(options.clone()).await?;
        check_abort(options.signal.as_ref())?;
        let Some(entry) = collected.entries.get(name) else {
            return Ok(None);
        };
        let definition = if let Some(runtime) = &entry.runtime {
            Some(runtime.clone())
        } else if let Some(provider) = &entry.provider {
            self.wait(
                provider
                    .provider
                    .get(entry.candidate.clone(), options.clone()),
                options.signal.as_ref(),
            )
            .await?
        } else {
            None
        };
        let Some(definition) = definition else {
            return Ok(None);
        };
        validate_definition(&definition)?;
        if definition.text("name") != entry.candidate.text("name") {
            if let Some(provider) = &entry.provider
                && entry
                    .layer
                    .0
                    .providers
                    .get(provider.provider.name())
                    .is_some_and(|live| Arc::ptr_eq(&live.provider, &provider.provider))
            {
                self.state.invalidate();
            }
            return Ok(None);
        }
        Ok(Some(definition))
    }
    async fn collect(&self, options: Arc<SkillViewOptions>) -> Result<CollectResult, BoxError> {
        check_abort(options.signal.as_ref())?;
        for attempt in 1..=2 {
            let chain = scope_chain_of(options.scope.as_ref());
            let (revision, key) = {
                let state = lock(&self.state.data);
                let key = CacheKey {
                    cwd: options.cwd.clone(),
                    chain,
                    revision: state.revision,
                };
                if let Some(entries) = state.cache.get(&key) {
                    return Ok(CollectResult {
                        entries: entries.clone(),
                        cacheable: true,
                    });
                }
                (state.revision, key)
            };
            let mut result = finish_collect(
                self.collect_fresh(options.clone()).await?,
                options.signal.as_ref(),
            )?;
            let mut state = lock(&self.state.data);
            if revision != state.revision {
                if attempt < 2 {
                    continue;
                }
                result.cacheable = false;
            } else if result.cacheable {
                state.cache.insert(key, result.entries.clone());
                if state.cache.len() as f64 > self.state.cap {
                    state.cache.shift_remove_index(0);
                }
            }
            return Ok(result);
        }
        // The inclusive loop returns on its second iteration.
        unreachable!("bounded collection returns on its final attempt")
    }
    async fn collect_fresh(
        &self,
        options: Arc<SkillViewOptions>,
    ) -> Result<CollectResult, BoxError> {
        let layers = std::iter::once(self.layers.global.clone())
            .chain(self.layers.chain_layers(options.scope.as_ref()))
            .collect::<Vec<_>>();
        let mut result = CollectResult {
            entries: IndexMap::new(),
            cacheable: true,
        };
        for layer in layers {
            let (mut entries, complete) = self.list_layer(&layer, options.clone()).await?;
            result.cacheable &= complete;
            entries.sort_by(|a, b| {
                a.rank()
                    .partial_cmp(&b.rank())
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.provider_order().cmp(&b.provider_order()))
                    .then(a.local_order.cmp(&b.local_order))
            });
            let mut seen = HashSet::new();
            for entry in entries {
                let name = entry.candidate.text("name").unwrap_or_default().to_owned();
                if !seen.insert(name.clone()) {
                    self.ctx.logger().warn(format!("skill \"{name}\" from {} ignored because a higher-priority skill already exists", entry.candidate.text("source").unwrap_or_default()));
                    continue;
                }
                result.entries.insert(name, entry);
            }
        }
        Ok(result)
    }
    async fn list_layer(
        &self,
        layer: &SkillLayer,
        options: Arc<SkillViewOptions>,
    ) -> Result<(Vec<IndexedCandidate>, bool), BoxError> {
        check_abort(options.signal.as_ref())?;
        let mut runtime: Vec<_> = lock(&layer.0.runtime).values().cloned().collect();
        runtime.sort_by(|a, b| a.text("name").cmp(&b.text("name")));
        let mut entries: Vec<_> = runtime
            .into_iter()
            .enumerate()
            .map(|(local_order, skill)| IndexedCandidate {
                candidate: runtime_candidate(&skill),
                runtime: Some(skill),
                provider: None,
                local_order,
                layer: layer.clone(),
            })
            .collect();
        let providers: Vec<_> = layer.0.providers.values().collect();
        let mut complete = true;
        for provider in providers {
            let output = match self
                .wait(
                    provider.provider.list(options.clone()),
                    options.signal.as_ref(),
                )
                .await
            {
                Ok(output) => output,
                Err(error) => {
                    check_abort(options.signal.as_ref())?;
                    complete = false;
                    self.ctx.logger().warn(format!(
                        "skill provider \"{}\" skipped: {}",
                        provider.provider.name(),
                        error_message(error.as_ref())
                    ));
                    continue;
                }
            };
            let observation = normalize(output, provider.provider.name())?;
            complete &= observation.complete;
            for (local_order, candidate) in observation.candidates.into_iter().enumerate() {
                validate_candidate(&candidate, provider.provider.name())?;
                entries.push(IndexedCandidate {
                    candidate,
                    provider: Some(provider.clone()),
                    runtime: None,
                    local_order,
                    layer: layer.clone(),
                });
            }
        }
        Ok((entries, complete))
    }
    async fn wait<T: Send + 'static>(
        &self,
        work: impl Future<Output = Result<T, BoxError>> + Send + 'static,
        signal: Option<&AbortSignal>,
    ) -> Result<T, BoxError> {
        let Some(signal) = signal else {
            return work.await;
        };
        // A Promise continues after its caller aborts. Keep its driver owned by
        // the registry fiber, so uncooperative work still ends at unload.
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let driver = self.ctx.spawn("skills.provider()", move |cancel| async move {
            tokio::select! { result = work => { let _ = sender.send(result); } _ = cancel.cancelled() => {} } Ok(())
        })?;
        check_abort(Some(signal))?;
        let result = tokio::select! {
            biased;
            reason = signal.cancelled() => Err(normalize_error(reason.into())),
            result = receiver => result.map_err(|_| Box::new(NativeError::Error("skill provider work disposed".into())) as BoxError)?.map_err(normalize_error),
        };
        if !signal.aborted() {
            driver.dispose().await;
        }
        result
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
fn check_abort(signal: Option<&AbortSignal>) -> Result<(), BoxError> {
    if let Some(signal) = signal {
        signal
            .throw_if_aborted()
            .map_err(|error| normalize_error(error.into()))?;
    }
    Ok(())
}
fn finish_collect(
    result: CollectResult,
    signal: Option<&AbortSignal>,
) -> Result<CollectResult, BoxError> {
    check_abort(signal)?;
    Ok(result)
}
fn error_message(error: &(dyn std::error::Error + 'static)) -> String {
    std::panic::catch_unwind(AssertUnwindSafe(|| {
        let error = SharedError::of(error).map_or(error, |shared| shared.get_ref());
        if let Some(value) = error.downcast_ref::<SkillThrownValue>() {
            return value.to_string();
        }
        let name = HarnessError::of(error)
            .map(|e| e.name())
            .unwrap_or_else(|| {
                error
                    .downcast_ref::<NativeError>()
                    .map_or("Error", NativeError::name)
            });
        let message = error.to_string();
        if message.is_empty() {
            name.into()
        } else {
            format!("{name}: {message}")
        }
    }))
    .unwrap_or_else(|_| "[unrenderable thrown value]".into())
}
fn normalize_error(error: BoxError) -> BoxError {
    let inner = SharedError::of(error.as_ref()).map_or(
        error.as_ref() as &(dyn std::error::Error + 'static),
        |shared| shared.get_ref(),
    );
    if inner.downcast_ref::<SkillThrownValue>().is_some() {
        Box::new(NativeError::Error(error_message(inner)))
    } else {
        error
    }
}
fn invalid(message: impl Into<String>) -> HarnessError {
    SkillError::new(message, SkillErrorCode::SkillInvalidDefinition)
}
fn normalize(
    output: SkillProviderOutput,
    name: &str,
) -> Result<SkillProviderObservation, HarnessError> {
    match output {
        SkillProviderOutput::Candidates(candidates) => Ok(SkillProviderObservation {
            candidates,
            complete: true,
        }),
        SkillProviderOutput::Observation(observation) => Ok(observation),
        SkillProviderOutput::Raw(value) => {
            if let Some(values) = value.as_array() {
                return Ok(SkillProviderObservation {
                    candidates: values.iter().cloned().map(Into::into).collect(),
                    complete: true,
                });
            }
            if let (Some(values), Some(complete)) = (
                value.get("candidates").and_then(Value::as_array),
                value.get("complete").and_then(Value::as_bool),
            ) {
                return Ok(SkillProviderObservation {
                    candidates: values.iter().cloned().map(Into::into).collect(),
                    complete,
                });
            }
            Err(invalid(format!(
                "skill provider \"{name}\" list() must return an array or {{ candidates, complete }} observation"
            )))
        }
    }
}
fn validate_invocation(policy: Option<&Value>, subject: &str) -> Result<(), HarnessError> {
    let Some(policy) = policy else {
        return Ok(());
    };
    if !policy.is_object() {
        return Err(invalid(format!(
            "{subject} with a non-object invocation policy"
        )));
    }
    for key in ["modelInvocable", "userInvocable"] {
        if !policy.get(key).is_some_and(Value::is_boolean) {
            return Err(invalid(format!(
                "{subject} with a non-boolean invocation.{key}"
            )));
        }
    }
    Ok(())
}
fn check_receiver(fields: &Fields, key: &str) -> Result<(), BoxError> {
    if fields.raw.as_ref().is_some_and(Value::is_null) {
        return Err(
            NativeError::Type(format!("Cannot read properties of null (reading '{key}')")).into(),
        );
    }
    Ok(())
}
fn validate_candidate(skill: &SkillCandidate, provider: &str) -> Result<(), BoxError> {
    check_receiver(&skill.0, "name")?;
    validate_candidate_fields(skill, provider).map_err(Into::into)
}
fn validate_candidate_fields(skill: &SkillCandidate, provider: &str) -> Result<(), HarnessError> {
    let Some(name) = skill.text("name") else {
        return Err(invalid(format!(
            "skill provider \"{provider}\" returned a non-string skill name"
        )));
    };
    if !is_skill_name(name) {
        return Err(invalid(format!(
            "skill provider \"{provider}\" returned invalid skill name \"{name}\""
        )));
    }
    let subject = format!("skill provider \"{provider}\" returned skill \"{name}\"");
    let Some(description) = skill.text("description") else {
        return Err(invalid(format!("{subject} with a non-string description")));
    };
    if description.is_empty() {
        return Err(invalid(format!("{subject} without a description")));
    }
    validate_invocation(skill.get("invocation"), &subject)?;
    for key in ["whenToUse", "source", "rank", "provider", "path"] {
        let value = skill.get(key);
        if matches!(key, "whenToUse" | "path") && value.is_none() {
            continue;
        }
        if key == "rank" {
            if !value.and_then(Value::as_f64).is_some_and(f64::is_finite) {
                return Err(invalid(format!("{subject} with an invalid rank")));
            }
        } else if !value.is_some_and(Value::is_string) {
            return Err(invalid(format!("{subject} with a non-string {key}")));
        }
        if key == "provider" && skill.text(key) != Some(provider) {
            return Err(invalid(format!(
                "{subject} for provider \"{}\"",
                skill.text(key).unwrap_or_default()
            )));
        }
    }
    Ok(())
}
fn validate_runtime(skill: &SkillRegistration) -> Result<(), BoxError> {
    check_receiver(&skill.0, "name")?;
    let name = skill.text("name");
    if !name.is_some_and(is_skill_name) {
        let name = skill
            .get("name")
            .map_or_else(|| "undefined".into(), hoh_util_values::js_string);
        return Err(invalid(format!("invalid skill name \"{name}\"")).into());
    }
    let name = name.unwrap_or_default();
    let Some(description) = skill.get("description") else {
        return Err(NativeError::Type(
            "Cannot read properties of undefined (reading 'length')".into(),
        )
        .into());
    };
    if description.is_null() {
        return Err(
            NativeError::Type("Cannot read properties of null (reading 'length')".into()).into(),
        );
    }
    if description.as_str().is_some_and(str::is_empty)
        || description.as_array().is_some_and(Vec::is_empty)
    {
        return Err(invalid(format!("skill \"{name}\" requires a description")).into());
    }
    validate_invocation(
        skill.get("invocation"),
        &format!("runtime skill \"{name}\""),
    )?;
    Ok(())
}
fn validate_definition(skill: &SkillDefinition) -> Result<(), BoxError> {
    check_receiver(&skill.0, "name")?;
    validate_definition_fields(skill).map_err(Into::into)
}
fn validate_definition_fields(skill: &SkillDefinition) -> Result<(), HarnessError> {
    let Some(name) = skill.text("name") else {
        return Err(invalid("loaded skill name must be a string"));
    };
    if !is_skill_name(name) {
        return Err(invalid(format!("loaded skill has invalid name \"{name}\"")));
    }
    let subject = format!("loaded skill \"{name}\"");
    let Some(description) = skill.text("description") else {
        return Err(invalid(format!("{subject} description must be a string")));
    };
    if description.is_empty() {
        return Err(invalid(format!("{subject} requires a description")));
    }
    validate_invocation(skill.get("invocation"), &subject)?;
    for key in ["whenToUse", "source", "provider", "content", "path"] {
        let value = skill.get(key);
        if matches!(key, "whenToUse" | "path") && value.is_none() {
            continue;
        }
        if !value.is_some_and(Value::is_string) {
            return Err(invalid(format!("{subject} {key} must be a string")));
        }
    }
    Ok(())
}
fn project(fields: &Fields, keys: &[&str]) -> Fields {
    Fields {
        values: keys
            .iter()
            .filter_map(|key| fields.values.get(*key).map(|v| ((*key).into(), v.clone())))
            .collect(),
        locator: None,
        raw: None,
    }
}
fn summary(skill: &SkillCandidate) -> SkillSummary {
    SkillSummary(Arc::new(project(
        &skill.0,
        &[
            "name",
            "path",
            "description",
            "whenToUse",
            "invocation",
            "source",
            "provider",
            "resourceBase",
        ],
    )))
}
fn compare_skill_summary(left: &SkillSummary, right: &SkillSummary) -> std::cmp::Ordering {
    left.text("name").cmp(&right.text("name"))
}

fn runtime_candidate(skill: &SkillDefinition) -> SkillCandidate {
    let mut fields = project(
        &skill.0,
        &[
            "name",
            "description",
            "whenToUse",
            "invocation",
            "source",
            "provider",
            "resourceBase",
        ],
    );
    fields
        .values
        .insert("rank".into(), Arc::new(json!(RUNTIME_RANK)));
    fields
        .values
        .insert("locator".into(), Arc::new(skill.to_value()));
    fields.locator = Some(Arc::new(skill.clone()));
    for key in ["path", "metadata"] {
        if let Some(value) = skill.shared_field(key) {
            fields.values.insert(key.into(), value);
        }
    }
    SkillCandidate(Arc::new(fields))
}

#[cfg(test)]
mod boundary_tests {
    use super::*;

    #[test]
    fn summary_comparator_returns_equal_for_the_same_name() {
        let left = SkillSummary::from_value(json!({"name":"same-name"}));
        let right = SkillSummary::from_value(json!({"name":"same-name"}));

        assert_eq!(
            compare_skill_summary(&left, &right),
            std::cmp::Ordering::Equal
        );
    }
}
