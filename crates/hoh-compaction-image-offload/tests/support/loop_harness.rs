//! Agent-loop harness: the port of `harness()` (`mountAgentLoopTestDependencies`) and of the
//! `ScriptedAdapter` of image-offload.spec.ts, written against the mounted services of this
//! workspace (`hoh-agent-loop/tests/common` is the pattern; its helpers cannot be imported).
#![allow(dead_code)]

use futures_util::stream;
use hoh_agent::{Agent, AgentOptions, AgentRegistryPlugin, Live};
use hoh_agent_loop::AgentLoopPlugin;
use hoh_compaction_image_offload::ImageOffloadPlugin;
use hoh_error::{BoxError, HarnessError};
use hoh_kernel::{Cfg, Context, Fiber, Inject, Kernel, NoConfig, Plugin, Service};
use hoh_llm::{
    BuiltinLlmFailureCode, GenerateOptions, Llm, LlmAdapter, LlmError, LlmErrorOptions, LlmRuntime,
    LlmStream, StreamChunk,
};
use hoh_session::{SessionId, SessionStore, Sessions};
use hoh_session_projection::SessionProjectionRegistry;
use hoh_system_prompt::SystemPrompt;
use hoh_tool_executor_native::NativeToolExecutor;
use hoh_tools::{ToolRuntime, Tools};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard},
};

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn chunk(value: Value) -> StreamChunk {
    serde_json::from_value(value).expect("stream chunk")
}

/// `textResponse(text)`: one text block, then stop.
pub fn text_response(text: &str) -> Entry {
    Entry::Chunks(vec![
        chunk(json!({"type":"block-start","index":0,"blockType":"text"})),
        chunk(json!({"type":"block-end","index":0,"block":{"type":"text","text":text}})),
        chunk(json!({"type":"finish","reason":{"kind":"stop"}})),
    ])
}

/// `offloadRequired(n)`: the adapter throws `IMAGE_OFFLOAD_REQUIRED` with `offloadImages: n`.
pub fn offload_required(offload_images: u64) -> Entry {
    Entry::Fail(
        LlmError::with_options(
            "request images exceed the route budget",
            BuiltinLlmFailureCode::ImageOffloadRequired,
            LlmErrorOptions {
                offload_images: Some(offload_images),
                ..Default::default()
            },
        )
        .expect("llm error"),
    )
}

/// `() => { throw new LlmError('provider outage', 'SERVER') }`.
pub fn server_failure() -> Entry {
    Entry::Fail(LlmError::new("provider outage", BuiltinLlmFailureCode::Server).expect("llm error"))
}

pub enum Entry {
    Chunks(Vec<StreamChunk>),
    /// The generator throws when first iterated.
    Fail(HarnessError),
}

/// Replies one scripted stream per request and declares no retry policy.
pub struct ScriptedAdapter {
    script: Mutex<VecDeque<Entry>>,
    /// Each request as JSON (`GenerateOptions`), in call order.
    pub requests: Mutex<Vec<Value>>,
}

impl ScriptedAdapter {
    pub fn new(script: Vec<Entry>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into()),
            requests: Mutex::new(Vec::new()),
        })
    }
    pub fn request_count(&self) -> usize {
        lock(&self.requests).len()
    }
    pub fn request(&self, index: usize) -> Value {
        lock(&self.requests)[index].clone()
    }
    /// `adapter.requests.map(offloadedNames)`.
    pub fn offloaded_names_per_request(&self) -> Vec<Vec<String>> {
        lock(&self.requests).iter().map(offloaded_names).collect()
    }
}

impl LlmAdapter for ScriptedAdapter {
    fn stream(&self, options: GenerateOptions) -> Result<LlmStream, BoxError> {
        lock(&self.requests).push(serde_json::to_value(&options)?);
        match lock(&self.script).pop_front() {
            None => Ok(LlmStream::new(stream::once(async {
                Err::<StreamChunk, BoxError>("script exhausted".into())
            }))),
            Some(Entry::Chunks(chunks)) => Ok(LlmStream::chunks(chunks)),
            Some(Entry::Fail(error)) => Ok(LlmStream::new(stream::once(async move {
                Err::<StreamChunk, BoxError>(Box::new(error))
            }))),
        }
    }
}

/// `offloadedNames(options)`: names of the images the request sends as offloaded.
pub fn offloaded_names(options: &Value) -> Vec<String> {
    let mut names = vec![];
    for message in options["messages"].as_array().expect("messages") {
        for block in message["content"].as_array().expect("content") {
            if block["type"] == "image" && block["offloaded"] == true {
                names.push(
                    block["attachment"]["name"]
                        .as_str()
                        .unwrap_or("")
                        .to_owned(),
                );
            }
        }
    }
    names
}

#[derive(Inject)]
struct NativeDeps {
    tools: Service<Tools>,
}

