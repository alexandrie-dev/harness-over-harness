//! Shared test support: the native executor, an agent stand-in, tool fixtures (the ports of
//! `@deepseek-ai/dsh-tool-testkit`) and an approval answerer.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Kernel, NoConfig, Plugin, Service};
use hoh_llm::{ContentBlock, ToolCallBlock, ToolCallId};
use hoh_scope::ScopeKey;
use hoh_session::Session;
use hoh_system_prompt::SystemPrompt;
use hoh_timeout::{AbortController, AbortSignal};
use hoh_tools::shim::{Approval, ApprovalApi, ApprovalOutcome, ApprovalRequest, BoxFuture};
use hoh_tools::{
    Agent, DefineToolOptions, DefineToolOutput, ToolBatchInput, ToolBatchResult, ToolDefinition,
    ToolExecution, ToolExecutionInput, ToolExecutionPipeline, ToolExecutionResult, ToolExecutor,
    ToolModelInput, ToolRegistrationOptions, ToolRuntime, Tools, ToolsService, define_tool,
    execute_tool_batch, project_tool_schema,
};
use serde_json::{Value, json};

pub fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn call_id(id: &str) -> ToolCallId {
    id.into()
}

/// A signal that never aborts.
pub fn live_signal() -> AbortSignal {
    AbortController::new().signal()
}

/// The native executor of `@deepseek-ai/dsh-tool-executor-native`, written against the real pipeline.
pub struct NativeExecutor {
    pipeline: ToolExecutionPipeline,
    max_parallel: u64,
}

impl NativeExecutor {
    pub fn new(ctx: Context) -> Self {
        Self::with_limit(ctx, 10)
    }
    pub fn with_limit(ctx: Context, max_parallel: u64) -> Self {
        Self {
            pipeline: ToolExecutionPipeline::new(ctx).expect("tools service is mounted"),
            max_parallel,
        }
    }
}

impl ToolExecutor for NativeExecutor {
    fn describe(&self, tools: &[Arc<ToolDefinition>]) -> Result<ToolModelInput, BoxError> {
        let schemas = tools
            .iter()
            .map(|definition| project_tool_schema(definition, false))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ToolModelInput {
            schemas,
            known_names: None,
        })
    }
    fn execute(&self, input: ToolBatchInput) -> BoxFuture<'_, Result<ToolBatchResult, BoxError>> {
        Box::pin(async move {
            let limit = self.max_parallel;
            execute_tool_batch(input, &self.pipeline, &move || limit).await
        })
    }
}

#[derive(Inject)]
pub struct NativeDeps {
    tools: Service<Tools>,
}

pub struct NativeExecutorPlugin;

impl Plugin for NativeExecutorPlugin {
    const NAME: &'static str = "native-tool-executor";
    type Config = NoConfig;
    type Deps = NativeDeps;
    async fn apply(ctx: Context, deps: NativeDeps, _config: Cfg<NoConfig>) -> Result<(), BoxError> {
        deps.tools
            .set_executor(&ctx, Arc::new(NativeExecutor::new(ctx.clone())))?;
        Ok(())
    }
}

pub struct Env {
    pub kernel: Kernel,
    pub ctx: Context,
    pub tools: Service<Tools>,
    pub prompt: Service<hoh_system_prompt::SystemPromptKey>,
}

impl Env {
    pub fn service(&self) -> &ToolsService {
        &self.tools
    }
}

/// SystemPrompt, ToolRuntime and the native executor in a fresh kernel.
pub async fn setup() -> Env {
    let env = setup_without_executor().await;
    env.ctx
        .plugin::<NativeExecutorPlugin>(json!({}))
        .expect("plugin mounts")
        .settled()
        .await
        .expect("executor settles");
    env
}

pub async fn setup_without_executor() -> Env {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SystemPrompt>(json!({}))
        .expect("system prompt mounts")
        .settled()
        .await
        .expect("system prompt settles");
    ctx.plugin::<ToolRuntime>(json!({}))
        .expect("tools mount")
        .settled()
        .await
        .expect("tools settle");
    let tools = ctx.get::<Tools>().expect("tools service");
    let prompt = ctx.get().expect("system prompt service");
    Env {
        kernel,
        ctx,
        tools,
        prompt,
    }
}

/// An agent stand-in: a scope key and a standalone session.
pub struct TestAgent {
    pub key: ScopeKey,
    pub session: Session,
}

impl TestAgent {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            key: ScopeKey::new(),
            session: Session::create("test".into(), None, None, None, None).expect("session"),
        })
    }
}

impl Agent for TestAgent {
    fn scope_key(&self) -> ScopeKey {
        self.key.clone()
    }
    fn session(&self) -> Session {
        self.session.clone()
    }
}

pub fn text(text: &str) -> ContentBlock {
    ContentBlock::text(text)
}

