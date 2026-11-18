//! Port of @deepseek-ai/dsh-system-prompt (packages/core/system-prompt).
//! Registry for ordered system sections, dynamic context, tool schemas, and prompt variables.

use std::{
    any::{Any, TypeId},
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fmt,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex, MutexGuard},
};

use futures_util::{FutureExt, future::BoxFuture};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Disposer, Plugin, Realm, ServiceKey, Waterfall};
use hoh_llm::{ContextSnapshotSection, ToolSchema};
use hoh_scope::{
    AnonymousEntries, NamedEntries, ScopeFilter, ScopeKey, ScopeLayer, Scoped, ScopedLayers,
    scope_target, store::EffectOptions,
};
use hoh_timeout::AbortSignal;
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Reserved `Config::tool_order` marker for unlisted tools.
pub const TOOL_ORDER_REST: &str = "<unlisted-tools>";

/// The deployment persona prefix's section name. A composition can replace this slot.
pub const PERSONA_PREFIX_SECTION: &str = "deployment:persona-prefix";

/// Deployment persona suffix section name shared by global and scoped contributions.
pub const PERSONA_SUFFIX_SECTION: &str = "deployment:persona-suffix";

/// How a variable name is written between the braces, as JavaScript prints the pattern.
const VARIABLE_NAME_SOURCE: &str = "/^[a-z][a-z0-9_]*$/";

/// Failures of the registry itself. The source throws plain `Error` and `TypeError` values.
#[derive(Debug, thiserror::Error)]
pub enum SystemPromptError {
    /// A name is taken in the global layer.
    #[error(
        "prompt {kind} \"{name}\" is already registered (for a per-agent {hint}, register through that agent's `agent.ctx` instead)"
    )]
    Duplicate {
        kind: &'static str,
        hint: &'static str,
        name: String,
    },
    /// A name is taken in a scoped layer.
    #[error("prompt {kind} \"{name}\" is already registered in this scope")]
    DuplicateScoped { kind: &'static str, name: String },
    /// `TypeError` for a section or context order that is not a finite number.
    #[error("prompt {kind} \"{name}\" order must be a finite number")]
    OrderNotFinite { kind: &'static str, name: String },
    #[error("invalid prompt variable name \"{name}\" (must match {VARIABLE_NAME_SOURCE})")]
    InvalidVariableName { name: String },
    #[error("toolOrder lists \"{name}\" more than once")]
    ToolOrderDuplicate { name: String },
    /// `TypeError: toolOrder is not iterable`: the source's `for...of` over an explicit `null`.
    #[error("toolOrder is not iterable")]
    ToolOrderNotIterable,
    #[error(
        "toolOrder must contain the \"{TOOL_ORDER_REST}\" rest entry (where unlisted tools are inserted)"
    )]
    ToolOrderMissingRest,
    #[error(
        "tool provider returned reserved tool name \"{TOOL_ORDER_REST}\" (reserved for toolOrder's rest entry)"
    )]
    ReservedToolName,
    #[error("toolOrder lists unregistered tool{plural} {listed}; known tools: {known}")]
    UnregisteredTools {
        plural: &'static str,
        listed: String,
        known: String,
    },
    #[error("multiple complete prompt sections are active: {names}")]
    MultipleComplete { names: String },
    #[error(
        "malformed prompt variable reference at \"{excerpt}…\" in {kind} \"{owner}\" (references are complete simple {{{{name}}}} groups)"
    )]
    MalformedReferenceAt {
        excerpt: String,
        kind: &'static str,
        owner: String,
    },
    #[error(
        "malformed prompt variable reference \"{{{{{name}}}}}\" in {kind} \"{owner}\" (variable names match {VARIABLE_NAME_SOURCE})"
    )]
    MalformedReference {
        name: String,
        kind: &'static str,
        owner: String,
    },
    #[error(
        "unknown prompt variable \"{{{{{name}}}}}\" in {kind} \"{owner}\"; registered variables: {known}"
    )]
    UnknownVariable {
        name: String,
        kind: &'static str,
        owner: String,
        known: String,
    },
    #[error(
        "prompt variable \"{{{{{name}}}}}\" has no value for this assembly ({kind} \"{owner}\")"
    )]
    UndefinedVariable {
        name: String,
        kind: &'static str,
        owner: String,
    },
}

/// Names of centrally allocated prompt-section positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PromptSectionOrderName {
    HarnessIdentity,
    DeploymentPersonaPrefix,
    PlanPolicy,
    TeamPolicy,
    PtcOnly,
    FileReference,
    ToolBash,
    ToolPwsh,
    ToolRead,
    ToolWrite,
    ToolEdit,
    ToolApplyPatch,
    ToolGlob,
    ToolGrep,
    ToolJobs,
    ToolPty,
    ToolPtyPanel,
    ToolWebSearch,
    ToolWebFetch,
    ToolLsp,
    ToolSessionQuery,
    ToolGoal,
    ToolSchedule,
    ToolWorkflow,
    ToolRalph,
    ToolSubagent,
    ToolReport,
    ToolComputerUse,
    McpServers,
    ToolsSdk,
    DeliverableFileReferences,
    StructuredOutput,
    HarnessSource,
    WebSurface,
    DeploymentPersonaSuffix,
}

