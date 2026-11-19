#![allow(dead_code)]
use futures_util::future::BoxFuture;
use hoh_agent::{Agent, AgentRegistryPlugin, Agents};
use hoh_agent_loop::AgentLoopPlugin;
use hoh_error::BoxError;
use hoh_kernel::{Context, Kernel, Plugin};
use hoh_llm::{GenerateOptions, Llm, LlmAdapter, LlmRuntime, LlmStream, StreamChunk};
use hoh_protocol::InitializeParams;
use hoh_sdk_jsonrpc_server::{
    HarnessSdkJsonRpcServer, HarnessSdkJsonRpcServerOptions, transport::JsonRpcTransportPeer,
};
use hoh_session::{SessionStore, Sessions};
use hoh_tools::{ToolBatchInput, ToolBatchResult, ToolDefinition, ToolExecutor, ToolModelInput};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard},
};
use tempfile::TempDir;

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
pub async fn mount<P: Plugin>(ctx: &Context, config: Value) -> hoh_kernel::Fiber {
    let fiber = ctx.plugin::<P>(config).unwrap();
    fiber.settled().await.unwrap();
    fiber
}
pub type Host = Arc<
    dyn Fn(
            String,
            Value,
            Option<hoh_timeout::AbortSignal>,
        ) -> BoxFuture<'static, Result<Value, BoxError>>
        + Send
        + Sync,
>;
#[derive(Default)]
pub struct Peer {
    pub notices: Mutex<Vec<(String, Value)>>,
    pub requests: Mutex<Vec<(String, Value)>>,
    pub host: Mutex<Option<Host>>,
    pub notify_failure: Mutex<Option<String>>,
}
impl JsonRpcTransportPeer for Peer {
    fn request(
        &self,
        method: &str,
        params: Value,
        signal: Option<hoh_timeout::AbortSignal>,
    ) -> BoxFuture<'static, Result<Value, BoxError>> {
        lock(&self.requests).push((method.into(), params.clone()));
        match lock(&self.host).clone() {
            Some(host) => host(method.into(), params, signal),
            None => Box::pin(async { Err("unexpected host request".into()) }),
        }
    }
    fn notify(&self, method: &str, params: Option<Value>) -> Result<(), BoxError> {
        if let Some(failure) = lock(&self.notify_failure).as_ref() {
            return Err(failure.clone().into());
        }
        lock(&self.notices).push((method.into(), params.unwrap_or(Value::Null)));
        Ok(())
    }
}
impl Peer {
    pub fn events(&self, method: &str) -> Vec<Value> {
        lock(&self.notices)
            .iter()
            .filter(|(m, _)| m == method)
            .map(|(_, p)| p.clone())
            .collect()
    }
    pub fn answer(&self, value: Value) {
        *lock(&self.host) = Some(Arc::new(move |_, _, _| {
            let value = value.clone();
            Box::pin(async move { Ok(value) })
        }));
    }
}
#[derive(Default)]
pub struct Replay {
    pub requests: Mutex<Vec<GenerateOptions>>,
    pub texts: Mutex<VecDeque<String>>,
    pub reasoning: Mutex<Option<hoh_llm::LlmModelReasoningInfo>>,
    pub gate: Mutex<Option<Arc<tokio::sync::Semaphore>>>,
    pub scripted_chunks: Mutex<Option<Vec<StreamChunk>>>,
    /// Pause immediately before the zero-based scripted chunk at this position.
    pub pause_before: Mutex<Option<(usize, Arc<tokio::sync::Semaphore>)>>,
    pub entered: Arc<tokio::sync::Notify>,
}
impl LlmAdapter for Replay {
    fn resolve_model<'a>(
        &'a self,
        provider: &'a str,
        model: &'a str,
        _: Option<&'a hoh_timeout::AbortSignal>,
    ) -> hoh_llm::LlmFuture<'a, hoh_llm::LlmResolvedModelInfo> {
        Box::pin(async move {
            Ok(hoh_llm::LlmResolvedModelInfo {
                model: hoh_llm::LlmModelInfo {
                    provider: provider.into(),
                    id: model.into(),
                    name: model.into(),
                    ..Default::default()
                },
                reasoning: lock(&self.reasoning).clone(),
                ..Default::default()
            })
        })
    }

    fn stream(&self, options: GenerateOptions) -> Result<LlmStream, BoxError> {
        let signal = options.signal.clone();
        lock(&self.requests).push(options);
        let text = lock(&self.texts)
            .pop_front()
            .unwrap_or_else(|| "answer".into());
        let mut chunks = lock(&self.scripted_chunks).take().unwrap_or_else(|| {
            [
                json!({"type":"block-start","index":0,"blockType":"text"}),
                json!({"type":"text-delta","index":0,"text":text}),
                json!({"type":"block-end","index":0,"block":{"type":"text","text":text}}),
                json!({"type":"finish","reason":{"kind":"stop"}}),
            ]
            .into_iter()
            .map(|v| serde_json::from_value::<StreamChunk>(v).unwrap())
            .collect::<Vec<_>>()
        });
        if let Some((index, gate)) = lock(&self.pause_before).take() {
            use futures_util::{StreamExt, stream};
            assert!(
                index < chunks.len(),
                "pause position outside scripted stream"
            );
            let mut tail = chunks.split_off(index);
            let first = tail.remove(0);
            let entered = self.entered.clone();
            let delayed_signal = signal.clone();
            let delayed = stream::once(async move {
                entered.notify_one();
                if let Some(signal) = delayed_signal {
                    tokio::select! {p=gate.acquire()=>{p.unwrap().forget();},reason=signal.cancelled()=>return Err(Box::new(reason) as BoxError)}
                } else {
                    gate.acquire().await.unwrap().forget();
                }
                Ok(first)
            });
            return Ok(LlmStream::new(
                stream::iter(chunks.into_iter().map(Ok))
                    .chain(delayed)
                    .chain(stream::iter(tail.into_iter().map(Ok))),
            ));
        }
        if let Some(gate) = lock(&self.gate).take() {
            use futures_util::{StreamExt, stream};
            let first = chunks.remove(0);
            let entered = self.entered.clone();
            let head = stream::once(async move {
                entered.notify_one();
                if let Some(signal) = signal {
                    tokio::select! {p=gate.acquire()=>{p.unwrap().forget();},reason=signal.cancelled()=>return Err(Box::new(reason) as BoxError)}
                } else {
                    gate.acquire().await.unwrap().forget();
                }
                Ok(first)
            });
            return Ok(LlmStream::new(
                head.chain(stream::iter(chunks.into_iter().map(Ok))),
            ));
        }
        Ok(LlmStream::chunks(chunks))
    }
}

