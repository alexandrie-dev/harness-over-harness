//! Tool definitions and invocation data shared by registries and executors.

use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use hoh_error::{BoxError, HarnessError};
use hoh_llm::{ContentBlock, ToolCallId, ToolSchema, UserMessage};
use hoh_scope::ScopeKey;
use hoh_timeout::AbortSignal;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error_codes::TOOL_CONTEXT_UNAVAILABLE;
use crate::presentation::{ToolCallView, ToolResultView};
use crate::shim::{Agent, BoxFuture, ModelSelection};

/// Reserved model-facing program transport name.
pub const RUN_CODE_NAME: &str = "run_code";

/// Canonical error code for cancellation after a tool body was invoked.
pub const TOOL_ABORTED: &str = "ABORTED";

/// Canonical error code for cancellation before a tool body was invoked.
pub const TOOL_ABORTED_BEFORE_DISPATCH: &str = "ABORTED_BEFORE_DISPATCH";

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Pure projection from validated arguments and value to Native/model content.
pub type RenderFn =
    Arc<dyn Fn(&Value, &Value) -> Result<Vec<ContentBlock>, BoxError> + Send + Sync>;
/// Pure replayable presentation projection for native and nested calls.
pub type PresentationMetaFn = Arc<dyn Fn(&Value, &Value) -> Result<Value, BoxError> + Send + Sync>;
/// Runs one accepted call and returns only its canonical lossless-JSON value.
pub type ExecuteFn = Arc<
    dyn Fn(Value, Arc<ToolExecution>) -> BoxFuture<'static, Result<Value, BoxError>> + Send + Sync,
>;
/// Content projection over a normalized outcome; `None` preserves the content.
pub type ContentHook =
    Arc<dyn Fn(&ToolExecution, &ToolExecutionResult) -> Option<Vec<ContentBlock>> + Send + Sync>;
/// Pure synchronous classifier for overlap with sibling tool calls.
pub type ConcurrencyFn = Arc<dyn Fn(&Value) -> bool + Send + Sync>;
/// Pure pending-state presenter.
pub type PresentCallFn = Arc<dyn Fn(&Value) -> Option<ToolCallView> + Send + Sync>;
/// Pure completed-state presenter.
pub type PresentResultFn = Arc<dyn Fn(&Value, &ToolResult) -> Option<ToolResultView> + Send + Sync>;

/// Tool-owned canonical output contract used after the body returns a JSON value.
#[derive(Clone)]
pub struct ToolOutputDefinition {
    /// Raw supported JSON Schema enforced against every successful canonical value.
    pub schema: Value,
    pub render: RenderFn,
    pub presentation_meta: Option<PresentationMetaFn>,
}

/// A registered tool: its schema plus the execution function.
#[derive(Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Map<String, Value>,
    /// Requests deferred loading of the tool definition; see `ToolSchema::defer_loading`.
    pub defer_loading: Option<bool>,
    /// Mandatory canonical output declaration.
    pub output: ToolOutputDefinition,
    pub execute: ExecuteFn,
    /// Install execution-prepared content before `tools/post-execute` policies.
    pub project_content: Option<ContentHook>,
    /// Synchronous last-mile transform for model-facing content, invoked once per normalized outcome.
    pub finalize_content: Option<ContentHook>,
    /// Cooperative tool-call timeout budget in milliseconds; never sent to the model.
    pub timeout_ms: Option<f64>,
    /// Only `true` opts in to overlap with sibling calls; never model-visible.
    pub is_concurrency_safe: Option<ConcurrencyFn>,
    pub present_call: Option<PresentCallFn>,
    pub present_result: Option<PresentResultFn>,
}

impl ToolDefinition {
    /// A definition with the mandatory members; every optional member is absent.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Map<String, Value>,
        output: ToolOutputDefinition,
        execute: ExecuteFn,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            defer_loading: None,
            output,
            execute,
            project_content: None,
            finalize_content: None,
            timeout_ms: None,
            is_concurrency_safe: None,
            present_call: None,
            present_result: None,
        }
    }
}

