//! Shared harness: the port of `harness()` and `MockAdapter` of the source test suite.
#![allow(dead_code)]

use futures_util::{StreamExt, stream};
use hoh_agent::{
    Agent, AgentFactory, AgentOptions, AgentRegistryPlugin, Agents, CreateAgentOptions, Live,
};
use hoh_agent_loop::AgentLoopPlugin;
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Kernel, Plugin, Service};
use hoh_llm::{
    ContentBlock, GenerateOptions, Llm, LlmAdapter, LlmRuntime, LlmStream, MessageSource,
    NewUserMessage, StreamChunk, UserMessage, create_user_message,
};
use hoh_session::{SessionId, SessionStore, Sessions};
use hoh_session_projection::SessionProjectionRegistry;
use hoh_system_prompt::SystemPrompt;
use hoh_tool_executor_native::NativeToolExecutor;
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, ToolDefinition, ToolExecution, ToolRegistrationOptions,
    ToolRuntime, Tools, define_tool,
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn chunk(value: Value) -> StreamChunk {
    serde_json::from_value(value).expect("stream chunk")
}

/// `textResponse`: one char per delta, usage, stop.
pub fn text_response(text: &str) -> Vec<StreamChunk> {
    finished_text(text, "stop")
}

/// `maxTokensResponse`: the model was cut off at the output-token ceiling.
pub fn max_tokens_response(text: &str) -> Vec<StreamChunk> {
    finished_text(text, "max-tokens")
}

fn finished_text(text: &str, finish: &str) -> Vec<StreamChunk> {
    let mut chunks = vec![chunk(
        json!({"type":"block-start","index":0,"blockType":"text"}),
    )];
    chunks.extend(
        text.chars()
            .map(|c| chunk(json!({"type":"text-delta","index":0,"text":c.to_string()}))),
    );
    chunks.push(chunk(
        json!({"type":"block-end","index":0,"block":{"type":"text","text":text}}),
    ));
    chunks.push(chunk(json!({"type":"usage","usage":{
        "inputTokens":10,"outputTokens":text.encode_utf16().count()}})));
    chunks.push(chunk(json!({"type":"finish","reason":{"kind":finish}})));
    chunks
}

/// `toolCallResponse`.
pub fn tool_call_response(
    call_id: &str,
    name: &str,
    args: &Value,
    text: Option<&str>,
) -> Vec<StreamChunk> {
    let arguments = args.to_string();
    let split = arguments
        .char_indices()
        .nth(5)
        .map_or(arguments.len(), |(i, _)| i);
    let mut chunks = Vec::new();
    let mut index = 0;
    if let Some(text) = text.filter(|t| !t.is_empty()) {
        chunks.push(chunk(
            json!({"type":"block-start","index":index,"blockType":"text"}),
        ));
        chunks.push(chunk(
            json!({"type":"text-delta","index":index,"text":text}),
        ));
        chunks.push(chunk(
            json!({"type":"block-end","index":index,"block":{"type":"text","text":text}}),
        ));
        index += 1;
    }
    chunks.push(chunk(
        json!({"type":"block-start","index":index,"blockType":"tool-call"}),
    ));
    chunks.push(chunk(
        json!({"type":"tool-call-delta","index":index,"id":call_id,
        "name":name,"argumentsDelta":&arguments[..split]}),
    ));
    chunks.push(chunk(
        json!({"type":"tool-call-delta","index":index,"id":call_id,
        "argumentsDelta":&arguments[split..]}),
    ));
    chunks.push(chunk(json!({"type":"block-end","index":index,"block":{
        "type":"tool-call","id":call_id,"name":name,"arguments":arguments}})));
    chunks.push(chunk(
        json!({"type":"usage","usage":{"inputTokens":10,"outputTokens":5}}),
    ));
    chunks.push(chunk(
        json!({"type":"finish","reason":{"kind":"tool-calls"}}),
    ));
    chunks
}

fn default_head() -> Vec<StreamChunk> {
    vec![
        chunk(json!({"type":"block-start","index":0,"blockType":"text"})),
        chunk(json!({"type":"text-delta","index":0,"text":"partial"})),
    ]
}