/// The window profile installs its native executor alongside the tool runtime. These server tests
/// have no tool definitions or tool calls, but prompt assembly still requires that composition
/// dependency to exist.
struct EmptyToolExecutor;

impl ToolExecutor for EmptyToolExecutor {
    fn describe(&self, _: &[Arc<ToolDefinition>]) -> Result<ToolModelInput, BoxError> {
        Ok(ToolModelInput::default())
    }

    fn execute(&self, _: ToolBatchInput) -> BoxFuture<'_, Result<ToolBatchResult, BoxError>> {
        Box::pin(async { Ok(ToolBatchResult::default()) })
    }
}

pub struct Env {
    pub kernel: Kernel,
    pub ctx: Context,
    pub dir: TempDir,
    pub peer: Arc<Peer>,
    pub model: Arc<Replay>,
    pub server: HarnessSdkJsonRpcServer,
    pub loop_fiber: hoh_kernel::Fiber,
    _tool_executor: hoh_kernel::Disposer,
}
pub async fn base(
    dir: &TempDir,
    persistence: Option<&std::path::Path>,
) -> (
    Kernel,
    Context,
    Arc<Replay>,
    hoh_kernel::Fiber,
    hoh_kernel::Disposer,
) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.provide::<Llm>(Arc::new(LlmRuntime::with_context(&ctx)))
        .await
        .unwrap();
    ctx.provide::<Sessions>(Arc::new(SessionStore::new()))
        .await
        .unwrap();
    mount::<hoh_session_projection::SessionProjectionRegistry>(&ctx, json!({})).await;
    mount::<hoh_system_prompt::SystemPrompt>(&ctx, json!({})).await;
    mount::<hoh_tools::ToolRuntime>(&ctx, json!({})).await;
    let tool_executor = ctx
        .get::<hoh_tools::Tools>()
        .unwrap()
        .set_executor(&ctx, Arc::new(EmptyToolExecutor))
        .unwrap();
    mount::<AgentRegistryPlugin>(&ctx, json!({})).await;
    mount::<hoh_fs_local::FsLocalPlugin>(&ctx, json!({"cwd":dir.path()})).await;
    ctx.provide::<hoh_working_directory::shim::SessionProjectionsKey>(Arc::new(
        hoh_working_directory::shim::SessionProjections,
    ))
    .await
    .unwrap();
    mount::<hoh_working_directory::WorkingDirectory>(&ctx, json!({"defaultDirectory":dir.path()}))
        .await;
    if let Some(root) = persistence {
        mount::<hoh_session_persistence_jsonl::JsonlSessionPersistencePlugin>(
            &ctx,
            json!({"root":root, "compression":"none"}),
        )
        .await;
    }
    let loop_fiber = mount::<AgentLoopPlugin>(&ctx, json!({"agents":[]})).await;
    let model = Arc::new(Replay::default());
    ctx.get::<Llm>()
        .unwrap()
        .register_adapter(&ctx, vec!["mock".into()], model.clone())
        .unwrap();
    (kernel, ctx, model, loop_fiber, tool_executor)
}
impl Env {
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        Self::at(dir, None).await
    }
    pub async fn persistent(root: &std::path::Path) -> Self {
        Self::at(tempfile::tempdir().unwrap(), Some(root)).await
    }
    async fn at(dir: TempDir, persistence: Option<&std::path::Path>) -> Self {
        let (kernel, ctx, model, loop_fiber, tool_executor) = base(&dir, persistence).await;
        let peer = Arc::new(Peer::default());
        let server = HarnessSdkJsonRpcServer::new(
            ctx.clone(),
            peer.clone(),
            HarnessSdkJsonRpcServerOptions::default(),
        )
        .unwrap();
        Self {
            kernel,
            ctx,
            dir,
            peer,
            model,
            server,
            loop_fiber,
            _tool_executor: tool_executor,
        }
    }
    pub async fn initialize(&self) {
        self.server.initialize(self.params()).await.unwrap();
    }
    pub fn params(&self) -> InitializeParams {
        serde_json::from_value(json!({"cwd":self.dir.path(), "provider":"mock", "model":"mock"}))
            .unwrap()
    }
    pub async fn open(&self, id: &str) -> Arc<dyn Agent> {
        self.server
            .handle_request("session/working-directory/get", json!({"sessionId":id}))
            .await
            .unwrap();
        self.agent(id)
    }
    pub fn agent(&self, id: &str) -> Arc<dyn Agent> {
        self.ctx.get::<Agents>().unwrap().get(&id.into()).unwrap()
    }
    pub async fn prompt(&self, id: &str, text: &str) -> Value {
        self.server
            .handle_request(
                "session/prompt",
                json!({"sessionId":id, "contentBlocks":[{"type":"text", "text":text}]}),
            )
            .await
            .unwrap()
    }
    pub async fn query(&self, id: &str, text: &str) -> Result<Value, BoxError> {
        self.server
            .handle_request(
                "session/query",
                json!({"sessionId":id, "contentBlocks":[{"type":"text", "text":text}]}),
            )
            .await
    }
    pub async fn wait(&self, id: &str) {
        self.server
            .handle_request("session/wait", json!({"sessionId":id}))
            .await
            .unwrap();
    }
    pub async fn close(self) {
        self.server.shutdown().await.unwrap();
        self.kernel.shutdown().await;
    }
}
pub fn types(agent: &Arc<dyn Agent>) -> Vec<String> {
    agent
        .session()
        .snapshot_events(None, None)
        .iter()
        .map(|e| e.event_type().into())
        .collect()
}
pub async fn until(predicate: impl Fn() -> bool) {
    for _ in 0..10000 {
        if predicate() {
            return;
        }
        tokio::task::yield_now().await;
    }
    assert!(predicate(), "condition never held");
}