impl PromptSectionOrderName {
    fn order(self) -> f64 {
        use PromptSectionOrderName::*;
        match self {
            HarnessIdentity => -1000.0,
            DeploymentPersonaPrefix => 0.0,
            PlanPolicy => 500.0,
            TeamPolicy => 600.0,
            PtcOnly => 800.0,
            FileReference => 900.0,
            ToolBash => 1000.0,
            ToolPwsh => 1010.0,
            ToolRead => 1100.0,
            ToolWrite => 1200.0,
            ToolEdit => 1300.0,
            ToolApplyPatch => 1350.0,
            ToolGlob => 1400.0,
            ToolGrep => 1500.0,
            ToolJobs => 1600.0,
            ToolPty => 1700.0,
            ToolPtyPanel => 1710.0,
            ToolWebSearch => 2000.0,
            ToolWebFetch => 2100.0,
            ToolLsp => 2200.0,
            ToolSessionQuery => 2300.0,
            ToolGoal => 2400.0,
            ToolSchedule => 2500.0,
            ToolWorkflow => 2600.0,
            ToolRalph => 2700.0,
            ToolSubagent => 2800.0,
            ToolReport => 2900.0,
            ToolComputerUse => 3000.0,
            McpServers => 3100.0,
            ToolsSdk => 5000.0,
            DeliverableFileReferences => 9000.0,
            StructuredOutput => 9900.0,
            // Local paths and endpoints follow reusable instructions.
            HarnessSource => 10000.0,
            WebSurface => 10100.0,
            DeploymentPersonaSuffix => 10200.0,
        }
    }
}

/// Names of centrally allocated runtime-context positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PromptContextOrderName {
    WorkingDirectory,
    SandboxPolicy,
    ApprovalPolicy,
    SubagentDelegation,
    HarnessSource,
    WebSurface,
}

impl PromptContextOrderName {
    fn order(self) -> f64 {
        match self {
            Self::WorkingDirectory => 100.0,
            Self::SandboxPolicy => 110.0,
            Self::ApprovalPolicy => 115.0,
            Self::SubagentDelegation => 120.0,
            // Process-specific paths and endpoints stay out of the cached system prompt.
            Self::HarnessSource => 200.0,
            Self::WebSurface => 210.0,
        }
    }
}

/// Plugin extensions of [`AssembleContext`]: the source merges fields into the interface
/// from other packages (`agent`, `toolSet`, `modelSelection`); here each package stores a
/// value of its own type.
#[derive(Clone, Default)]
struct Extensions(HashMap<TypeId, Arc<dyn Any + Send + Sync>>);

/// Context for one prompt assembly.
#[derive(Clone, Default)]
pub struct AssembleContext {
    /// Scope whose providers and waterfall listeners participate. When absent,
    /// only global providers and subject-less listeners participate.
    pub scope: Option<ScopeKey>,
    /// Explicit control signal for the turn that requested this assembly, when any.
    pub signal: Option<AbortSignal>,
    extensions: Extensions,
}

impl AssembleContext {
    pub fn new(scope: Option<ScopeKey>) -> Self {
        Self {
            scope,
            ..Self::default()
        }
    }
    /// Attach a field a plugin merges into the context; a second value of the same type replaces the first.
    pub fn with_extension<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.extensions.0.insert(TypeId::of::<T>(), Arc::new(value));
        self
    }
    pub fn extension<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.extensions
            .0
            .get(&TypeId::of::<T>())
            .and_then(|value| value.downcast_ref::<T>())
    }
}

impl fmt::Debug for AssembleContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssembleContext")
            .field("scope", &self.scope)
            .field("signal", &self.signal.is_some())
            .field("extensions", &self.extensions.0.len())
            .finish()
    }
}

type TextProvider = dyn Fn(&AssembleContext) -> Result<String, BoxError> + Send + Sync;

/// Static text or a provider evaluated at each assembly.
#[derive(Clone)]
pub enum PromptText {
    Static(String),
    Provider(Arc<TextProvider>),
}

impl PromptText {
    pub fn provider(
        provider: impl Fn(&AssembleContext) -> Result<String, BoxError> + Send + Sync + 'static,
    ) -> Self {
        Self::Provider(Arc::new(provider))
    }
    fn resolve(&self, context: &AssembleContext) -> Result<String, BoxError> {
        match self {
            Self::Static(text) => Ok(text.clone()),
            Self::Provider(provider) => provider(context),
        }
    }
}

impl Default for PromptText {
    fn default() -> Self {
        Self::Static(String::new())
    }
}
impl From<&str> for PromptText {
    fn from(text: &str) -> Self {
        Self::Static(text.to_owned())
    }
}
impl From<String> for PromptText {
    fn from(text: String) -> Self {
        Self::Static(text)
    }
}

/// One contributed section of the system prompt (registry input).
#[derive(Clone, Default)]
pub struct PromptSection {
    /// Unique name — a duplicate registration fails.
    pub name: String,
    /// Sections are concatenated in ascending order. Equal orders use code-unit name order.
    pub order: f64,
    pub text: PromptText,
    /// Whether to interpolate prompt variables. Absent means true; `Some(false)` preserves literal text.
    pub interpolate: Option<bool>,
    /// Treat this contribution as the complete system prompt.
    pub complete: bool,
}

/// Dynamic model context materialized as a durable user-role snapshot.
#[derive(Clone, Default)]
pub struct PromptContext {
    /// Unique name — a duplicate registration fails.
    pub name: String,
    /// Contexts are joined in ascending order.
    pub order: f64,
    pub text: PromptText,
    /// Whether to interpolate prompt variables. Absent means true; `Some(false)` preserves literal text.
    pub interpolate: Option<bool>,
    /// Keep this operational context when optional runtime context is disabled.
    pub required: bool,
}

/// One section of an assembly: [`PromptSection`] with its text resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssembledSection {
    pub name: String,
    /// The resolved (but not yet interpolated) section text.
    pub text: String,
    pub interpolate: Option<bool>,
}