/// Inputs for a tool's step-time activation condition.
#[derive(Clone)]
pub struct ToolActivationContext {
    /// Agent preparing the step.
    pub agent: Arc<dyn Agent>,
    /// Provider, model, and optional reasoning effort captured for the step.
    pub model_selection: ModelSelection,
    /// Cancellation of step preparation; asynchronous conditions must observe it.
    pub signal: AbortSignal,
}

/// Decide whether the step's requests may call the tool.
pub type WhenFn =
    Arc<dyn Fn(ToolActivationContext) -> BoxFuture<'static, Result<bool, BoxError>> + Send + Sync>;

/// Lifecycle-owned conditions for a registered tool.
#[derive(Clone, Default)]
pub struct ToolRegistrationOptions {
    /// Omission enables the tool. False excludes it from declarations and dispatch; an error fails step preparation.
    pub when: Option<WhenFn>,
}

impl ToolRegistrationOptions {
    /// A synchronous condition.
    pub fn when_sync(
        condition: impl Fn(&ToolActivationContext) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            when: Some(Arc::new(move |context| {
                let enabled = condition(&context);
                Box::pin(async move { Ok(enabled) })
            })),
        }
    }
}

type ViewFn = dyn Fn(Option<&ScopeKey>) -> IndexMap<String, Arc<ToolDefinition>> + Send + Sync;

struct ResolvedInner {
    closed: AtomicBool,
    view: Box<ViewFn>,
}

/// One step's admitted registrations, also used by nested PTC calls.
#[derive(Clone)]
pub struct ResolvedToolSet(Arc<ResolvedInner>);

impl ResolvedToolSet {
    pub(crate) fn new(
        view: impl Fn(Option<&ScopeKey>) -> IndexMap<String, Arc<ToolDefinition>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self(Arc::new(ResolvedInner {
            closed: AtomicBool::new(false),
            view: Box::new(view),
        }))
    }

    /// Close admission after the step's tool batch settles.
    pub fn dispose(&self) {
        self.0.closed.store(true, Ordering::SeqCst);
    }

    /// Read admitted registrations that remain visible in their original scope. Another scope is
    /// never admitted.
    pub fn view(&self, scope: Option<&ScopeKey>) -> IndexMap<String, Arc<ToolDefinition>> {
        if self.0.closed.load(Ordering::SeqCst) {
            return IndexMap::new();
        }
        (self.0.view)(scope)
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// Failure to determine a tool's step-time availability.
pub struct ToolActivationError;

impl ToolActivationError {
    pub const NAME: &'static str = "ToolActivationError";
    pub const CODE: &'static str = "TOOL_ACTIVATION_FAILED";

    /// Preserve a condition failure without treating it as normal unavailability.
    pub fn new(name: &str, cause: impl Into<BoxError>) -> HarnessError {
        HarnessError::named(
            Self::NAME,
            format!("cannot resolve availability of tool \"{name}\""),
            Self::CODE,
        )
        .with_cause(cause)
    }
}

/// The completed outcome handed to `ToolDefinition::present_result`.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    /// The final model-facing content (or the rendered error text on failure).
    pub content: Vec<ContentBlock>,
    /// Whether the call failed.
    pub is_error: bool,
    /// The tool-private presentation payload projected by its output declaration.
    pub meta: Option<Value>,
}

/// Opaque call identity that permits correlation without exposing mutable execution state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ToolExecutionToken(u64);

impl ToolExecutionToken {
    /// Mint a same-process correlation token whose identity is its value.
    pub(crate) fn mint() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// Parsed input to the shared invocation pipeline. The pipeline adds its own token to form a
/// [`ToolExecution`]; callers do not choose that token.
#[derive(Clone)]
pub struct ToolExecutionInput {
    pub call_id: ToolCallId,
    /// Root model-requested call owning this execution tree. Callers omit it for a root execution;
    /// nested dispatchers propagate the enclosing value.
    pub root_call_id: Option<ToolCallId>,
    pub name: String,
    /// Binding-time tool schema for a PTC inner call; never logged.
    pub schema: Option<ToolSchema>,
    /// Losslessly JSON-serializable parsed arguments (tools validate their own schema).
    pub arguments: Value,
    /// The agent on whose behalf the call runs (set by the agent loop).
    pub agent: Option<Arc<dyn Agent>>,
    /// Step admission shared by Native and PTC dispatch. Conditional tools require it.
    pub tool_set: Option<ResolvedToolSet>,
    /// Opaque token of the enclosing transport execution, when one exists.
    pub parent: Option<ToolExecutionToken>,
    /// Required caller-owned cancellation for this invocation.
    pub signal: AbortSignal,
}

impl ToolExecutionInput {
    /// A root call without agent, step admission, schema or parent.
    pub fn new(
        call_id: ToolCallId,
        name: impl Into<String>,
        arguments: Value,
        signal: AbortSignal,
    ) -> Self {
        Self {
            call_id,
            root_call_id: None,
            name: name.into(),
            schema: None,
            arguments,
            agent: None,
            tool_set: None,
            parent: None,
            signal,
        }
    }
}

/// Scheduling mode for one pending call. `Parallel` may overlap with siblings; `Exclusive` runs
/// alone and forms an ordering barrier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolExecutionMode {
    Parallel,
    Exclusive,
}

impl ToolExecutionMode {
    /// The `kind` string of the source's object-tagged union.
    pub fn kind(self) -> &'static str {
        match self {
            Self::Parallel => "parallel",
            Self::Exclusive => "exclusive",
        }
    }
}