/// The native executor of `@deepseek-ai/dsh-tool-executor-native` with its default limit.
struct NativeExecutorPlugin;

impl Plugin for NativeExecutorPlugin {
    const NAME: &'static str = "native-tool-executor";
    type Config = NoConfig;
    type Deps = NativeDeps;
    async fn apply(ctx: Context, deps: NativeDeps, _config: Cfg<NoConfig>) -> Result<(), BoxError> {
        deps.tools.set_executor(
            &ctx,
            Arc::new(NativeToolExecutor::with_max_parallel(ctx.clone(), || 10)?),
        )?;
        Ok(())
    }
}

async fn mount<P: Plugin>(ctx: &Context, config: Value) -> Fiber {
    let fiber = ctx.plugin::<P>(config).expect("plugin mounts");
    fiber.settled().await.expect("plugin settles");
    fiber
}

pub struct Env {
    pub kernel: Kernel,
    pub ctx: Context,
    pub adapter: Arc<ScriptedAdapter>,
    _registration: Option<hoh_llm::AdapterRegistrationHandle>,
}

impl Env {
    /// `mountAgentLoopTestDependencies(ctx)`: every service the loop needs except the loop itself.
    pub async fn dependencies(adapter: Arc<ScriptedAdapter>) -> Env {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.provide::<Llm>(Arc::new(LlmRuntime::with_context(&ctx)))
            .await
            .expect("llm provides");
        ctx.provide::<Sessions>(Arc::new(SessionStore::new()))
            .await
            .expect("sessions provide");
        mount::<SessionProjectionRegistry>(&ctx, json!({})).await;
        mount::<SystemPrompt>(&ctx, json!({})).await;
        mount::<ToolRuntime>(&ctx, json!({})).await;
        mount::<NativeExecutorPlugin>(&ctx, json!({})).await;
        mount::<AgentRegistryPlugin>(&ctx, json!({})).await;
        Env {
            kernel,
            ctx,
            adapter,
            _registration: None,
        }
    }

    /// `ctx.llm.registerAdapter(['mock'], adapter)`.
    pub fn register_adapter(&mut self) {
        self._registration = Some(
            self.ctx
                .get::<Llm>()
                .expect("llm service")
                .register_adapter(&self.ctx, vec!["mock".into()], self.adapter.clone())
                .expect("adapter registers"),
        );
    }

    /// `ctx.plugin(offload)`.
    pub async fn mount_offload(&self) -> Fiber {
        mount::<ImageOffloadPlugin>(&self.ctx, json!({})).await
    }

    /// `ctx.plugin(AgentLoop, { agents: [] })`.
    pub async fn mount_agent_loop(&self) {
        mount::<AgentLoopPlugin>(&self.ctx, json!({ "agents": [] })).await;
    }

    /// `ctx.agentLoop.create(id, { provider: 'mock', model: 'mock' })`.
    pub async fn create_agent(&self, id: &str) -> Arc<dyn Agent> {
        let agent_loop = self
            .ctx
            .get::<hoh_agent_loop::AgentLoopKey>()
            .expect("agent loop service");
        agent_loop
            .create(
                SessionId::from(id),
                AgentOptions {
                    provider: Some(Live::Value("mock".into())),
                    model: Some(Live::Value("mock".into())),
                    ..AgentOptions::default()
                },
                hoh_agent::AgentSessionMeta::default(),
            )
            .await
            .expect("agent creates")
    }
}

/// `harness(adapter)` of the source: dependencies, the offload plugin, the loop, the adapter.
pub async fn harness(script: Vec<Entry>) -> Env {
    let mut env = Env::dependencies(ScriptedAdapter::new(script)).await;
    env.mount_offload().await;
    env.mount_agent_loop().await;
    env.register_adapter();
    env
}

/// `decisions(session)`: the `image/offload` events.
pub fn decisions(session: &hoh_session::Session) -> Vec<Arc<hoh_session::SessionEvent>> {
    session
        .snapshot_events(None, None)
        .iter()
        .filter(|event| event.event_type() == "image/offload")
        .cloned()
        .collect()
}

/// `replacements(session)`: surface replacements as `[original seq, replacement seq]` pairs.
pub fn replacements(session: &hoh_session::Session) -> Vec<(u64, u64)> {
    session
        .snapshot_events(None, None)
        .iter()
        .filter(|event| hoh_session::is_replacement_surface_event(event))
        .map(|event| {
            (
                event.source_event_seqs().expect("sourceEventSeqs")[0]
                    .as_u64()
                    .expect("seq"),
                u64::from(event.seq()),
            )
        })
        .collect()
}

pub fn event_types(session: &hoh_session::Session) -> Vec<String> {
    session
        .snapshot_events(None, None)
        .iter()
        .map(|event| event.event_type().to_owned())
        .collect()
}