/// One resolved dynamic context contribution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssembledContext {
    pub name: String,
    /// The resolved text before variable interpolation.
    pub text: String,
    pub interpolate: Option<bool>,
}

/// Tool schemas visible in one assembly and their pre-restriction name set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolProviderResult {
    /// The schemas this provider contributes to THIS assembly.
    pub schemas: Vec<ToolSchema>,
    /// The pre-restriction name universe for config validation (defaults to `schemas`' names).
    pub known_names: Option<Vec<String>>,
}

/// Prompt variables of one assembly: a JavaScript record's key order. Array-index keys
/// (`"2"`, `"10"`) enumerate first in ascending order, the rest by insertion; a `None`
/// value records "registered but no value here".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Variables(IndexMap<String, Option<String>>);

impl Variables {
    pub fn new() -> Self {
        Self::default()
    }
    /// Set a variable; an existing name keeps its position, as a record property does.
    pub fn insert(
        &mut self,
        name: impl Into<String>,
        value: Option<String>,
    ) -> Option<Option<String>> {
        self.0.insert(name.into(), value)
    }
    /// `Object.hasOwn`-style lookup: only registered names are found.
    pub fn get(&self, name: &str) -> Option<&Option<String>> {
        self.0.get(name)
    }
    pub fn contains_key(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }
    /// `delete record[name]`: a later insertion of the name goes to the end.
    pub fn remove(&mut self, name: &str) -> Option<Option<String>> {
        self.0.shift_remove(name)
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// `Object.keys` order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.ordered_keys().into_iter()
    }
    /// `Object.entries` order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Option<String>)> {
        self.ordered_keys().into_iter().map(|key| {
            let value = self.0.get(key).unwrap_or(&None);
            (key, value)
        })
    }
    /// Array-index keys ascending first, then the rest by insertion.
    fn ordered_keys(&self) -> Vec<&str> {
        fn index(key: &str) -> Option<u32> {
            let number: u32 = key.parse().ok()?;
            (number != u32::MAX && number.to_string() == key).then_some(number)
        }
        let mut indexed: Vec<(u32, &str)> = self
            .0
            .keys()
            .filter_map(|key| index(key).map(|number| (number, key.as_str())))
            .collect();
        indexed.sort_by_key(|(number, _)| *number);
        indexed
            .into_iter()
            .map(|(_, key)| key)
            .chain(
                self.0
                    .keys()
                    .map(String::as_str)
                    .filter(|key| index(key).is_none()),
            )
            .collect()
    }
}

impl PartialEq<IndexMap<String, Option<String>>> for Variables {
    fn eq(&self, other: &IndexMap<String, Option<String>>) -> bool {
        &self.0 == other
    }
}
impl From<IndexMap<String, Option<String>>> for Variables {
    fn from(variables: IndexMap<String, Option<String>>) -> Self {
        Self(variables)
    }
}
impl FromIterator<(String, Option<String>)> for Variables {
    fn from_iter<I: IntoIterator<Item = (String, Option<String>)>>(entries: I) -> Self {
        Self(entries.into_iter().collect())
    }
}

/// The content of a prompt presentation: what the assembly waterfall may transform.
/// Sections and contexts remain uninterpolated until rendered; tool declarations are
/// owned by their providers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PresentationData {
    pub sections: Vec<AssembledSection>,
    pub contexts: Vec<AssembledContext>,
    pub variables: Variables,
}

/// The presentation object of one assembly. Cloning shares the object: every clone sees
/// and changes the same content, as the source's presentation object passes through its
/// listener chain and tail by reference.
#[derive(Clone, Default)]
pub struct PromptPresentation(Arc<Mutex<PresentationData>>);

impl PromptPresentation {
    pub fn new(data: PresentationData) -> Self {
        Self(Arc::new(Mutex::new(data)))
    }
    /// Mutate the shared content.
    pub fn update<R>(&self, mutate: impl FnOnce(&mut PresentationData) -> R) -> R {
        mutate(&mut lock(&self.0))
    }
    /// A copy of the content as it is now.
    pub fn snapshot(&self) -> PresentationData {
        lock(&self.0).clone()
    }
    /// Whether both handles are the same object.
    pub fn same_object(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl fmt::Debug for PromptPresentation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PromptPresentation")
            .field(&*lock(&self.0))
            .finish()
    }
}

/// Completed model inputs: the presentation plus tool declarations in canonical order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PromptAssembly {
    pub sections: Vec<AssembledSection>,
    pub contexts: Vec<AssembledContext>,
    pub variables: Variables,
    pub tools: Vec<ToolSchema>,
}

impl PromptAssembly {
    fn from_presentation(presentation: PresentationData, tools: Vec<ToolSchema>) -> Self {
        Self {
            sections: presentation.sections,
            contexts: presentation.contexts,
            variables: presentation.variables,
            tools,
        }
    }
}

/// Event `system-prompt/assemble`: presentation waterfall over assembled sections,
/// contexts, and variables. Scope-filtered dispatch: scoped listeners receive only
/// that scope's assemblies. The presentation is one object shared by the whole chain;
/// a listener mutates it through [`update`](Self::update) (or through the handle that
/// `next.run()` returns, which is the same object) and delegates with `next.run()`.
pub struct SystemPromptAssemble {
    assembly: PromptPresentation,
    /// The caller's per-assembly context.
    pub context: Arc<AssembleContext>,
}

impl SystemPromptAssemble {
    /// The assembly object the chain shapes (shared, not a copy).
    pub fn assembly(&self) -> PromptPresentation {
        self.assembly.clone()
    }
    /// Mutate the assembly the rest of the chain will see.
    pub fn update<R>(&self, mutate: impl FnOnce(&mut PresentationData) -> R) -> R {
        self.assembly.update(mutate)
    }
}
hoh_kernel::event!(
    SystemPromptAssemble = "system-prompt/assemble",
    Waterfall<PromptPresentation>
);