/// One pending tool call inside the executor pipeline, and the runtime context handed to a tool
/// implementation after the executor has accepted it (`ToolRunContext`). Parsed arguments cross
/// one lossless-JSON boundary before policy; call identity, the caller signal, and the
/// executor-assigned token are immutable.
pub struct ToolExecution {
    pub call_id: ToolCallId,
    /// Root model-requested call, resolved for every root and nested execution.
    pub root_call_id: ToolCallId,
    pub name: String,
    pub schema: Option<ToolSchema>,
    pub arguments: Value,
    pub agent: Option<Arc<dyn Agent>>,
    pub tool_set: Option<ResolvedToolSet>,
    pub parent: Option<ToolExecutionToken>,
    /// Executor-assigned identity shared with nested calls only as their opaque `parent` token.
    pub token: ToolExecutionToken,
    signal: Mutex<AbortSignal>,
    signal_replaced: AtomicBool,
    pub(crate) caller_signal: AbortSignal,
    pub(crate) body_invoked: AtomicBool,
    deferred: Mutex<Vec<UserMessage>>,
    concluding: AtomicBool,
    pub(crate) finalizer: Option<ContentHook>,
    pub(crate) projector: Mutex<Option<ContentHook>>,
    /// Fires when the initial polling pass of this call's wrapper chain ended, or the dispatch settled.
    started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

/// The runtime context a tool body receives; the same object as [`ToolExecution`].
pub type ToolRunContext = ToolExecution;

impl ToolExecution {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn mint(
        input: &ToolExecutionInput,
        arguments: Value,
        finalizer: Option<ContentHook>,
        projector: Option<ContentHook>,
    ) -> Self {
        Self {
            call_id: input.call_id.clone(),
            root_call_id: input
                .root_call_id
                .clone()
                .unwrap_or_else(|| input.call_id.clone()),
            name: input.name.clone(),
            schema: input.schema.clone(),
            arguments,
            agent: input.agent.clone(),
            tool_set: input.tool_set.clone(),
            parent: input.parent,
            token: ToolExecutionToken::mint(),
            signal: Mutex::new(input.signal.clone()),
            signal_replaced: AtomicBool::new(false),
            caller_signal: input.signal.clone(),
            body_invoked: AtomicBool::new(false),
            deferred: Mutex::new(Vec::new()),
            concluding: AtomicBool::new(false),
            finalizer,
            projector: Mutex::new(projector),
            started: Mutex::new(None),
        }
    }