pub enum Entry {
    Chunks(Vec<StreamChunk>),
    /// Streams one chunk then waits until aborted.
    Hang,
    /// As `Hang`, but the stream notices the abort only after this long (`hang-slow`: 50ms).
    HangSlow(Duration),
    /// Streams exactly these chunks, then waits until aborted (`hangAfter`).
    HangAfter(Vec<StreamChunk>),
    /// The stream fails at once with this message.
    Fail(&'static str),
}

/// Mock adapter driven by a script: each model call consumes the next entry and records its request.
pub struct MockAdapter {
    script: Mutex<VecDeque<Entry>>,
    pub requests: Mutex<Vec<GenerateOptions>>,
}

impl MockAdapter {
    pub fn new(script: Vec<Entry>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into()),
            requests: Mutex::new(Vec::new()),
        })
    }
    pub fn chunks(script: Vec<Vec<StreamChunk>>) -> Arc<Self> {
        Self::new(script.into_iter().map(Entry::Chunks).collect())
    }
    pub fn request_count(&self) -> usize {
        lock(&self.requests).len()
    }
}

impl LlmAdapter for MockAdapter {
    fn stream(&self, options: GenerateOptions) -> Result<LlmStream, BoxError> {
        let signal = options.signal.clone();
        lock(&self.requests).push(options);
        match lock(&self.script).pop_front() {
            None => Ok(LlmStream::new(stream::once(async {
                Err::<StreamChunk, BoxError>("MockAdapter: script exhausted".into())
            }))),
            Some(Entry::Chunks(chunks)) => Ok(LlmStream::chunks(chunks)),
            Some(Entry::Fail(message)) => Ok(LlmStream::new(stream::once(async move {
                Err::<StreamChunk, BoxError>(message.into())
            }))),
            Some(entry @ (Entry::Hang | Entry::HangSlow(_) | Entry::HangAfter(_))) => {
                let (head, delay) = match entry {
                    Entry::HangAfter(head) => (head, Duration::ZERO),
                    Entry::HangSlow(delay) => (default_head(), delay),
                    _ => (default_head(), Duration::ZERO),
                };
                let tail = stream::once(async move {
                    if let Some(signal) = signal {
                        signal.cancelled().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                    tokio::time::sleep(delay).await;
                    Err::<StreamChunk, BoxError>("aborted".into())
                });
                Ok(LlmStream::new(
                    stream::iter(head.into_iter().map(Ok)).chain(tail),
                ))
            }
        }
    }
}

#[derive(Inject)]
struct NativeDeps {
    tools: Service<Tools>,
}

/// Configuration of the native executor plugin: the executor's own concurrency limit (`executorLimit`).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ExecutorConfig {
    limit: Option<u64>,
}

struct NativeExecutorPlugin;

impl Plugin for NativeExecutorPlugin {
    const NAME: &'static str = "native-tool-executor";
    type Config = ExecutorConfig;
    type Deps = NativeDeps;
    async fn apply(
        ctx: Context,
        deps: NativeDeps,
        config: Cfg<ExecutorConfig>,
    ) -> Result<(), BoxError> {
        let limit = config.get().limit.unwrap_or(10);
        deps.tools.set_executor(
            &ctx,
            Arc::new(NativeToolExecutor::with_max_parallel(
                ctx.clone(),
                move || limit,
            )?),
        )?;
        Ok(())
    }
}

pub struct Env {
    pub kernel: Kernel,
    pub ctx: Context,
    pub tools: Service<Tools>,
    pub adapter: Arc<MockAdapter>,
    /// The mounted agent-loop plugin (its disposal unloads the factory).
    pub loop_fiber: hoh_kernel::Fiber,
    _registration: hoh_llm::AdapterRegistrationHandle,
}

/// Mount every service the loop needs, then the loop itself with `config`.
pub async fn harness_with(adapter: Arc<MockAdapter>, config: Value) -> Env {
    try_harness(adapter, config, None, None)
        .await
        .unwrap_or_else(|error| panic!("harness: {error}"))
}

