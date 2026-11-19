//! Helpers of the s17 test files (tool-order, tool-updates, tool-executors, …). They sit next to
//! `tests/common` (owned by s6) and build on it; nothing here changes `src` or `common`.
#![allow(dead_code)]

use futures_util::{StreamExt, stream};
use hoh_agent::{Agent, AgentOptions, AgentRegistryPlugin, AgentSessionMeta, Live, ModelSelection};
use hoh_agent_loop::{AgentLoopKey, AgentLoopPlugin};
use hoh_error::BoxError;
use hoh_kernel::{Context, Fiber, Kernel, Plugin, Service};
use hoh_llm::{
    ContentBlock, GenerateOptions, Llm, LlmAdapter, LlmFuture, LlmModelInfo, LlmResolvedModelInfo,
    LlmRuntime, LlmStream, MessageSource, NewUserMessage, StreamChunk, UserMessage,
    create_user_message,
};
use hoh_session::{SessionId, SessionStore, Sessions};
use hoh_session_projection::SessionProjectionRegistry;
use hoh_system_prompt::{SystemPrompt, SystemPromptKey};
use hoh_timeout::AbortSignal;
use hoh_tool_executor_native::NativeToolExecutorPlugin;
use hoh_tools::{ToolRuntime, Tools};
use serde_json::{Value, json};
use std::sync::Arc;
use std::{
    collections::VecDeque,
    sync::{Mutex, MutexGuard},
    time::Duration,
};

pub type RequestHook = Arc<dyn Fn(&GenerateOptions) + Send + Sync>;

/// A mock adapter that also declares the update modes of the routes it serves
/// (`adapter.toolUpdate`, `adapter.systemPromptUpdate` of the source `MockAdapter`).
pub struct ModelAdapter {
    pub inner: Arc<MockAdapter>,
    pub tool_update: Option<String>,
    pub system_prompt_update: Option<String>,
    /// Runs on every request before the scripted entry streams (a script entry that is a function in
    /// the source and has a side effect).
    pub on_request: Option<RequestHook>,
}

impl ModelAdapter {
    pub fn new(inner: Arc<MockAdapter>) -> Self {
        Self {
            inner,
            tool_update: None,
            system_prompt_update: None,
            on_request: None,
        }
    }
}

impl LlmAdapter for ModelAdapter {
    fn stream(&self, options: GenerateOptions) -> Result<LlmStream, BoxError> {
        if let Some(hook) = &self.on_request {
            hook(&options);
        }
        self.inner.stream(options)
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
                tool_update: self.tool_update.clone(),
                system_prompt_update: self.system_prompt_update.clone(),
                ..Default::default()
            })
        })
    }
}

/// What a test mounts: the configuration of the system prompt, of the loop, and whether the native
/// executor is selected at the root.
pub struct StackOptions {
    pub system_prompt: Value,
    pub loop_config: Value,
    pub native_executor: bool,
    /// Mount the JSONL backend with this configuration, before or after the loop.
    pub persistence: Option<Persistence>,
}

pub struct Persistence {
    pub config: Value,
    pub before_loop: bool,
}

impl Default for StackOptions {
    fn default() -> Self {
        Self {
            system_prompt: json!({}),
            loop_config: json!({ "agents": [] }),
            native_executor: true,
            persistence: None,
        }
    }
}

pub struct Stack {
    pub kernel: Kernel,
    pub ctx: Context,
    pub tools: Service<Tools>,
    pub system_prompt: Service<SystemPromptKey>,
    pub loop_fiber: Fiber,
    _registration: hoh_llm::AdapterRegistrationHandle,
}

async fn mount<P: Plugin>(ctx: &Context, config: Value) -> Result<Fiber, BoxError> {
    let fiber = ctx.plugin::<P>(config)?;
    fiber.settled().await?;
    Ok(fiber)
}

/// `harness(adapter)` of the source suites: LlmRuntime, SessionStore, SessionProjectionRegistry,
/// SystemPrompt, ToolRuntime, the native executor, AgentRegistry and AgentLoop, then the adapter on `mock`.
pub async fn stack(adapter: Arc<dyn LlmAdapter>, options: StackOptions) -> Stack {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.provide::<Llm>(Arc::new(LlmRuntime::with_context(&ctx)))
        .await
        .expect("llm provides");
    ctx.provide::<Sessions>(Arc::new(SessionStore::new()))
        .await
        .expect("sessions provide");
    mount::<SessionProjectionRegistry>(&ctx, json!({}))
        .await
        .expect("projections mount");
    mount::<SystemPrompt>(&ctx, options.system_prompt)
        .await
        .expect("system prompt mounts");
    mount::<ToolRuntime>(&ctx, json!({}))
        .await
        .expect("tools mount");
    if options.native_executor {
        mount::<NativeToolExecutorPlugin>(&ctx, json!({}))
            .await
            .expect("native executor mounts");
    }
    mount::<AgentRegistryPlugin>(&ctx, json!({}))
        .await
        .expect("agent registry mounts");
    let mut persistence = options.persistence;
    if let Some(backend) = persistence.take_if(|backend| backend.before_loop) {
        mount::<hoh_session_persistence_jsonl::JsonlSessionPersistencePlugin>(&ctx, backend.config)
            .await
            .expect("persistence mounts");
    }
    let loop_fiber = mount::<AgentLoopPlugin>(&ctx, options.loop_config)
        .await
        .expect("agent loop mounts");
    if let Some(backend) = persistence {
        mount::<hoh_session_persistence_jsonl::JsonlSessionPersistencePlugin>(&ctx, backend.config)
            .await
            .expect("persistence mounts");
    }
    let registration = ctx
        .get::<Llm>()
        .expect("llm service")
        .register_adapter(&ctx, vec!["mock".into()], adapter)
        .expect("adapter registers");
    Stack {
        tools: ctx.get::<Tools>().expect("tools service"),
        system_prompt: ctx.get::<SystemPromptKey>().expect("system prompt service"),
        kernel,
        ctx,
        loop_fiber,
        _registration: registration,
    }
}