    /// Ask for the "dispatch started" acknowledgement. The source's `dispatch` runs synchronously up to
    /// its first await, so a scheduler that prepares the next call right after starting this one must be
    /// able to wait until the around-wrappers of this call have run to their first suspension.
    pub(crate) fn arm_started(&self) -> tokio::sync::oneshot::Receiver<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        *lock(&self.started) = Some(tx);
        rx
    }

    /// Acknowledge the start of dispatch; repeated calls are no-ops.
    pub(crate) fn signal_started(&self) {
        if let Some(tx) = lock(&self.started).take() {
            let _ = tx.send(());
        }
    }

    /// The cancellation signal visible to the next wrapper or tool body.
    pub fn signal(&self) -> AbortSignal {
        lock(&self.signal).clone()
    }

    /// Replace the signal for a `tools/execute` wrapper's delegated lifetime. The executor fuses every
    /// replacement with the captured caller signal, so a wrapper cannot detach caller cancellation.
    pub fn set_signal(&self, signal: AbortSignal) {
        self.signal_replaced.store(true, Ordering::SeqCst);
        *lock(&self.signal) = signal;
    }

    pub(crate) fn restore_signal(&self, signal: AbortSignal) {
        *lock(&self.signal) = signal;
    }

    pub(crate) fn signal_was_replaced(&self) -> bool {
        self.signal_replaced.load(Ordering::SeqCst)
    }

    /// Defer one context — typically a nested-dispatch context ferried by a composite tool, or a fresh
    /// plugin-sourced instruction — until this tool's final result reaches the agent loop.
    pub fn defer_context(&self, context: UserMessage) {
        lock(&self.deferred).push(context);
    }

    pub(crate) fn deferred_contexts(&self) -> Vec<UserMessage> {
        lock(&self.deferred).clone()
    }

    /// Mark a successful final result as terminal for the current agent turn.
    pub fn conclude_turn(&self) {
        self.concluding.store(true, Ordering::SeqCst);
    }

    pub(crate) fn concludes(&self) -> bool {
        self.concluding.load(Ordering::SeqCst)
    }
}

/// Structured error metadata for a failed tool call (alongside the model-facing text).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolErrorInfo {
    pub name: String,
    pub code: String,
    /// Optional raw user-facing detail; durable projections preserve it but model-facing content does not include it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl ToolErrorInfo {
    pub fn new(name: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            code: code.into(),
            reason: None,
        }
    }
}

/// Canonical failure detail; internal routing information remains optional.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolFailure {
    /// Human-readable failure message without the Native `Error: ` envelope.
    pub message: String,
    /// Internal error class/code used by policy and durable diagnostics.
    pub info: Option<ToolErrorInfo>,
}

/// Thrown (internally) when the model requests a tool that isn't registered.
pub struct ToolNotFoundError;

impl ToolNotFoundError {
    pub const NAME: &'static str = "ToolNotFoundError";
    pub const CODE: &'static str = "UNKNOWN_TOOL";

    /// `reachable_from` says how the model reaches this tool instead, when the name IS visible and
    /// only the presentation denies calling it directly.
    pub fn new(tool_name: &str, reachable_from: Option<&str>) -> HarnessError {
        HarnessError::named(
            Self::NAME,
            match reachable_from {
                None => format!("unknown tool \"{tool_name}\""),
                Some(reachable) => format!("unknown tool \"{tool_name}\": {reachable}"),
            },
            Self::CODE,
        )
    }
}

/// Thrown by a tool body when its composition or call lacks context the tool requires, such as a
/// calling Agent, an injected service, or an open turn.
pub struct ToolContextUnavailableError;

impl ToolContextUnavailableError {
    pub const NAME: &'static str = "ToolContextUnavailableError";

    pub fn new(message: impl Into<String>) -> HarnessError {
        HarnessError::named(Self::NAME, message, TOOL_CONTEXT_UNAVAILABLE)
    }
}

/// The `violations` list of a `ToolOutputError`, attached with `HarnessError::with_detail`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolOutputViolations(pub Vec<String>);