/// The full harness: `config` goes to the loop, `executor_limit` to the native executor and
/// `persist_root` mounts the JSONL backend (before the loop, so teardown unwinds the loop first).
pub async fn try_harness(
    adapter: Arc<MockAdapter>,
    config: Value,
    executor_limit: Option<u64>,
    persist_root: Option<&std::path::Path>,
) -> Result<Env, BoxError> {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.provide::<Llm>(Arc::new(LlmRuntime::with_context(&ctx)))
        .await
        .expect("llm provides");
    ctx.provide::<Sessions>(Arc::new(SessionStore::new()))
        .await
        .expect("sessions provide");
    mount::<SessionProjectionRegistry>(&ctx, json!({})).await?;
    mount::<SystemPrompt>(&ctx, json!({})).await?;
    mount::<ToolRuntime>(&ctx, json!({})).await?;
    mount::<NativeExecutorPlugin>(&ctx, json!({ "limit": executor_limit })).await?;
    mount::<AgentRegistryPlugin>(&ctx, json!({})).await?;
    if let Some(root) = persist_root {
        mount::<hoh_session_persistence_jsonl::JsonlSessionPersistencePlugin>(
            &ctx,
            json!({ "root": root.to_str().expect("utf-8 root"), "compression": "none" }),
        )
        .await?;
    }
    let loop_fiber = mount::<AgentLoopPlugin>(&ctx, config).await?;
    let registration = ctx
        .get::<Llm>()
        .expect("llm service")
        .register_adapter(&ctx, vec!["mock".into()], adapter.clone())
        .expect("adapter registers");
    let tools = ctx.get::<Tools>().expect("tools service");
    Ok(Env {
        kernel,
        ctx,
        tools,
        adapter,
        loop_fiber,
        _registration: registration,
    })
}

pub async fn harness(adapter: Arc<MockAdapter>) -> Env {
    harness_with(adapter, json!({ "agents": [] })).await
}

/// `harness(adapter, maxParallelToolCalls, executorLimit)` of tool-calls.spec.ts.
pub async fn harness_limits(
    adapter: Arc<MockAdapter>,
    max_parallel: Option<u64>,
    executor_limit: Option<u64>,
) -> Env {
    let mut config = json!({ "agents": [] });
    if let Some(limit) = max_parallel {
        config["maxParallelToolCalls"] = json!(limit);
    }
    try_harness(adapter, config, executor_limit, None)
        .await
        .expect("harness")
}

/// A harness on a JSONL store rooted at `root` (`mountPersistentHarness`).
pub async fn persistent_harness(adapter: Arc<MockAdapter>, root: &std::path::Path) -> Env {
    try_harness(adapter, json!({ "agents": [] }), None, Some(root))
        .await
        .expect("persistent harness")
}

impl Env {
    pub async fn close(self) {
        self.kernel.shutdown().await;
    }
}

async fn mount<P: Plugin>(ctx: &Context, config: Value) -> Result<hoh_kernel::Fiber, BoxError> {
    let fiber = ctx.plugin::<P>(config)?;
    fiber.settled().await?;
    Ok(fiber)
}

pub fn mock_options() -> AgentOptions {
    AgentOptions {
        provider: Some(Live::Value("mock".into())),
        model: Some(Live::Value("mock".into())),
        ..AgentOptions::default()
    }
}

/// `ctx.agentLoop.create(id, { provider: 'mock', model: 'mock' })`.
pub async fn create_agent(env: &Env, id: &str) -> Arc<dyn Agent> {
    let agent_loop = env
        .ctx
        .get::<hoh_agent_loop::AgentLoopKey>()
        .expect("agent loop service");
    agent_loop
        .create(
            SessionId::from(id),
            mock_options(),
            hoh_agent::AgentSessionMeta::default(),
        )
        .await
        .expect("agent creates")
}

pub fn user_message(text: &str) -> UserMessage {
    create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source: MessageSource::new("user"),
    })
}

/// Send a waking follow-up and wait for the driver to go idle.
pub async fn send_and_wait(agent: &Arc<dyn Agent>, text: &str) {
    agent.followup(user_message(text)).expect("followup");
    agent.when_idle().await;
}

pub fn event_types(agent: &Arc<dyn Agent>) -> Vec<String> {
    agent
        .session()
        .snapshot_events(None, None)
        .iter()
        .map(|event| event.event_type().to_owned())
        .collect()
}

