//! Model tool descriptions and complete batch execution owned by plugins.

use std::sync::Arc;

use hoh_error::{BoxError, HarnessError};
use hoh_llm::{ToolCallBlock, ToolCallId, UserMessage};
use hoh_system_prompt::ToolProviderResult;
use hoh_timeout::AbortSignal;

use crate::contracts::{ResolvedToolSet, ToolDefinition, ToolExecutionResult};
use crate::shim::{Agent, BoxFuture};

/// Model-call schemas produced by an executor (`schemas` and `knownNames` of the prompt provider result).
pub type ToolModelInput = ToolProviderResult;

/// Text of an executor-owned prompt section.
#[derive(Clone)]
pub enum ToolPromptText {
    /// Static guidance.
    Static(String),
    /// Text generated from this step's visible capability definitions.
    Provider(Arc<dyn Fn(&[Arc<ToolDefinition>]) -> String + Send + Sync>),
}

/// Executor-owned guidance registered through the system-prompt section API.
#[derive(Clone)]
pub struct ToolPromptSection {
    pub name: String,
    pub order: f64,
    pub interpolate: Option<bool>,
    pub text: ToolPromptText,
}

/// The committed model step receiving root call/result events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolLogStep {
    pub turn: u64,
    pub step: u64,
}

/// Raw model calls, with optional Agent ownership and model-step logging.
pub struct ToolBatchInput {
    pub calls: Vec<ToolCallBlock>,
    pub signal: AbortSignal,
    /// Positive integer overriding the executor's concurrency limit for this batch.
    pub max_parallel_tool_calls: Option<u64>,
    /// Owning Agent supplies scoped tools and execution context, including outside a turn.
    pub agent: Option<Arc<dyn Agent>>,
    /// Step admission for every call in the batch, including program subcalls.
    pub tool_set: Option<ResolvedToolSet>,
    /// Committed model step receiving root call/result events; requires `agent`.
    pub log: Option<ToolLogStep>,
}

impl ToolBatchInput {
    /// A batch without agent, admission, override or logging.
    pub fn new(calls: Vec<ToolCallBlock>, signal: AbortSignal) -> Self {
        Self {
            calls,
            signal,
            max_parallel_tool_calls: None,
            agent: None,
            tool_set: None,
            log: None,
        }
    }
}

/// One settled result correlated with its original model call.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCallResult {
    pub call_id: ToolCallId,
    pub result: ToolExecutionResult,
}

/// Model-ordered outcomes and context accepted for the next step.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolBatchResult {
    pub results: Vec<ToolCallResult>,
    pub additional_contexts: Vec<UserMessage>,
    pub concluded: bool,
}

/// A plugin owns argument parsing, dispatch, scheduling, cancellation, and logging.
pub trait ToolExecutor: Send + Sync + 'static {
    /// Guidance registered and disposed with this executor; empty means no executor-owned sections.
    fn sections(&self) -> Vec<ToolPromptSection> {
        Vec::new()
    }
    /// Describe the model's callable tools: currently visible capability definitions, after scoped
    /// restrictions. Returns schemas and names used to validate tool ordering.
    fn describe(&self, tools: &[Arc<ToolDefinition>]) -> Result<ToolModelInput, BoxError>;
    /// Execute a complete batch. Returns one outcome per input call in input order, including skipped
    /// calls on abort. Never settles while started work is still running. Internal failures reject with
    /// [`ToolBatchExecutionError`] after draining, preserving committed outcomes and context.
    fn execute(&self, input: ToolBatchInput) -> BoxFuture<'_, Result<ToolBatchResult, BoxError>>;
}

/// A drained batch failed internally; only the included results are committed.
#[derive(Debug)]
pub struct ToolBatchExecutionError {
    pub cause: BoxError,
    pub partial: ToolBatchResult,
}

impl ToolBatchExecutionError {
    pub fn new(cause: BoxError, partial: ToolBatchResult) -> Self {
        Self { cause, partial }
    }
}

impl std::fmt::Display for ToolBatchExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Tool batch execution failed")
    }
}

impl std::error::Error for ToolBatchExecutionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

/// The selected provider is missing or unloaded; another provider must not take its calls.
pub struct ToolExecutorUnavailableError;

impl ToolExecutorUnavailableError {
    pub const NAME: &'static str = "ToolExecutorUnavailableError";
    pub const CODE: &'static str = "TOOL_EXECUTOR_UNAVAILABLE";

    pub fn new() -> HarnessError {
        HarnessError::named(
            Self::NAME,
            "The selected tool executor is unavailable. Load an executor in this composition before running the agent.",
            Self::CODE,
        )
    }
}