/// Thrown when a tool body or post-policy value violates its declared output.
pub struct ToolOutputError;

impl ToolOutputError {
    pub const NAME: &'static str = "ToolOutputError";
    pub const CODE: &'static str = "INVALID_TOOL_OUTPUT";

    pub fn new(tool_name: &str, violations: Vec<String>) -> HarnessError {
        HarnessError::named(
            Self::NAME,
            format!(
                "tool \"{tool_name}\" returned invalid output: {}",
                violations.join("; ")
            ),
            Self::CODE,
        )
        .with_detail(ToolOutputViolations(violations))
    }

    pub fn violations(error: &HarnessError) -> Option<&[String]> {
        error
            .detail::<ToolOutputViolations>()
            .map(|detail| detail.0.as_slice())
    }
}

/// Successful canonical tool execution, including its Native/model projection.
///
/// The registry freezes the results it creates (the source `deepFreeze`s them): a frozen result keeps its
/// original in every field, its setters change nothing, and no `&mut` access to a field exists, so a
/// wrapper cannot edit it in place or swap a field from a copy. A copy (`clone`) or a result built with
/// `new` is a new object, so its setters apply.
#[derive(Debug)]
pub struct ToolExecutionSuccess {
    /// Execution-local canonical value; deliberately omitted from durable events.
    pub(crate) value: Value,
    pub(crate) content: Vec<ContentBlock>,
    pub(crate) meta: Option<Value>,
    pub(crate) additional_contexts: Option<Vec<UserMessage>>,
    /// The agent loop stops after committing this successful result batch.
    pub(crate) concludes_turn: bool,
    pub(crate) canonical: Option<Arc<CanonicalSeal>>,
}

/// Failed canonical tool execution; failures never carry a successful value. Like the source, a failure
/// the registry normalized stays editable until its final materialization, so only a result frozen by
/// [`ToolExecutionResult::marked`] ignores setters.
#[derive(Debug)]
pub struct ToolExecutionFailure {
    pub(crate) error: ToolFailure,
    pub(crate) content: Vec<ContentBlock>,
    pub(crate) meta: Option<Value>,
    pub(crate) additional_contexts: Option<Vec<UserMessage>>,
    pub(crate) canonical: Option<Arc<CanonicalSeal>>,
}

macro_rules! setters {
    ($($setter:ident => $field:ident: $ty:ty),* $(,)?) => {
        $(
            /// Replace the field on an editable object; a frozen result keeps its original.
            pub fn $setter(&mut self, $field: $ty) {
                if !self.is_frozen() {
                    self.$field = $field;
                }
            }
        )*
    };
}

impl ToolExecutionSuccess {
    pub fn new(value: Value, content: Vec<ContentBlock>) -> Self {
        Self {
            value,
            content,
            meta: None,
            additional_contexts: None,
            concludes_turn: false,
            canonical: None,
        }
    }

    fn is_frozen(&self) -> bool {
        self.canonical.as_ref().is_some_and(|seal| seal.frozen)
    }

    pub fn value(&self) -> &Value {
        &self.value
    }

    pub fn content(&self) -> &[ContentBlock] {
        &self.content
    }

    pub fn meta(&self) -> Option<&Value> {
        self.meta.as_ref()
    }

    pub fn additional_contexts(&self) -> Option<&Vec<UserMessage>> {
        self.additional_contexts.as_ref()
    }

    pub fn concludes_turn(&self) -> bool {
        self.concludes_turn
    }

    setters! {
        set_value => value: Value,
        set_content => content: Vec<ContentBlock>,
        set_meta => meta: Option<Value>,
        set_additional_contexts => additional_contexts: Option<Vec<UserMessage>>,
        set_concludes_turn => concludes_turn: bool,
    }
}

impl ToolExecutionFailure {
    pub fn new(error: ToolFailure, content: Vec<ContentBlock>) -> Self {
        Self {
            error,
            content,
            meta: None,
            additional_contexts: None,
            canonical: None,
        }
    }