/// Event `system-prompt/change`: emitted when any prompt provider changes. Unfiltered,
/// because a global change affects every scope.
pub struct SystemPromptChange;
hoh_kernel::event!(
    SystemPromptChange = "system-prompt/change",
    hoh_kernel::Emit
);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether `name` matches `^[a-z][a-z0-9_]*$`.
fn is_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Code-unit name comparison — locale-independent, so the order is identical on every machine.
fn compare_names(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// `a.order - b.order || compareNames(a.name, b.name)`.
fn compare_orders(a: f64, b: f64) -> Ordering {
    let difference = a - b;
    if difference > 0.0 {
        Ordering::Greater
    } else if difference < 0.0 {
        Ordering::Less
    } else {
        Ordering::Equal
    }
}

fn comparable_section(a: &PromptSection, b: &PromptSection) -> Ordering {
    compare_orders(a.order, b.order).then_with(|| compare_names(&a.name, &b.name))
}

fn compare_tool_names(a: &ToolSchema, b: &ToolSchema) -> Ordering {
    compare_names(&a.name, &b.name)
}

/// Validate duplicate names and the required [`TOOL_ORDER_REST`] marker.
/// Registered names are checked later because plugins have not loaded yet.
fn validate_tool_order(
    tool_order: Option<&Option<Vec<String>>>,
) -> Result<Option<Vec<String>>, SystemPromptError> {
    let Some(tool_order) = tool_order else {
        return Ok(None);
    };
    // An explicit `null` survives the source's schema; `for...of null` then throws.
    let Some(tool_order) = tool_order else {
        return Err(SystemPromptError::ToolOrderNotIterable);
    };
    let mut seen = HashSet::new();
    for name in tool_order {
        if !seen.insert(name.as_str()) {
            return Err(SystemPromptError::ToolOrderDuplicate { name: name.clone() });
        }
    }
    if !seen.contains(TOOL_ORDER_REST) {
        return Err(SystemPromptError::ToolOrderMissingRest);
    }
    Ok(Some(tool_order.clone()))
}

/// Apply configured tool order, inserting unlisted tools lexicographically at
/// [`TOOL_ORDER_REST`]. Unknown configured names fail; known but restricted
/// names may be absent.
fn order_tools(
    mut tools: Vec<ToolSchema>,
    tool_order: Option<&Vec<String>>,
    known_names: &HashSet<String>,
) -> Result<Vec<ToolSchema>, SystemPromptError> {
    if tools.iter().any(|tool| tool.name == TOOL_ORDER_REST) {
        return Err(SystemPromptError::ReservedToolName);
    }
    let Some(tool_order) = tool_order else {
        tools.sort_by(compare_tool_names);
        return Ok(tools);
    };
    let unknown: Vec<&String> = tool_order
        .iter()
        .filter(|name| name.as_str() != TOOL_ORDER_REST && !known_names.contains(name.as_str()))
        .collect();
    if !unknown.is_empty() {
        let mut known: Vec<&str> = known_names.iter().map(String::as_str).collect();
        known.sort_by(|a, b| compare_names(a, b));
        let known = if known.is_empty() {
            "(none)".to_owned()
        } else {
            known.join(", ")
        };
        return Err(SystemPromptError::UnregisteredTools {
            plural: if unknown.len() > 1 { "s" } else { "" },
            listed: unknown
                .iter()
                .map(|name| format!("\"{name}\""))
                .collect::<Vec<_>>()
                .join(", "),
            known,
        });
    }
    let listed: HashSet<&str> = tool_order.iter().map(String::as_str).collect();
    let mut rest: Vec<ToolSchema> = tools
        .iter()
        .filter(|tool| !listed.contains(tool.name.as_str()))
        .cloned()
        .collect();
    rest.sort_by(compare_tool_names);
    let mut ordered = Vec::new();
    for name in tool_order {
        if name == TOOL_ORDER_REST {
            ordered.extend(rest.iter().cloned());
        } else {
            ordered.extend(tools.iter().filter(|tool| &tool.name == name).cloned());
        }
    }
    Ok(ordered)
}

/// Plugin config: the deployment-authored fragment of the system prompt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// Include the fixed DeepSeek Harness identity before the deployment persona (default true).
    pub include_harness_identity: bool,
    /// Include optional runtime-context snapshots in model history (default true); required context remains.
    pub include_runtime_context: bool,
    /// Deployment-wide persona prefix template before first-party guidance. A scoped section named
    /// `deployment:persona-prefix` shadows it; `{{variable}}` references are strict.
    pub persona_prefix: String,
    /// Persona suffix template after first-party guidance. A scoped `deployment:persona-suffix`
    /// section shadows it; `{{variable}}` references are strict. Defaults to empty.
    pub persona_suffix: String,
    /// Model-facing tool names in order, with [`TOOL_ORDER_REST`] exactly once.
    /// Omitted (`None`) means lexicographic order. An explicit `null` (`Some(None)`) is kept
    /// as the source keeps it and makes loading fail: the source cannot iterate it.
    #[serde(
        default,
        deserialize_with = "explicit_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<Vec<String>>")]
    pub tool_order: Option<Option<Vec<String>>>,
}

/// Distinguish an absent key (`None`) from an explicit `null` (`Some(None)`).
fn explicit_null<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<Vec<String>>>, D::Error> {
    Option::<Vec<String>>::deserialize(deserializer).map(Some)
}