impl Stack {
    /// `ctx.agentLoop.create(id, { provider: 'mock', model })`.
    pub async fn create_agent(&self, id: &str, model: &str) -> Arc<dyn Agent> {
        let agent_loop = self.ctx.get::<AgentLoopKey>().expect("agent loop service");
        agent_loop
            .create(
                SessionId::from(id),
                AgentOptions {
                    provider: Some(Live::Value("mock".into())),
                    model: Some(Live::Value(model.into())),
                    ..AgentOptions::default()
                },
                AgentSessionMeta::default(),
            )
            .await
            .expect("agent creates")
    }

    pub async fn close(self) {
        self.kernel.shutdown().await;
    }
}

pub fn mock_agent_options() -> AgentOptions {
    AgentOptions {
        provider: Some(Live::Value("mock".into())),
        model: Some(Live::Value("mock".into())),
        ..AgentOptions::default()
    }
}

/// `agent.model.select({ provider: 'mock', model })`.
pub fn select_model(agent: &Arc<dyn Agent>, model: &str) {
    agent
        .model()
        .select(&ModelSelection {
            provider: "mock".into(),
            model: model.into(),
            reasoning_effort: None,
        })
        .expect("model selects");
}

pub fn tools_agent(agent: &Arc<dyn Agent>) -> Arc<dyn hoh_tools::Agent> {
    agent.clone()
}

/// Events of one type as the log holds them (`snapshotEvents().filter(e => e.type === …)`).
pub fn events_of_types(agent: &Arc<dyn Agent>, types: &[&str]) -> Vec<Value> {
    agent
        .session()
        .snapshot_events(None, None)
        .iter()
        .filter(|event| types.contains(&event.event_type()))
        .map(|event| event.as_value().clone())
        .collect()
}

/// Names of a header's tools, in header order (`header?.tools?.map(tool => tool.name)`).
pub fn header_tool_names(agent: &Arc<dyn Agent>) -> Option<Vec<String>> {
    let events = agent.session().snapshot_events(None, None);
    let header = hoh_session::fold_request_header(&events, None)?;
    let tools = header.get("tools")?.as_array()?;
    Some(
        tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap_or_default().to_owned())
            .collect(),
    )
}

pub fn tool_names(options: &GenerateOptions) -> Option<Vec<String>> {
    options
        .tools
        .as_ref()
        .map(|tools| tools.iter().map(|tool| tool.name.clone()).collect())
}

/// `expect(actual).toMatchObject(expected)`: objects match as subsets, arrays element by element at
/// equal length, everything else by equality.
pub fn matches_object(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Object(actual), Value::Object(expected)) => expected.iter().all(|(key, want)| {
            actual
                .get(key)
                .is_some_and(|have| matches_object(have, want))
        }),
        (Value::Array(actual), Value::Array(expected)) => {
            actual.len() == expected.len()
                && actual
                    .iter()
                    .zip(expected)
                    .all(|(have, want)| matches_object(have, want))
        }
        _ => actual == expected,
    }
}

#[track_caller]
pub fn assert_match_object(actual: &Value, expected: &Value) {
    assert!(
        matches_object(actual, expected),
        "toMatchObject failed\n  actual:   {actual}\n  expected: {expected}"
    );
}

/// `defineContentToolFixture({ name, description, parameters, execute })`: the content blocks are the
/// canonical value (no concurrency opt-in).
pub fn fixture_tool<F, Fut>(
    name: &str,
    description: &str,
    parameters: Value,
    execute: F,
) -> hoh_tools::ToolDefinition
where
    F: Fn(Value, Arc<hoh_tools::ToolExecution>) -> Fut + Send + Sync + 'static,
    Fut:
        std::future::Future<Output = Result<Vec<hoh_llm::ContentBlock>, BoxError>> + Send + 'static,
{
    let options = hoh_tools::DefineToolOptions::new(
        name,
        description,
        parameters,
        hoh_tools::DefineToolOutput {
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
    hoh_tools::define_tool(options).expect("tool defines")
}

/// A fixture tool whose body returns one text block.
pub fn text_tool(name: &str, description: &str, text: &str) -> hoh_tools::ToolDefinition {
    let text = text.to_owned();
    fixture_tool(name, description, json!({}), move |_, _| {
        let text = text.clone();
        async move { Ok(vec![hoh_llm::ContentBlock::text(text)]) }
    })
}

// ---- copies of the helpers of tests/common (owned by s6; kept independent of its in-flux file) ----

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

pub async fn settle(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}