    fn is_frozen(&self) -> bool {
        self.canonical.as_ref().is_some_and(|seal| seal.frozen)
    }

    pub fn error(&self) -> &ToolFailure {
        &self.error
    }

    pub fn content(&self) -> &[ContentBlock] {
        &self.content
    }

    pub fn meta(&self) -> Option<&Value> {
        self.meta.as_ref()
    }

    pub fn additional_contexts(&self) -> Option<&Vec<UserMessage>> {
        self.additional_contexts.as_ref()
    }

    setters! {
        set_error => error: ToolFailure,
        set_content => content: Vec<ContentBlock>,
        set_meta => meta: Option<Value>,
        set_additional_contexts => additional_contexts: Option<Vec<UserMessage>>,
    }
}

// A copy is a new object in the source (spread syntax): it loses the registry's canonical mark.
impl Clone for ToolExecutionSuccess {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            content: self.content.clone(),
            meta: self.meta.clone(),
            additional_contexts: self.additional_contexts.clone(),
            concludes_turn: self.concludes_turn,
            canonical: None,
        }
    }
}

impl Clone for ToolExecutionFailure {
    fn clone(&self) -> Self {
        Self {
            error: self.error.clone(),
            content: self.content.clone(),
            meta: self.meta.clone(),
            additional_contexts: self.additional_contexts.clone(),
            canonical: None,
        }
    }
}

impl PartialEq for ToolExecutionSuccess {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
            && self.content == other.content
            && self.meta == other.meta
            && self.additional_contexts == other.additional_contexts
            && self.concludes_turn == other.concludes_turn
    }
}

impl PartialEq for ToolExecutionFailure {
    fn eq(&self, other: &Self) -> bool {
        self.error == other.error
            && self.content == other.content
            && self.meta == other.meta
            && self.additional_contexts == other.additional_contexts
    }
}

/// The registry's mark on a normalized result: the owning dispatch (the source's `canonicalResults`
/// `WeakMap`) and whether the result was also frozen.
#[derive(Debug)]
pub(crate) struct CanonicalSeal {
    pub(crate) token: ToolExecutionToken,
    pub(crate) frozen: bool,
}

/// The discriminated, execution-local outcome of one tool call.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolExecutionResult {
    Success(ToolExecutionSuccess),
    Failure(ToolExecutionFailure),
}

