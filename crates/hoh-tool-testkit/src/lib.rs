//! Test-support helpers for executing model-facing tools through the real tool pipeline.

use std::{future::Future, sync::Arc};

use hoh_error::BoxError;
use hoh_kernel::Context;
use hoh_llm::{ContentBlock, ToolCallBlock, ToolCallId};
use hoh_timeout::AbortSignal;
use hoh_tools::{
    Agent, ConcurrencyFn, ContentHook, DefineToolOptions, DefineToolOutput, PresentCallFn,
    PresentResultFn, ResolvedToolSet, ScheduledToolDispatch, ScheduledToolPreparation,
    ToolBatchInput, ToolDefinition, ToolExecution, ToolExecutionInput, ToolExecutionPipeline,
    ToolExecutionResult, Tools, ToolsError, define_tool,
};
use hoh_util_values::js_json_stringify;
use serde_json::{Value, json};

/// The options of `defineContentToolFixture`, excluding the fixed output declaration.
pub struct ContentToolFixtureOptions<F> {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub defer_loading: bool,
    pub timeout_ms: Option<f64>,
    pub is_concurrency_safe: Option<ConcurrencyFn>,
    pub execute: F,
    pub project_content: Option<ContentHook>,
    pub finalize_content: Option<ContentHook>,
    pub present_call: Option<PresentCallFn>,
    pub present_result: Option<PresentResultFn>,
}

impl<F> ContentToolFixtureOptions<F> {
    /// Options with the mandatory members; every optional member is absent.
    pub fn new<Fut>(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        execute: F,
    ) -> Self
    where
        F: Fn(Value, Arc<ToolExecution>) -> Fut,
        Fut: Future<Output = Result<Vec<ContentBlock>, BoxError>>,
    {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            defer_loading: false,
            timeout_ms: None,
            is_concurrency_safe: None,
            execute,
            project_content: None,
            finalize_content: None,
            present_call: None,
            present_result: None,
        }
    }
}

/// Define a tool whose canonical value and rendered result are its returned content blocks.
pub fn define_content_tool_fixture<F, Fut>(
    options: ContentToolFixtureOptions<F>,
) -> Result<ToolDefinition, BoxError>
where
    F: Fn(Value, Arc<ToolExecution>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Vec<ContentBlock>, BoxError>> + Send + 'static,
{
    let execute = options.execute;
    let mut define = DefineToolOptions::new(
        options.name,
        options.description,
        options.parameters,
        DefineToolOutput {
            schema: json!({ "type": "array", "items": { "type": "json" } }),
            // `define_tool` only captures this callback. The fallible renderer below replaces it
            // before the definition can escape this function.
            render: Arc::new(|_, _| Vec::new()),
            presentation_meta: None,
        },
        move |args, exec| {
            let future = execute(args, exec);
            Box::pin(async move { Ok(serde_json::to_value(future.await?)?) })
        },
    );
    define.defer_loading = options.defer_loading;
    define.timeout_ms = options.timeout_ms;
    define.is_concurrency_safe = options.is_concurrency_safe;
    define.project_content = options.project_content;
    define.finalize_content = options.finalize_content;
    define.present_call = options.present_call;
    define.present_result = options.present_result;

    let mut definition = define_tool(define)?;
    definition.output.render =
        Arc::new(|_, value| Ok(serde_json::from_value::<Vec<ContentBlock>>(value.clone())?));
    Ok(definition)
}

/// Drive one parsed invocation through prepare, dispatch, and complete.
pub async fn execute_tool_fixture(
    ctx: &Context,
    input: ToolExecutionInput,
) -> Result<ToolExecutionResult, BoxError> {
    let pipeline = ToolExecutionPipeline::new(ctx.clone())?;
    let prepared = pipeline.prepare(input).await;
    let exec = prepared.exec().clone();
    let dispatched = match prepared {
        ScheduledToolPreparation::Dispatch(exec) => pipeline.dispatch(exec).await,
        ScheduledToolPreparation::PostResult { result, .. } => {
            ScheduledToolDispatch::PostResult(result)
        }
        ScheduledToolPreparation::FinalResult { result, .. } => {
            ScheduledToolDispatch::FinalResult(result)
        }
    };
    Ok(pipeline.complete(&exec, dispatched).await)
}

/// Inputs for executing one raw model call through the selected tool executor.
pub struct ModelToolFixtureInput {
    pub call_id: ToolCallId,
    pub name: String,
    /// `None` represents JavaScript `undefined`; JSON `null` remains `Some(Value::Null)`.
    pub arguments: Option<Value>,
    pub signal: AbortSignal,
    pub agent: Option<Arc<dyn Agent>>,
    pub tool_set: Option<ResolvedToolSet>,
}

impl ModelToolFixtureInput {
    /// A model call without agent ownership or step admission.
    pub fn new(
        call_id: ToolCallId,
        name: impl Into<String>,
        arguments: Value,
        signal: AbortSignal,
    ) -> Self {
        Self {
            call_id,
            name: name.into(),
            arguments: Some(arguments),
            signal,
            agent: None,
            tool_set: None,
        }
    }
}

/// Execute one raw call through the executor selected by `ctx.tools`.
pub async fn execute_model_tool_fixture(
    ctx: &Context,
    input: ModelToolFixtureInput,
) -> Result<ToolExecutionResult, BoxError> {
    let arguments = input.arguments.as_ref().ok_or_else(|| {
        Box::new(ToolsError::TypeError(
            "Model tool fixture arguments must encode as JSON".into(),
        )) as BoxError
    })?;
    let call = ToolCallBlock {
        id: input.call_id,
        name: input.name,
        arguments: js_json_stringify(arguments),
    };
    let tools = ctx.get::<Tools>().ok_or_else(|| {
        Box::new(ToolsError::TypeError(
            "Cannot read properties of undefined (reading 'execute')".into(),
        )) as BoxError
    })?;
    let mut batch = ToolBatchInput::new(vec![call], input.signal);
    batch.agent = input.agent;
    batch.tool_set = input.tool_set;
    let result = tools.execute(batch).await?;
    result
        .results
        .into_iter()
        .next()
        .map(|result| result.result)
        .ok_or_else(|| {
            Box::new(ToolsError::Error(
                "Executor omitted the model tool fixture result".into(),
            )) as BoxError
        })
}