impl Default for Config {
    fn default() -> Self {
        Self {
            include_harness_identity: true,
            include_runtime_context: true,
            persona_prefix: String::new(),
            persona_suffix: String::new(),
            tool_order: None,
        }
    }
}

/// Interpolate strict `{{variable}}` references, drop empty sections, and join
/// the rest with blank lines. Sections with `interpolate: Some(false)` retain literal
/// text. Malformed, unknown, or undefined references in other sections fail;
/// a lone `{{` without any later `}}` is literal prose, and substituted values
/// are not scanned again.
pub fn render_prompt(assembly: &PromptAssembly) -> Result<String, SystemPromptError> {
    let mut texts = Vec::new();
    for section in &assembly.sections {
        let text = if section.interpolate == Some(false) {
            section.text.clone()
        } else {
            interpolate(&section.name, &section.text, &assembly.variables, "section")?
        };
        if !text.is_empty() {
            texts.push(text);
        }
    }
    Ok(texts.join("\n\n"))
}

/// Render the complete dynamic context snapshot, preserving literal contributions.
/// Returns `""` when no context is active.
pub fn render_context_snapshot(assembly: &PromptAssembly) -> Result<String, SystemPromptError> {
    Ok(join_context_sections(&render_context_sections(assembly)?))
}

/// The model-facing snapshot text for an already-rendered section list.
pub fn join_context_sections(sections: &[ContextSnapshotSection]) -> String {
    let body = sections
        .iter()
        .map(|section| section.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    if body.is_empty() {
        return String::new();
    }
    format!(
        "Current runtime context. This snapshot supersedes earlier runtime-context snapshots.\n\n{body}"
    )
}

/// The same snapshot, kept as the named contributions it was assembled from: one entry
/// per contributing context that rendered to non-empty text.
pub fn render_context_sections(
    assembly: &PromptAssembly,
) -> Result<Vec<ContextSnapshotSection>, SystemPromptError> {
    let mut sections = Vec::new();
    for context in &assembly.contexts {
        let text = if context.interpolate == Some(false) {
            context.text.clone()
        } else {
            interpolate(&context.name, &context.text, &assembly.variables, "context")?
        };
        if !text.is_empty() {
            sections.push(ContextSnapshotSection {
                name: context.name.clone(),
                text,
            });
        }
    }
    Ok(sections)
}

/// Interpolate one section or context and attribute diagnostics to its owning input.
fn interpolate(
    owner: &str,
    text: &str,
    variables: &Variables,
    kind: &'static str,
) -> Result<String, SystemPromptError> {
    let mut result = String::new();
    let mut last = 0;
    while let Some(found) = text[last..].find("{{") {
        let open = last + found;
        // A complete `{{...}}` group at the scan position: no braces inside the name.
        let rest = &text[open + 2..];
        let group = rest
            .find(['{', '}'])
            .filter(|&end| rest[end..].starts_with("}}"))
            .map(|end| &rest[..end]);
        let Some(name) = group else {
            // A later closing brace makes this malformed; otherwise it is literal prose.
            if text[open + 2..].contains("}}") {
                let units: Vec<u16> = text[open..].encode_utf16().take(16).collect();
                return Err(SystemPromptError::MalformedReferenceAt {
                    excerpt: String::from_utf16_lossy(&units),
                    kind,
                    owner: owner.to_owned(),
                });
            }
            result.push_str(&text[last..open + 2]);
            last = open + 2;
            continue;
        };
        // `{{}}` yields an empty name and follows the malformed-reference path.
        if !is_variable_name(name) {
            return Err(SystemPromptError::MalformedReference {
                name: name.to_owned(),
                kind,
                owner: owner.to_owned(),
            });
        }
        // Names are looked up among the assembly's own variables only.
        let Some(value) = variables.get(name) else {
            let known: Vec<&str> = variables.keys().collect();
            return Err(SystemPromptError::UnknownVariable {
                name: name.to_owned(),
                kind,
                owner: owner.to_owned(),
                known: if known.is_empty() {
                    "(none)".to_owned()
                } else {
                    known.join(", ")
                },
            });
        };
        let Some(value) = value else {
            return Err(SystemPromptError::UndefinedVariable {
                name: name.to_owned(),
                kind,
                owner: owner.to_owned(),
            });
        };
        result.push_str(&text[last..open]);
        result.push_str(value);
        last = open + name.len() + 4;
    }
    result.push_str(&text[last..]);
    Ok(result)
}

/// Resolve one registered runtime fact without assembling sections or tools.
fn resolve_context(
    entry: &PromptContext,
    context: &AssembleContext,
) -> Result<AssembledContext, BoxError> {
    Ok(AssembledContext {
        name: entry.name.clone(),
        text: entry.text.resolve(context)?,
        interpolate: entry.interpolate,
    })
}

/// One tool-schema provider stored in a prompt layer.
type ToolProvider =
    Arc<dyn Fn(&AssembleContext) -> Result<ToolProviderResult, BoxError> + Send + Sync>;

/// One prompt-variable provider stored in a prompt layer.
type VariableProvider =
    Arc<dyn Fn(&AssembleContext) -> Result<Option<String>, BoxError> + Send + Sync>;

/// All prompt registrations owned by one global or scoped layer.
struct PromptLayer {
    sections: NamedEntries<Arc<PromptSection>>,
    contexts: NamedEntries<Arc<PromptContext>>,
    runtime_context_suppressors: AnonymousEntries<bool>,
    tool_providers: AnonymousEntries<ToolProvider>,
    variables: NamedEntries<VariableProvider>,
}

impl PromptLayer {
    /// Create one prompt layer with diagnostics specific to its ownership scope.
    fn new(scoped: bool) -> Self {
        let duplicate = move |kind: &'static str, hint: &'static str| {
            move |name: &str| -> BoxError {
                if scoped {
                    Box::new(SystemPromptError::DuplicateScoped {
                        kind,
                        name: name.to_owned(),
                    })
                } else {
                    Box::new(SystemPromptError::Duplicate {
                        kind,
                        hint,
                        name: name.to_owned(),
                    })
                }
            }
        };
        Self {
            sections: NamedEntries::new(duplicate("section", "override")),
            contexts: NamedEntries::new(duplicate("context", "override")),
            runtime_context_suppressors: AnonymousEntries::new(),
            tool_providers: AnonymousEntries::new(),
            variables: NamedEntries::new(duplicate("variable", "value")),
        }
    }
}

impl ScopeLayer for PromptLayer {
    fn is_empty(&self) -> bool {
        self.sections.is_empty()
            && self.contexts.is_empty()
            && self.runtime_context_suppressors.is_empty()
            && self.tool_providers.is_empty()
            && self.variables.is_empty()
    }
}

/// The service's dispatch filter (`Service[Context.filter]`): a listener is admitted when
/// it sees the same `systemPrompt` realm as the caller the dispatch is made for.
struct RealmFilter(Realm);

impl ScopeFilter for RealmFilter {
    fn matches_filter(&self, listener: &Context) -> bool {
        listener.realm_of(<SystemPromptKey as ServiceKey>::NAME) == self.0
    }
}

/// Registry service for the prompt inputs assembled before each model step.
///
/// Every registering method takes the caller's context first: the registration is an
/// effect of that context, so it is removed when the caller is unloaded, and its scope
/// (if any) decides the layer it lands in.
pub struct SystemPrompt {
    ctx: Context,
    layers: ScopedLayers<PromptLayer>,
    tool_order: Option<Vec<String>>,
}

hoh_kernel::service!(pub SystemPromptKey: SystemPrompt = "systemPrompt");

impl Plugin for SystemPrompt {
    const NAME: &'static str = "SystemPrompt";
    type Config = Config;
    type Deps = ();

    async fn apply(ctx: Context, _deps: (), config: Cfg<Config>) -> Result<(), BoxError> {
        let service = Arc::new(Self::new(ctx.clone(), &config.get())?);
        ctx.provide::<SystemPromptKey>(service).await?;
        Ok(())
    }
}

/// An error value for a panic payload, keeping the text a listener panicked with.
fn panic_error(payload: Box<dyn Any + Send>) -> BoxError {
    match payload.downcast::<String>() {
        Ok(text) => (*text).into(),
        Err(payload) => match payload.downcast::<&'static str>() {
            Ok(text) => (*text).into(),
            Err(_) => "event listener panicked".into(),
        },
    }
}