impl ToolExecutionResult {
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Failure(_))
    }

    pub fn content(&self) -> &[ContentBlock] {
        match self {
            Self::Success(success) => &success.content,
            Self::Failure(failure) => &failure.content,
        }
    }

    pub fn meta(&self) -> Option<&Value> {
        match self {
            Self::Success(success) => success.meta.as_ref(),
            Self::Failure(failure) => failure.meta.as_ref(),
        }
    }

    pub fn additional_contexts(&self) -> Option<&Vec<UserMessage>> {
        match self {
            Self::Success(success) => success.additional_contexts.as_ref(),
            Self::Failure(failure) => failure.additional_contexts.as_ref(),
        }
    }

    pub fn error(&self) -> Option<&ToolFailure> {
        match self {
            Self::Success(_) => None,
            Self::Failure(failure) => Some(&failure.error),
        }
    }

    pub fn value(&self) -> Option<&Value> {
        match self {
            Self::Success(success) => Some(&success.value),
            Self::Failure(_) => None,
        }
    }

    pub fn concludes_turn(&self) -> bool {
        matches!(self, Self::Success(success) if success.concludes_turn)
    }

    pub(crate) fn seal(&self) -> Option<&Arc<CanonicalSeal>> {
        match self {
            Self::Success(success) => success.canonical.as_ref(),
            Self::Failure(failure) => failure.canonical.as_ref(),
        }
    }

    /// Drop the canonical mark: the result was rebuilt and must be normalized again.
    pub(crate) fn unsealed(mut self) -> Self {
        match &mut self {
            Self::Success(success) => success.canonical = None,
            Self::Failure(failure) => failure.canonical = None,
        }
        self
    }

    /// Mark one registry-normalized result as canonical only for its owning dispatch and freeze it, as the
    /// source's `deepFreeze` does: wrappers cannot edit it in place.
    pub(crate) fn marked(self, exec: &ToolExecution) -> Self {
        self.sealed_as(exec, true)
    }

    /// Mark a result as canonical without freezing it: the source's `markCanonical` only records the
    /// owning dispatch in a `WeakMap`, and the plain object stays editable until final materialization.
    pub(crate) fn marked_editable(self, exec: &ToolExecution) -> Self {
        self.sealed_as(exec, false)
    }

    fn sealed_as(self, exec: &ToolExecution, frozen: bool) -> Self {
        let mut result = self.unsealed();
        result.set_seal(Arc::new(CanonicalSeal {
            token: exec.token,
            frozen,
        }));
        result
    }

    fn set_seal(&mut self, seal: Arc<CanonicalSeal>) {
        match self {
            Self::Success(success) => success.canonical = Some(seal),
            Self::Failure(failure) => failure.canonical = Some(seal),
        }
    }

    /// The text of every text block joined by newlines (a convenience for tests and presenters).
    pub fn text(&self) -> String {
        self.content()
            .iter()
            .filter_map(ContentBlock::text_value)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Pre-dispatch decision. `Allow` runs the call; `Deny` materializes its model-facing reason and
/// optional structured error identity; `Cancel` selects the canonical cancellation result without
/// presenting a policy denial; `Ask` runs only after an approval service returns `AllowedOnce`
/// and otherwise denies.
#[derive(Clone, Debug, PartialEq)]
pub enum PreToolDecision {
    Allow,
    Deny {
        reason: String,
        info: Option<ToolErrorInfo>,
    },
    Cancel,
    Ask {
        reason: Option<String>,
        /// Localized prompt text keyed by locale; always carries `en`.
        display_reason: Option<IndexMap<String, String>>,
    },
}

/// Post-dispatch decision: accept, replace one projection, attach context for the next request,
/// or block by turning corrective feedback into an error result.
#[derive(Clone, Debug, PartialEq)]
pub enum PostToolDecision {
    Accept {
        content: Option<Vec<ContentBlock>>,
        value: Option<Value>,
        additional_contexts: Option<Vec<UserMessage>>,
    },
    Block {
        feedback: Vec<ContentBlock>,
        info: Option<ToolErrorInfo>,
        additional_contexts: Option<Vec<UserMessage>>,
    },
}

impl PostToolDecision {
    /// `{ kind: 'accept' }`.
    pub fn accept() -> Self {
        Self::Accept {
            content: None,
            value: None,
            additional_contexts: None,
        }
    }
}

/// Per-scope filter over global tools. Restrictions intersect and do not affect scoped
/// registrations or the reserved PTC mode transport.
#[derive(Clone, Debug, Default)]
pub struct ToolRestriction {
    /// Global tool names that stay visible; everything else is removed.
    pub allow: Option<Vec<String>>,
    /// Global tool names removed from visibility.
    pub deny: Option<Vec<String>>,
}

/// A guard denial: the model-facing reason and optional structured error identity recorded on the result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolGuardDenial {
    pub reason: String,
    pub info: Option<ToolErrorInfo>,
}

impl From<&str> for ToolGuardDenial {
    fn from(reason: &str) -> Self {
        Self {
            reason: reason.to_owned(),
            info: None,
        }
    }
}

impl From<String> for ToolGuardDenial {
    fn from(reason: String) -> Self {
        Self { reason, info: None }
    }
}

/// A monotonic execution guard evaluated after every `tools/pre-execute` listener and before the tool
/// body. Returning a denial denies the call; returning `None` leaves it unchanged. An `Err` fails the
/// call like a thrown guard. Because guards have no allow result, listener ordering cannot turn a denial
/// back into permission.
pub type ToolGuard =
    Arc<dyn Fn(&ToolExecution) -> Result<Option<ToolGuardDenial>, BoxError> + Send + Sync>;