pub mod commands {
    use super::*;
    use hoh_commands::{
        CommandDefinition, CommandInputDescriptor, CommandResult, CommandRuntime, Commands,
    };
    use hoh_sdk_jsonrpc_server::command_helpers::BuiltinCommandName;
    use hoh_timeout::AbortSignal;
    pub type Handler = Arc<
        dyn Fn(
                Arc<dyn Agent>,
                String,
                Vec<hoh_llm::ContentBlock>,
                AbortSignal,
            ) -> BoxFuture<'static, Result<Option<CommandResult>, BoxError>>
            + Send
            + Sync,
    >;
    pub type Calls = Arc<Mutex<Vec<(String, Vec<hoh_llm::ContentBlock>)>>>;
    pub struct Registry {
        runtime: Arc<CommandRuntime>,
        ctx: Context,
        registrations: Mutex<Vec<hoh_kernel::Disposer>>,
        pub calls: Calls,
        pub handler: Arc<Mutex<Handler>>,
    }
    impl Registry {
        pub async fn mount(h: &Env) -> Arc<Self> {
            Self::mount_on(&h.ctx).await
        }
        pub async fn mount_on(ctx: &Context) -> Arc<Self> {
            let runtime = Arc::new(CommandRuntime::new(ctx.clone()).unwrap());
            let this = Arc::new(Self {
                runtime: runtime.clone(),
                ctx: ctx.clone(),
                registrations: Mutex::new(Vec::new()),
                calls: Arc::new(Mutex::new(Vec::new())),
                handler: Arc::new(Mutex::new(Arc::new(|_, _, _, _| {
                    Box::pin(async {
                        Ok(Some(CommandResult::Success {
                            text: Some("native outcome".into()),
                            source_event_seq: None,
                        }))
                    })
                }))),
            });
            ctx.provide::<Commands>(runtime).await.unwrap();
            this
        }
        pub fn register(&self, name: &str, id: Option<BuiltinCommandName>) {
            self.register_with_definition_id(name, id.map(|id| id.definition_id().into()));
        }
        pub fn register_with_definition_id(
            &self,
            name: &str,
            definition_id: Option<hoh_commands::CommandDefinitionId>,
        ) {
            let registered_name = name.to_owned();
            let line_name = registered_name.clone();
            let handler = self.handler.clone();
            let calls = self.calls.clone();
            let definition = CommandDefinition {
                definition_id,
                name: registered_name,
                description: "Fixture command".into(),
                input: Some(
                    CommandInputDescriptor::new("Fixture command input").with_attachments(),
                ),
                record_input: None,
                handler: Arc::new(move |invocation| {
                    let handler = lock(&handler).clone();
                    let calls = calls.clone();
                    let line = format!("/{line_name}{}", invocation.raw_input);
                    let raw_input = invocation.raw_input;
                    let attachments = invocation.attachments.to_vec();
                    let agent = invocation.agent;
                    let signal = invocation.signal;
                    Box::pin(async move {
                        lock(&calls).push((line, attachments.clone()));
                        handler(agent, raw_input, attachments, signal)
                            .await?
                            .ok_or_else(|| {
                                "The selected command is unavailable in this SDK profile".into()
                            })
                    })
                }),
            };
            lock(&self.registrations).push(
                self.runtime
                    .register(&self.ctx, definition)
                    .expect("fixture command registers"),
            );
        }
        pub fn handler(&self, handler: Handler) {
            *lock(&self.handler) = handler;
        }
    }
}