impl SystemPrompt {
    /// Construct the service value and register the built-in sections on `ctx`.
    /// `Plugin::apply` publishes it as the `systemPrompt` service.
    pub fn new(ctx: Context, config: &Config) -> Result<Self, BoxError> {
        // The source emits with the event name first and no receiver: no receiver filter, so
        // every listener is notified whatever dispatch filter `ctx` carries.
        let notifier = ctx.root();
        let layers = ScopedLayers::new(
            |scope: Option<&ScopeKey>| Ok(PromptLayer::new(scope.is_some())),
            // A listener that throws makes the notification — and so the registration — fail.
            move || {
                catch_unwind(AssertUnwindSafe(|| notifier.emit(&SystemPromptChange)))
                    .map_err(panic_error)
            },
        )?;
        let service = Self {
            ctx,
            layers,
            tool_order: validate_tool_order(config.tool_order.as_ref())?,
        };
        let ctx = service.ctx.clone();
        // Keep harness-owned openers independent of the selected loop plugin.
        if config.include_harness_identity {
            service.section(
                &ctx,
                PromptSection {
                    name: "harness:identity".into(),
                    order: Self::get_section_order(PromptSectionOrderName::HarnessIdentity),
                    text: "You are an AI agent powered by DeepSeek Harness.".into(),
                    ..PromptSection::default()
                },
            )?;
        }
        service.section(
            &ctx,
            PromptSection {
                name: PERSONA_PREFIX_SECTION.into(),
                order: Self::get_section_order(PromptSectionOrderName::DeploymentPersonaPrefix),
                text: config.persona_prefix.clone().into(),
                ..PromptSection::default()
            },
        )?;
        service.section(
            &ctx,
            PromptSection {
                name: PERSONA_SUFFIX_SECTION.into(),
                order: Self::get_section_order(PromptSectionOrderName::DeploymentPersonaSuffix),
                text: config.persona_suffix.clone().into(),
                ..PromptSection::default()
            },
        )?;
        if !config.include_runtime_context {
            service.suppress_runtime_context(&ctx)?;
        }
        Ok(service)
    }

    /// Register an ordered prompt section in the calling context's scope. A scoped
    /// section shadows a global section with the same name; duplicates within one
    /// layer and non-finite orders fail. Registration and disposal emit
    /// `system-prompt/change`. Returns the exact effect disposer.
    pub fn section(&self, ctx: &Context, section: PromptSection) -> Result<Disposer, BoxError> {
        if !section.order.is_finite() {
            return Err(SystemPromptError::OrderNotFinite {
                kind: "section",
                name: section.name,
            }
            .into());
        }
        let name = section.name.clone();
        let section = Arc::new(section);
        self.layers.effect(
            ctx,
            |layer| layer.sections.insert(name, section),
            EffectOptions::new("systemPrompt.section()"),
        )
    }

    /// Resolve the centrally owned placement of a repository prompt section.
    pub fn get_section_order(name: PromptSectionOrderName) -> f64 {
        name.order()
    }