/// All user-message texts recorded in the log (to assert what actually ran).
pub fn user_texts(agent: &Arc<dyn Agent>) -> Vec<String> {
    agent
        .session()
        .snapshot_events(None, None)
        .iter()
        .filter(|event| event.event_type() == "user/message")
        .flat_map(|event| {
            event.data()["content"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str().map(str::to_owned))
        .collect()
}

pub fn last_event(agent: &Arc<dyn Agent>, event_type: &str) -> Option<Value> {
    agent
        .session()
        .snapshot_events(None, None)
        .iter()
        .rfind(|event| event.event_type() == event_type)
        .map(|event| event.as_value().clone())
}

pub fn unused(_: &dyn AgentFactory, _: &Agents, _: &CreateAgentOptions) {}

/// `multiCall`: one assistant response carrying the supplied tool calls, no text.
pub fn multi_call(calls: &[(&str, &str, Value)]) -> Vec<StreamChunk> {
    let mut chunks = Vec::new();
    for (index, (id, name, args)) in calls.iter().enumerate() {
        chunks.push(chunk(
            json!({"type":"block-start","index":index,"blockType":"tool-call"}),
        ));
        chunks.push(chunk(json!({"type":"block-end","index":index,"block":{
            "type":"tool-call","id":id,"name":name,"arguments":args.to_string()}})));
    }
    chunks.push(chunk(
        json!({"type":"usage","usage":{"inputTokens":5,"outputTokens":5}}),
    ));
    chunks.push(chunk(
        json!({"type":"finish","reason":{"kind":"tool-calls"}}),
    ));
    chunks
}

/// `defineContentToolFixture`: the content blocks are the canonical value.
pub fn content_tool<F, Fut>(
    name: &str,
    parameters: Value,
    concurrency_safe: bool,
    execute: F,
) -> ToolDefinition
where
    F: Fn(Value, Arc<ToolExecution>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Vec<ContentBlock>, BoxError>> + Send + 'static,
{
    let mut options = DefineToolOptions::new(
        name,
        format!("fixture {name}"),
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
            Box::pin(async move { Ok(serde_json::to_value(future.await?)?) })
        },
    );
    if concurrency_safe {
        options.is_concurrency_safe = Some(Arc::new(|_| true));
    }
    define_tool(options).expect("tool defines")
}

pub fn id_param() -> Value {
    json!({ "id": { "type": "string", "required": true } })
}

pub fn register_tool(env: &Env, tool: ToolDefinition) -> hoh_kernel::Disposer {
    env.tools
        .register(&env.ctx, tool, ToolRegistrationOptions::default())
        .expect("tool registers")
}

/// A tool whose calls block until the test releases them by id (`gatedTool`).
pub struct Gated {
    pub tool: ToolDefinition,
    pub started: Arc<Mutex<Vec<String>>>,
    gates: Arc<Mutex<std::collections::BTreeMap<String, tokio::sync::oneshot::Sender<()>>>>,
}

impl Gated {
    pub fn new(name: &str, parallel: bool) -> Self {
        let started = Arc::new(Mutex::new(Vec::new()));
        let gates = Arc::new(Mutex::new(std::collections::BTreeMap::new()));
        let (log, table) = (started.clone(), gates.clone());
        let tool = content_tool(name, id_param(), parallel, move |args, _| {
            let (log, table) = (log.clone(), table.clone());
            async move {
                let id = args["id"].as_str().unwrap_or_default().to_owned();
                lock(&log).push(id.clone());
                let (open, wait) = tokio::sync::oneshot::channel();
                lock(&table).insert(id.clone(), open);
                let _ = wait.await;
                Ok(vec![ContentBlock::text(format!("done-{id}"))])
            }
        });
        Self {
            tool,
            started,
            gates,
        }
    }
    pub fn parallel(name: &str) -> Self {
        Self::new(name, true)
    }
    pub fn exclusive(name: &str) -> Self {
        Self::new(name, false)
    }
    pub fn started(&self) -> Vec<String> {
        lock(&self.started).clone()
    }
    pub fn release(&self, id: &str) {
        if let Some(open) = lock(&self.gates).remove(id) {
            let _ = open.send(());
        }
    }
    pub fn pending(&self) -> Vec<String> {
        lock(&self.gates).keys().cloned().collect()
    }
}

/// Poll until `predicate` holds, letting the runtime drain between checks.
pub async fn until(mut predicate: impl FnMut() -> bool) {
    for _ in 0..3000 {
        if predicate() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert!(predicate(), "until: condition never held");
}

pub async fn settle(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

/// All events of one type, as raw JSON.
pub fn events_of(agent: &Arc<dyn Agent>, event_type: &str) -> Vec<Value> {
    agent
        .session()
        .snapshot_events(None, None)
        .iter()
        .filter(|event| event.event_type() == event_type)
        .map(|event| event.as_value().clone())
        .collect()
}

/// `tool/result` events as `(callId, error.code)`.
pub fn result_codes(agent: &Arc<dyn Agent>) -> Vec<(String, Option<String>)> {
    events_of(agent, "tool/result")
        .iter()
        .map(|event| {
            (
                event["data"]["message"]["toolCallId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                event["data"]["error"]["code"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

pub fn call_ids(agent: &Arc<dyn Agent>, event_type: &str) -> Vec<String> {
    events_of(agent, event_type)
        .iter()
        .map(|event| {
            event["data"]["callId"]
                .as_str()
                .or_else(|| event["data"]["message"]["toolCallId"].as_str())
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}