pub mod images {
    use super::*;
    use hoh_attachment::*;
    pub struct Store {
        limits: ImageAttachmentLimits,
        pub saved: Mutex<Vec<Vec<SaveImageAttachment>>>,
        pub gate: Mutex<Option<Arc<tokio::sync::Semaphore>>>,
        pub entered: tokio::sync::Notify,
    }
    impl Store {
        pub async fn mount(h: &Env) -> Arc<Self> {
            let store = Arc::new(Self {
                limits: ImageAttachmentLimits {
                    max_image_bytes: 1024,
                    max_images_per_message: 10,
                    max_message_image_bytes: 10240,
                    max_image_pixels: 1024,
                    max_image_dimension: 1024,
                    media_types: vec![
                        ImageMediaType::Png,
                        ImageMediaType::Jpeg,
                        ImageMediaType::Webp,
                        ImageMediaType::Gif,
                    ],
                },
                saved: Mutex::new(Vec::new()),
                gate: Mutex::new(None),
                entered: tokio::sync::Notify::new(),
            });
            h.ctx.provide::<Attachments>(store.clone()).await.unwrap();
            store
        }
    }
    impl AttachmentStore for Store {
        fn image_limits(&self) -> &ImageAttachmentLimits {
            &self.limits
        }
        fn validate_image<'a>(
            &'a self,
            _: &'a SaveImageAttachment,
        ) -> BoxFuture<'a, Result<(), BoxError>> {
            Box::pin(async { Ok(()) })
        }
        fn save_images<'a>(
            &'a self,
            inputs: &'a [SaveImageAttachment],
        ) -> BoxFuture<'a, Result<Vec<ImageAttachmentRef>, BoxError>> {
            Box::pin(async move {
                lock(&self.saved).push(inputs.to_vec());
                let gate = lock(&self.gate).take();
                self.entered.notify_one();
                if let Some(gate) = gate {
                    gate.acquire().await.unwrap().forget();
                }
                Ok(inputs
                    .iter()
                    .enumerate()
                    .map(|(i, image)| ImageAttachmentRef {
                        attachment_id: format!("sha256:image-{i}").into(),
                        media_type: image.media_type,
                        bytes: image.data.len() as u64,
                        width: 1,
                        height: 1,
                        name: image.name.clone(),
                        original_dimensions: None,
                    })
                    .collect())
            })
        }
        fn save_image<'a>(
            &'a self,
            input: &'a SaveImageAttachment,
        ) -> BoxFuture<'a, Result<ImageAttachmentRef, BoxError>> {
            Box::pin(async move {
                Ok(self
                    .save_images(std::slice::from_ref(input))
                    .await?
                    .remove(0))
            })
        }
        fn read_image<'a>(
            &'a self,
            reference: &'a ImageAttachmentRef,
            _: Option<&'a hoh_timeout::AbortSignal>,
        ) -> BoxFuture<'a, Result<StoredImageAttachment, BoxError>> {
            Box::pin(async move {
                Ok(StoredImageAttachment {
                    r#ref: reference.clone(),
                    data: vec![1],
                })
            })
        }
    }
}