    /// Resolve the centrally owned placement of a repository runtime context.
    pub fn get_context_order(name: PromptContextOrderName) -> f64 {
        name.order()
    }

    /// Register ordered dynamic context in the calling context's scope. Scoped
    /// entries shadow global entries with the same name.
    pub fn context(&self, ctx: &Context, context: PromptContext) -> Result<Disposer, BoxError> {
        if !context.order.is_finite() {
            return Err(SystemPromptError::OrderNotFinite {
                kind: "context",
                name: context.name,
            }
            .into());
        }
        let name = context.name.clone();
        let context = Arc::new(context);
        self.layers.effect(
            ctx,
            |layer| layer.contexts.insert(name, context),
            EffectOptions::new("systemPrompt.context()"),
        )
    }

    /// Suppress optional dynamic runtime-context contributions in the calling
    /// context's scope without changing the services that own or enforce those
    /// facts. Multiple suppressors remain independently disposable.
    pub fn suppress_runtime_context(&self, ctx: &Context) -> Result<Disposer, BoxError> {
        self.layers.effect(
            ctx,
            |layer| Ok(layer.runtime_context_suppressors.append(true)),
            EffectOptions::new("systemPrompt.suppressRuntimeContext()"),
        )
    }

    /// Register a tool-schema provider in the calling context's scope. Global and
    /// matching scoped providers both contribute; returning the reserved
    /// [`TOOL_ORDER_REST`] name makes assembly fail.
    pub fn tools(
        &self,
        ctx: &Context,
        provider: impl Fn(&AssembleContext) -> Result<ToolProviderResult, BoxError>
        + Send
        + Sync
        + 'static,
    ) -> Result<Disposer, BoxError> {
        let provider: ToolProvider = Arc::new(provider);
        self.layers.effect(
            ctx,
            |layer| Ok(layer.tool_providers.append(provider)),
            EffectOptions::new("systemPrompt.tools()"),
        )
    }

    /// Register a prompt variable in the calling context's scope. Scoped values
    /// shadow globals; invalid or duplicate names fail. A provider may return
    /// `None`, but rendering a section that references that value then fails.
    pub fn variable(
        &self,
        ctx: &Context,
        name: &str,
        provider: impl Fn(&AssembleContext) -> Result<Option<String>, BoxError> + Send + Sync + 'static,
    ) -> Result<Disposer, BoxError> {
        if !is_variable_name(name) {
            return Err(SystemPromptError::InvalidVariableName {
                name: name.to_owned(),
            }
            .into());
        }
        let provider: VariableProvider = Arc::new(provider);
        self.layers.effect(
            ctx,
            |layer| layer.variables.insert(name, provider),
            EffectOptions::new("systemPrompt.variable()"),
        )
    }

    /// Refresh accepted registered runtime facts for request admission. Contexts
    /// added only by the assembly waterfall retain their accepted values. Current
    /// suppression removes optional contexts, and missing required registrations
    /// are restored in registry order. Sections, tools, and interpolation variables
    /// retain the accepted assembly; their providers and waterfall do not rerun.
    pub fn refresh_context(
        &self,
        assembly: &PromptAssembly,
        context: &AssembleContext,
    ) -> Result<PromptAssembly, BoxError> {
        let providers = self
            .layers
            .merge(context.scope.as_ref(), |layer| &layer.contexts);
        let suppressed = !self.layers.global.runtime_context_suppressors.is_empty()
            || self
                .layers
                .chain_layers(context.scope.as_ref())
                .iter()
                .any(|layer| !layer.runtime_context_suppressors.is_empty());
        let mut required: IndexMap<&String, &Arc<PromptContext>> = providers
            .iter()
            .filter(|(_, entry)| entry.required)
            .collect();
        let mut contexts: Vec<AssembledContext> = Vec::new();
        for accepted in &assembly.contexts {
            let provider = providers.get(&accepted.name);
            required.shift_remove(&accepted.name);
            if suppressed && !provider.is_some_and(|provider| provider.required) {
                continue;
            }
            contexts.push(match provider {
                None => accepted.clone(),
                Some(provider) => resolve_context(provider, context)?,
            });
        }
        for provider in required.values() {
            let index = contexts
                .iter()
                .position(|entry| {
                    providers
                        .get(&entry.name)
                        .is_some_and(|next| next.order > provider.order)
                })
                .unwrap_or(contexts.len());
            contexts.insert(index, resolve_context(provider, context)?);
        }
        Ok(PromptAssembly {
            contexts,
            ..assembly.clone()
        })
    }