/// `defineContentToolFixture`: the content blocks are the canonical value.
pub fn content_tool<F, Fut>(name: &str, parameters: Value, execute: F) -> ToolDefinition
where
    F: Fn(Value, Arc<ToolExecution>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Vec<ContentBlock>, BoxError>> + Send + 'static,
{
    content_tool_with(name, parameters, execute, |_| {})
}

pub fn content_tool_with<F, Fut>(
    name: &str,
    parameters: Value,
    execute: F,
    customize: impl FnOnce(&mut DefineToolOptions),
) -> ToolDefinition
where
    F: Fn(Value, Arc<ToolExecution>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Vec<ContentBlock>, BoxError>> + Send + 'static,
{
    let mut options = DefineToolOptions::new(
        name,
        "",
        parameters,
        DefineToolOutput {
            schema: json!({ "type": "array", "items": { "type": "json" } }),
            render: Arc::new(|_, value| {
                serde_json::from_value(value.clone()).expect("content blocks")
            }),
            presentation_meta: None,
        },
        move |args, exec| {
            let future = execute(args, exec);
            Box::pin(async move {
                let blocks = future.await?;
                Ok(serde_json::to_value(blocks)?)
            })
        },
    );
    customize(&mut options);
    define_tool(options).expect("tool defines")
}

/// The `echo` tool of tools.spec.ts.
pub fn echo_tool() -> ToolDefinition {
    define_tool(DefineToolOptions::new(
        "echo",
        "echo arguments back",
        json!({ "text": { "type": "string" } }),
        DefineToolOutput {
            schema: json!({ "type": "string" }),
            render: Arc::new(|_, value| vec![text(value.as_str().expect("string"))]),
            presentation_meta: None,
        },
        |args, _| {
            Box::pin(async move { Ok(args.get("text").cloned().unwrap_or_else(|| json!(""))) })
        },
    ))
    .expect("echo defines")
}

pub fn register(env: &Env, definition: ToolDefinition) -> hoh_kernel::Disposer {
    env.tools
        .register(&env.ctx, definition, ToolRegistrationOptions::default())
        .expect("registers")
}

pub fn input(call: &str, name: &str, arguments: Value) -> ToolExecutionInput {
    ToolExecutionInput::new(call_id(call), name, arguments, live_signal())
}

/// `executeToolFixture`: drive the invocation stages directly.
pub async fn execute_tool_fixture(ctx: &Context, input: ToolExecutionInput) -> ToolExecutionResult {
    let pipeline = ToolExecutionPipeline::new(ctx.clone()).expect("pipeline");
    let prepared = pipeline.prepare(input).await;
    let exec = prepared.exec().clone();
    let outcome = match prepared {
        hoh_tools::ScheduledToolPreparation::Dispatch(exec) => pipeline.dispatch(exec).await,
        hoh_tools::ScheduledToolPreparation::PostResult { result, .. } => {
            hoh_tools::ScheduledToolDispatch::PostResult(result)
        }
        hoh_tools::ScheduledToolPreparation::FinalResult { result, .. } => {
            hoh_tools::ScheduledToolDispatch::FinalResult(result)
        }
    };
    pipeline.complete(&exec, outcome).await
}

pub fn model_call(id: &str, name: &str, arguments: &str) -> ToolCallBlock {
    ToolCallBlock {
        id: call_id(id),
        name: name.into(),
        arguments: arguments.into(),
    }
}

/// `executeModelToolFixture`: one raw call through the selected executor.
pub async fn execute_model_tool_fixture(
    env: &Env,
    id: &str,
    name: &str,
    arguments: &Value,
    signal: AbortSignal,
) -> ToolExecutionResult {
    let batch = env
        .tools
        .execute(ToolBatchInput::new(
            vec![model_call(id, name, &arguments.to_string())],
            signal,
        ))
        .await
        .expect("batch settles");
    batch
        .results
        .into_iter()
        .next()
        .expect("executor returned a result")
        .result
}

/// An approval service answering with a fixed outcome and recording the request fields it saw.
pub struct Answerer {
    pub outcome: Mutex<Result<ApprovalOutcome, String>>,
    pub seen: Mutex<Vec<(String, ToolCallId, Option<String>)>>,
}

impl Answerer {
    pub fn new(outcome: ApprovalOutcome) -> Arc<Self> {
        Arc::new(Self {
            outcome: Mutex::new(Ok(outcome)),
            seen: Mutex::new(Vec::new()),
        })
    }
}

impl ApprovalApi for Answerer {
    fn request(
        &self,
        request: ApprovalRequest,
    ) -> BoxFuture<'_, Result<ApprovalOutcome, BoxError>> {
        lock(&self.seen).push((request.tool_name, request.call_id, request.reason));
        let outcome = lock(&self.outcome).clone();
        Box::pin(async move { outcome.map_err(|message| message.into()) })
    }
}

pub async fn mount_approval(env: &Env, answerer: Arc<Answerer>) {
    env.ctx
        .provide::<Approval>(answerer)
        .await
        .expect("approval provides");
}