    /// Assemble global and scoped providers, apply canonical ordering, then run the
    /// presentation waterfall. Scoped sections and variables shadow globals. The
    /// waterfall's text is authoritative except that an effective complete section is
    /// restored afterwards as the sole prompt section; tool declarations stay
    /// provider-owned.
    ///
    /// Like the source's `async` method, providers run when this is called, and every
    /// configuration failure arrives through the returned future.
    pub fn assemble(
        &self,
        context: impl Into<Arc<AssembleContext>>,
    ) -> BoxFuture<'static, Result<PromptAssembly, BoxError>> {
        self.assemble_from(&self.ctx, context)
    }

    /// [`assemble`](Self::assemble) for a caller other than the service's own context. The
    /// source dispatches `system-prompt/assemble` from the caller's view of the service
    /// (`this` is the caller-bound proxy), so only listeners that see the same `systemPrompt`
    /// realm as `caller` take part. `assemble` is this call with the service's own context.
    pub fn assemble_from(
        &self,
        caller: &Context,
        context: impl Into<Arc<AssembleContext>>,
    ) -> BoxFuture<'static, Result<PromptAssembly, BoxError>> {
        match self.begin_assembly(caller, context.into()) {
            Ok(assembly) => assembly.finish().boxed(),
            Err(error) => std::future::ready(Err(error)).boxed(),
        }
    }

    fn begin_assembly(
        &self,
        caller: &Context,
        context: Arc<AssembleContext>,
    ) -> Result<Assembling, BoxError> {
        let scope = context.scope.clone();
        let scope_layers = self.layers.chain_layers(scope.as_ref());
        let runtime_context_suppressed = !self.layers.global.runtime_context_suppressors.is_empty()
            || scope_layers
                .iter()
                .any(|layer| !layer.runtime_context_suppressors.is_empty());
        // Scoped variables shadow globals; iteration stays live, as a JavaScript Map's does.
        let mut variables = Variables::new();
        for (name, provider) in self.layers.global.variables.entries() {
            variables.insert(name, provider(&context)?);
        }
        // Scope-chain variables, farthest first, so the nearest scope wins a name.
        for layer in &scope_layers {
            for (name, provider) in layer.variables.entries() {
                variables.insert(name, provider(&context)?);
            }
        }
        // Scoped sections shadow globals before the deterministic order sort.
        let section_by_name = self.layers.merge(scope.as_ref(), |layer| &layer.sections);
        let context_by_name = self.layers.merge(scope.as_ref(), |layer| &layer.contexts);
        // Validate order against pre-restriction names while collecting visible schemas.
        let mut providers: Vec<ToolProvider> = self.layers.global.tool_providers.values().collect();
        for layer in &scope_layers {
            providers.extend(layer.tool_providers.values());
        }
        let mut collected: Vec<ToolSchema> = Vec::new();
        let mut known_names: HashSet<String> = HashSet::new();
        for provider in providers {
            let result = provider(&context)?;
            let schemas: Vec<ToolSchema> = result
                .schemas
                .iter()
                .map(|tool| ToolSchema {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    parameters: tool.parameters.clone(),
                    defer_loading: (tool.defer_loading == Some(true)).then_some(true),
                })
                .collect();
            let accepted = result
                .known_names
                .unwrap_or_else(|| schemas.iter().map(|tool| tool.name.clone()).collect());
            collected.extend(schemas);
            known_names.extend(accepted);
        }
        let mut definitions: Vec<&Arc<PromptSection>> = section_by_name.values().collect();
        definitions.sort_by(|a, b| comparable_section(a, b));
        let complete: Vec<&&Arc<PromptSection>> = definitions
            .iter()
            .filter(|section| section.complete)
            .collect();
        if complete.len() > 1 {
            return Err(SystemPromptError::MultipleComplete {
                names: complete
                    .iter()
                    .map(|section| {
                        hoh_util_values::js_json_stringify(&serde_json::Value::String(
                            section.name.clone(),
                        ))
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            }
            .into());
        }
        let mut complete_section: Option<AssembledSection> = None;
        let mut sections = Vec::with_capacity(definitions.len());
        for section in &definitions {
            let assembled = AssembledSection {
                name: section.name.clone(),
                text: section.text.resolve(&context)?,
                interpolate: section.interpolate,
            };
            if section.complete {
                complete_section = Some(assembled.clone());
            }
            sections.push(assembled);
        }
        let tools = order_tools(collected, self.tool_order.as_ref(), &known_names)?;
        let mut entries: Vec<&Arc<PromptContext>> = context_by_name
            .values()
            .filter(|entry| !runtime_context_suppressed || entry.required)
            .collect();
        entries.sort_by(|a, b| compare_orders(a.order, b.order));
        let mut contexts = Vec::with_capacity(entries.len());
        for entry in entries {
            contexts.push(resolve_context(entry, &context)?);
        }
        let event = Arc::new(SystemPromptAssemble {
            assembly: PromptPresentation::new(PresentationData {
                sections,
                contexts,
                variables,
            }),
            context,
        });
        let filter = Arc::new(RealmFilter(
            caller.realm_of(<SystemPromptKey as ServiceKey>::NAME),
        ));
        let carrier: Scoped<RealmFilter> = scope_target(filter, scope);
        let waterfall = carrier
            .context(&self.ctx)
            .waterfall::<SystemPromptAssemble, PromptPresentation, _>(event, |event| async move {
                Ok(event.assembly())
            });
        Ok(Assembling {
            waterfall: Box::pin(waterfall),
            tools,
            complete_section,
            runtime_context_suppressed,
            required_contexts: context_by_name
                .values()
                .filter(|entry| entry.required)
                .map(|entry| entry.name.clone())
                .collect(),
        })
    }
}

/// An assembly whose providers have run and whose waterfall has been started.
struct Assembling {
    waterfall: BoxFuture<'static, Result<PromptPresentation, BoxError>>,
    tools: Vec<ToolSchema>,
    complete_section: Option<AssembledSection>,
    runtime_context_suppressed: bool,
    required_contexts: HashSet<String>,
}

impl Assembling {
    async fn finish(self) -> Result<PromptAssembly, BoxError> {
        let transformed = self.waterfall.await?.snapshot();
        if self.complete_section.is_none() && !self.runtime_context_suppressed {
            return Ok(PromptAssembly::from_presentation(transformed, self.tools));
        }
        let sections = match self.complete_section {
            None => transformed.sections,
            Some(section) => vec![section],
        };
        let contexts = if self.runtime_context_suppressed {
            transformed
                .contexts
                .into_iter()
                .filter(|entry| self.required_contexts.contains(&entry.name))
                .collect()
        } else {
            transformed.contexts
        };
        Ok(PromptAssembly {
            sections,
            contexts,
            variables: transformed.variables,
            tools: self.tools,
        })
    }
}
