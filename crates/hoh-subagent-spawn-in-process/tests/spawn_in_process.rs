use futures_util::{StreamExt, stream};
use hoh_agent::{
    Agent, AgentCreated, AgentOptions, AgentSessionMeta, Agents, CreateAgentOptions, Live,
};
use hoh_agent_loop_testkit::{
    AgentLoopTestDependenciesOptions, mount_agent_loop_test_dependencies,
    mount_agent_loop_test_harness,
};
use hoh_error::{BoxError, HarnessError, SharedError};
use hoh_kernel::{Context, Kernel};
use hoh_llm::{
    AdapterRegistrationHandle, ContentBlock, GenerateOptions, Llm, LlmAdapter, LlmStream,
    StreamChunk,
};
use hoh_session::{SessionCreated, Sessions};
use hoh_subagent::{
    ContinuableCreateRequest, STRUCTURED_OUTPUT_TOOL, SubagentActivation, SubagentActivationSpec,
    SubagentCapabilities, SubagentDeliveryTarget, SubagentError, SubagentPlugin, SubagentProvider,
    SubagentRunEndInfo, SubagentRunInfo, SubagentStartRequest, SubagentStopReason, Subagents,
};
use hoh_subagent_spawn_in_process::{
    Config, INJECT, NAME, SpawnInProcessProvider, SubagentSpawnInProcess,
};
use hoh_timeout::{AbortController, AbortSignal};
use hoh_tools::Tools;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard},
};
use tokio::sync::Notify;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn chunk(value: Value) -> StreamChunk {
    serde_json::from_value(value).expect("stream chunk")
}

fn text_response(text: &str) -> Vec<StreamChunk> {
    vec![
        chunk(json!({"type":"block-start","index":0,"blockType":"text"})),
        chunk(json!({"type":"text-delta","index":0,"text":text})),
        chunk(json!({"type":"block-end","index":0,"block":{"type":"text","text":text}})),
        chunk(json!({"type":"usage","usage":{"inputTokens":1,"outputTokens":1}})),
        chunk(json!({"type":"finish","reason":{"kind":"stop"}})),
    ]
}

fn max_tokens_response(text: &str) -> Vec<StreamChunk> {
    let mut chunks = text_response(text);
    chunks.pop();
    chunks.push(chunk(
        json!({"type":"finish","reason":{"kind":"max-tokens"}}),
    ));
    chunks
}

fn tool_call_response(id: &str, name: &str, arguments: Value) -> Vec<StreamChunk> {
    let arguments = arguments.to_string();
    vec![
        chunk(json!({"type":"block-start","index":0,"blockType":"tool-call"})),
        chunk(
            json!({"type":"tool-call-delta","index":0,"id":id,"name":name,"argumentsDelta":arguments}),
        ),
        chunk(
            json!({"type":"block-end","index":0,"block":{"type":"tool-call","id":id,"name":name,"arguments":arguments}}),
        ),
        chunk(json!({"type":"usage","usage":{"inputTokens":1,"outputTokens":1}})),
        chunk(json!({"type":"finish","reason":{"kind":"tool-calls"}})),
    ]
}

enum Entry {
    Chunks(Vec<StreamChunk>),
    Gated(Vec<StreamChunk>, Arc<Notify>),
    Hang,
}

struct ScriptedAdapter {
    script: Mutex<VecDeque<Entry>>,
    requests: Mutex<Vec<GenerateOptions>>,
}

impl ScriptedAdapter {
    fn new(script: Vec<Entry>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into()),
            requests: Mutex::new(Vec::new()),
        })
    }
}

impl LlmAdapter for ScriptedAdapter {
    fn stream(&self, options: GenerateOptions) -> Result<LlmStream, BoxError> {
        let signal = options.signal.clone();
        lock(&self.requests).push(options);
        match lock(&self.script).pop_front() {
            Some(Entry::Chunks(chunks)) => Ok(LlmStream::chunks(chunks)),
            Some(Entry::Gated(chunks, release)) => {
                let gated = stream::once(async move {
                    release.notified().await;
                    chunks
                })
                .flat_map(|chunks| stream::iter(chunks.into_iter().map(Ok)));
                Ok(LlmStream::new(gated))
            }
            Some(Entry::Hang) => {
                let head = stream::iter(text_response("partial").into_iter().take(2).map(Ok));
                let tail = stream::once(async move {
                    if let Some(signal) = signal {
                        signal.cancelled().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                    Err::<StreamChunk, BoxError>("aborted".into())
                });
                Ok(LlmStream::new(head.chain(tail)))
            }
            None => Ok(LlmStream::new(stream::once(async {
                Err::<StreamChunk, BoxError>("script exhausted".into())
            }))),
        }
    }
}

struct Harness {
    kernel: Kernel,
    ctx: Context,
    adapter: Arc<ScriptedAdapter>,
    parent: Arc<dyn Agent>,
    spawn_fiber: hoh_kernel::Fiber,
    _registration: AdapterRegistrationHandle,
}

impl Harness {
    async fn new(script: Vec<Entry>) -> Self {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        mount_agent_loop_test_dependencies(
            &ctx,
            AgentLoopTestDependenciesOptions {
                working_directory: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let loop_harness = mount_agent_loop_test_harness(&ctx).await.unwrap();
        let subagents = ctx.plugin::<SubagentPlugin>(json!({})).unwrap();
        subagents.settled().await.unwrap();
        let spawn_fiber = ctx
            .plugin::<SubagentSpawnInProcess>(json!({"providerName":"spawn"}))
            .unwrap();
        spawn_fiber.settled().await.unwrap();
        let adapter = ScriptedAdapter::new(script);
        let registration = ctx
            .get::<Llm>()
            .unwrap()
            .register_adapter(&ctx, vec!["mock".into()], adapter.clone())
            .unwrap();
        let parent = loop_harness
            .create_with(
                "parent".into(),
                AgentOptions {
                    provider: Some(Live::Value("mock".into())),
                    model: Some(Live::Value("mock".into())),
                    ..Default::default()
                },
                AgentSessionMeta {
                    cwd: Some("/tmp".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        Self {
            kernel,
            ctx,
            adapter,
            parent,
            spawn_fiber,
            _registration: registration,
        }
    }

    fn runtime(&self) -> hoh_kernel::Service<Subagents> {
        self.ctx.get::<Subagents>().unwrap()
    }

    fn agents(&self) -> hoh_kernel::Service<Agents> {
        self.ctx.get::<Agents>().unwrap()
    }

    fn spec(
        &self,
        signal: AbortSignal,
        max_depth: Option<u64>,
        options: Option<AgentOptions>,
        output_schema: Option<Value>,
    ) -> SubagentActivationSpec {
        SubagentActivationSpec {
            provider: "spawn".into(),
            label: "test child".into(),
            name: None,
            child_id: None,
            delivery: SubagentDeliveryTarget::Caller,
            request: SubagentStartRequest {
                cwd: None,
                agent_preset: None,
                label: None,
                prompt: vec![ContentBlock::text("do X")],
                parent: self.parent.clone(),
                signal,
                agent_options: options,
                output_schema,
                max_depth,
            },
        }
    }

    async fn start(&self) -> Result<SubagentActivation, BoxError> {
        self.runtime()
            .start_activation(self.spec(AbortController::new().signal(), None, None, None))
            .await
    }

    async fn close(self) {
        self.kernel.shutdown().await;
    }
}

fn output_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(ContentBlock::text_value)
        .collect::<Vec<_>>()
        .join("")
}

fn observe_creation_events(ctx: &Context) -> Arc<Mutex<Vec<&'static str>>> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let output = events.clone();
    ctx.on_contained::<SessionCreated>(move |_| {
        lock(&output).push("session/created");
        Ok(())
    })
    .unwrap();
    let output = events.clone();
    ctx.on_serial::<AgentCreated, (), _>(move |_| {
        let output = output.clone();
        async move {
            lock(&output).push("agent/created");
            Ok(None)
        }
    })
    .unwrap();
    events
}

#[tokio::test]
async fn runs_a_fresh_child_to_completion_and_returns_its_final_assistant_output() {
    let h = Harness::new(vec![Entry::Chunks(text_response("child answer"))]).await;
    let run = h.start().await.unwrap();
    let result = run.result.await.unwrap();
    assert_eq!(result.stop_reason, SubagentStopReason::Completed);
    assert_eq!(output_text(&result.output), "child answer");
    (run.dispose)().await.unwrap();
    h.close().await;
}

#[tokio::test]
async fn emits_subagent_start_only_after_the_fresh_child_is_published() {
    let h = Harness::new(vec![Entry::Chunks(text_response("child answer"))]).await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let output = seen.clone();
    let agents = h.agents();
    h.ctx
        .on_contained::<SubagentRunInfo>(move |info| {
            lock(&output).push((info.id.clone(), agents.get(&info.id).is_some()));
            Ok(())
        })
        .unwrap();
    let run = h.start().await.unwrap();
    assert_eq!(*lock(&seen), [(run.child_id.clone(), true)]);
    run.result.await.unwrap();
    (run.dispose)().await.unwrap();
    h.close().await;
}

#[tokio::test]
async fn gives_the_child_its_own_session_with_parent_session_lineage() {
    let h = Harness::new(vec![Entry::Chunks(text_response("hi"))]).await;
    let run = h.start().await.unwrap();
    let child = h.agents().get(&run.child_id).unwrap();
    assert_ne!(child.session().id(), h.parent.session().id());
    assert_eq!(
        child.session().header()["parentSession"],
        h.parent.session().id().as_ref()
    );
    run.result.await.unwrap();
    (run.dispose)().await.unwrap();
    h.close().await;
}

#[tokio::test]
async fn a_fresh_child_does_not_inherit_the_parent_conversation() {
    let h = Harness::new(vec![
        Entry::Chunks(text_response("parent turn")),
        Entry::Chunks(text_response("child sees nothing")),
    ])
    .await;
    h.parent
        .followup(hoh_llm::create_user_message(hoh_llm::NewUserMessage {
            content: vec![ContentBlock::text("parent prompt")],
            source: hoh_llm::MessageSource::new("user"),
        }))
        .unwrap();
    h.parent.when_idle().await;
    assert!(!h.parent.session().snapshot_events(None, None).is_empty());
    let run = h.start().await.unwrap();
    let child = h.agents().get(&run.child_id).unwrap();
    run.result.await.unwrap();
    let users = child
        .session()
        .snapshot_events(None, None)
        .iter()
        .filter(|event| event.event_type() == "user/message")
        .map(|event| event.data().clone())
        .collect::<Vec<_>>();
    assert_eq!(users[0]["content"][0]["text"], "do X");
    assert!(users.iter().all(|message| {
        message["content"]
            .as_array()
            .is_none_or(|content| content.iter().all(|block| block["text"] != "parent prompt"))
    }));
    (run.dispose)().await.unwrap();
    h.close().await;
}

#[tokio::test]
async fn releases_the_child_before_delivering_its_result() {
    let h = Harness::new(vec![Entry::Chunks(text_response("x"))]).await;
    let run = h.start().await.unwrap();
    assert!(h.agents().get(&run.child_id).is_some());
    run.result.await.unwrap();
    assert!(h.agents().get(&run.child_id).is_none());
    (run.dispose)().await.unwrap();
    h.close().await;
}

#[tokio::test]
async fn stamps_child_depth_as_parent_depth_plus_one() {
    let h = Harness::new(vec![Entry::Chunks(text_response("x"))]).await;
    assert!(h.parent.session().header().get("delegationDepth").is_none());
    let run = h.start().await.unwrap();
    let child = h.agents().get(&run.child_id).unwrap();
    assert_eq!(child.session().header()["delegationDepth"], 1);
    run.result.await.unwrap();
    (run.dispose)().await.unwrap();
    h.close().await;
}

#[tokio::test]
async fn refuses_to_spawn_past_max_depth() {
    let h = Harness::new(vec![]).await;
    let error = match h
        .runtime()
        .start_activation(h.spec(AbortController::new().signal(), Some(0), None, None))
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("depth-limited start unexpectedly succeeded"),
    };
    let routed = HarnessError::of(error.as_ref()).expect("depth failure must be machine-routable");
    assert_eq!(routed.name(), SubagentError::NAME);
    assert_eq!(routed.code(), "DEPTH_EXCEEDED");
    assert_eq!(routed.message(), "subagent depth 1 exceeds maxDepth 0");
    assert!(
        h.agents()
            .list()
            .iter()
            .all(|agent| agent.id() == h.parent.id())
    );
    h.close().await;
}

#[tokio::test]
async fn maps_a_child_that_hit_its_token_ceiling_to_max_tokens() {
    let h = Harness::new(vec![Entry::Chunks(max_tokens_response("cut off"))]).await;
    let run = h.start().await.unwrap();
    let result = run.result.await.unwrap();
    assert_eq!(result.stop_reason, SubagentStopReason::MaxTokens);
    (run.dispose)().await.unwrap();
    h.close().await;
}

#[tokio::test]
async fn maps_a_child_whose_turn_errored_to_error_with_empty_output() {
    let h = Harness::new(vec![]).await;
    let run = h.start().await.unwrap();
    let result = run.result.await.unwrap();
    assert_eq!(result.stop_reason, SubagentStopReason::Error);
    assert!(result.output.is_empty());
    (run.dispose)().await.unwrap();
    h.close().await;
}

#[tokio::test]
async fn rejects_without_publishing_when_the_request_signal_is_already_aborted() {
    let h = Harness::new(vec![]).await;
    let controller = AbortController::new();
    controller.abort_with(HarnessError::named(
        "CallerStopped",
        "why",
        "CALLER_STOPPED",
    ));
    let reason = controller.signal().reason().unwrap();
    let before_agents = h.agents().list().len();
    let error = match h
        .runtime()
        .start_activation(h.spec(controller.signal(), None, None, None))
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("aborted start unexpectedly succeeded"),
    };
    let propagated = SharedError::of(error.as_ref()).expect("abort reason must stay shared");
    assert!(propagated.ptr_eq(&reason));
    assert_eq!(h.agents().list().len(), before_agents);
    h.close().await;
}

#[tokio::test]
async fn same_tick_cancellation_rejects_start_and_prevents_child_publication() {
    let h = Harness::new(vec![]).await;
    let controller = AbortController::new();
    let before_agents = h.agents().list().len();
    let before_sessions = h.ctx.get::<Sessions>().unwrap().list().len();
    let runtime = h.runtime();
    let starting = runtime.start_activation(h.spec(controller.signal(), None, None, None));
    controller.abort_with("early");
    assert!(starting.await.is_err());
    assert_eq!(h.agents().list().len(), before_agents);
    assert_eq!(
        h.ctx.get::<Sessions>().unwrap().list().len(),
        before_sessions
    );
    h.close().await;
}

#[tokio::test]
async fn disposing_a_running_child_settles_the_activation_as_aborted() {
    let h = Harness::new(vec![Entry::Hang]).await;
    let run = h.start().await.unwrap();
    (run.dispose)().await.unwrap();
    let result = run.result.await.unwrap();
    assert_eq!(result.stop_reason, SubagentStopReason::Aborted);
    h.close().await;
}

#[tokio::test]
async fn dispose_cancels_the_child_reaches_quiescence_and_is_idempotent() {
    let h = Harness::new(vec![Entry::Hang]).await;
    let run = h.start().await.unwrap();
    (run.dispose)().await.unwrap();
    (run.dispose)().await.unwrap();
    let result = run.result.await.unwrap();
    assert_eq!(result.stop_reason, SubagentStopReason::Aborted);
    assert!(h.agents().get(&run.child_id).is_none());
    h.close().await;
}

#[tokio::test]
async fn the_provider_supplies_fresh_local_preparation_for_managed_activations() {
    let h = Harness::new(vec![]).await;
    let provider = h.runtime().get_provider("spawn").unwrap();
    assert!(provider.supports_continuable());
    assert!(!provider.supports_start());
    let spec = provider
        .prepare_continuable(ContinuableCreateRequest {
            cwd: "/tmp".into(),
            session_id: "continuable-child".into(),
            parent: h.parent.clone(),
            signal: AbortController::new().signal(),
        })
        .unwrap()
        .await
        .unwrap();
    assert!(spec.seed.is_none());
    h.close().await;
}

#[tokio::test]
async fn inherits_the_parent_cwd_into_the_child_session() {
    let h = Harness::new(vec![Entry::Chunks(text_response("x"))]).await;
    let run = h.start().await.unwrap();
    let child = h.agents().get(&run.child_id).unwrap();
    assert_eq!(child.session().header()["cwd"], "/tmp");
    run.result.await.unwrap();
    (run.dispose)().await.unwrap();
    h.close().await;
}

#[tokio::test]
async fn uses_request_agent_options_model_when_the_parent_has_no_model() {
    let h = Harness::new(vec![Entry::Chunks(text_response("explicit model child"))]).await;
    let mut options = CreateAgentOptions::new("modelless-parent".into());
    options.agent_options = Some(AgentOptions::default());
    let parent_handle = h.agents().create(&h.ctx, options).await.unwrap();
    let mut spec = h.spec(
        AbortController::new().signal(),
        None,
        Some(AgentOptions {
            provider: Some(Live::Value("mock".into())),
            model: Some(Live::Value("mock".into())),
            ..Default::default()
        }),
        None,
    );
    spec.request.parent = parent_handle.agent.clone();
    let run = h.runtime().start_activation(spec).await.unwrap();
    let result = run.result.await.unwrap();
    assert_eq!(result.stop_reason, SubagentStopReason::Completed);
    assert_eq!(output_text(&result.output), "explicit model child");
    assert_eq!(lock(&h.adapter.requests)[0].model, "mock");
    (run.dispose)().await.unwrap();
    parent_handle.dispose().await.unwrap();
    h.close().await;
}

#[test]
fn advertises_every_start_time_capability() {
    assert_eq!(
        SpawnInProcessProvider::new("spawn").capabilities(),
        SubagentCapabilities {
            agent_options: true,
            agent_preset: true,
            output_schema: true,
            depth_limit: true,
        }
    );
    assert!(!SpawnInProcessProvider::new("spawn").inherits_parent_context());
}

#[tokio::test]
async fn unregisters_the_provider_when_its_fiber_is_disposed() {
    let h = Harness::new(vec![]).await;
    assert!(h.runtime().get_provider("spawn").is_some());
    h.spawn_fiber.dispose().await;
    assert!(h.runtime().get_provider("spawn").is_none());
    h.close().await;
}

#[tokio::test]
async fn captures_structured_output_through_the_shipped_plugin() {
    let release = Arc::new(Notify::new());
    let h = Harness::new(vec![Entry::Gated(
        tool_call_response("c1", STRUCTURED_OUTPUT_TOOL, json!({"answer":42})),
        release.clone(),
    )])
    .await;
    let run = h
        .runtime()
        .start_activation(h.spec(
            AbortController::new().signal(),
            None,
            None,
            Some(json!({
                "type":"object",
                "properties":{"answer":{"type":"number"}},
                "required":["answer"]
            })),
        ))
        .await
        .unwrap();
    let scope = h.agents().get(&run.child_id).unwrap().scope_key();
    let tools = h.ctx.get::<Tools>().unwrap();
    assert!(
        tools
            .get(STRUCTURED_OUTPUT_TOOL, Some(&scope), None)
            .is_some()
    );
    release.notify_one();
    let result = run.result.await.unwrap();
    assert_eq!(result.stop_reason, SubagentStopReason::Completed);
    assert_eq!(result.structured, Some(json!({"answer":42})));
    assert!(
        tools
            .get(STRUCTURED_OUTPUT_TOOL, Some(&scope), None)
            .is_none()
    );
    (run.dispose)().await.unwrap();
    h.close().await;
}

#[tokio::test]
async fn a_backend_unload_does_not_revoke_an_accepted_holder_owned_run() {
    let h = Harness::new(vec![Entry::Hang]).await;
    let run = h
        .runtime()
        .start_activation(h.spec(
            AbortController::new().signal(),
            None,
            None,
            Some(json!({
                "type":"object",
                "properties":{"answer":{"type":"number"}}
            })),
        ))
        .await
        .unwrap();
    let scope = h.agents().get(&run.child_id).unwrap().scope_key();
    let tools = h.ctx.get::<Tools>().unwrap();
    assert!(
        tools
            .get(STRUCTURED_OUTPUT_TOOL, Some(&scope), None)
            .is_some()
    );
    h.spawn_fiber.dispose().await;
    assert!(h.runtime().get_provider("spawn").is_none());
    assert!(h.agents().get(&run.child_id).is_some());
    (run.dispose)().await.unwrap();
    assert_eq!(
        run.result.await.unwrap().stop_reason,
        SubagentStopReason::Aborted
    );
    assert!(
        tools
            .get(STRUCTURED_OUTPUT_TOOL, Some(&scope), None)
            .is_none()
    );
    h.close().await;
}

#[tokio::test]
async fn a_start_racing_an_already_unloading_backend_cannot_begin_child_creation() {
    let h = Harness::new(vec![]).await;
    let before_agents = h.agents().list().len();
    let before_sessions = h.ctx.get::<Sessions>().unwrap().list().len();
    let published = observe_creation_events(&h.ctx);
    h.spawn_fiber.dispose().await;
    let error = match h.start().await {
        Err(error) => error,
        Ok(_) => panic!("start without provider unexpectedly succeeded"),
    };
    assert!(error.to_string().contains("no subagent provider"));
    assert_eq!(h.agents().list().len(), before_agents);
    assert_eq!(
        h.ctx.get::<Sessions>().unwrap().list().len(),
        before_sessions
    );
    assert!(lock(&published).is_empty());
    h.close().await;
}

#[test]
fn has_the_namespace_plugin_export_shape_without_a_stray_default() {
    assert_eq!(NAME, "subagent-spawn-in-process");
    assert_eq!(INJECT, ["subagents"]);
    assert_eq!(Config::default().provider_name, "spawn");
    assert_eq!(<SubagentSpawnInProcess as hoh_kernel::Plugin>::NAME, NAME);
}

#[tokio::test]
async fn spawning_from_a_disposing_parent_fails_loud_with_no_orphaned_child() {
    let h = Harness::new(vec![]).await;
    let mut options = CreateAgentOptions::new("doomed".into());
    options.agent_options = Some(AgentOptions {
        provider: Some(Live::Value("mock".into())),
        model: Some(Live::Value("mock".into())),
        ..Default::default()
    });
    let parent = h.agents().create(&h.ctx, options).await.unwrap();
    parent.dispose().await.unwrap();
    let before_agents = h.agents().list().len();
    let before_sessions = h.ctx.get::<Sessions>().unwrap().list().len();
    let published = observe_creation_events(&h.ctx);
    let mut spec = h.spec(AbortController::new().signal(), None, None, None);
    spec.request.parent = parent.agent;
    let error = match h.runtime().start_activation(spec).await {
        Err(error) => error,
        Ok(_) => panic!("start from disposed parent unexpectedly succeeded"),
    };
    let message = error.to_string();
    assert!(
        message.contains("exact live parent agent")
            || message.contains("workingDirectory.ensure")
            || message == "inactive context",
        "unexpected disposed-parent failure: {message}"
    );
    assert_eq!(h.agents().list().len(), before_agents);
    assert_eq!(
        h.ctx.get::<Sessions>().unwrap().list().len(),
        before_sessions
    );
    assert!(lock(&published).is_empty());
    h.close().await;
}

#[tokio::test]
async fn parent_disposal_during_setup_prevents_every_publication_notification() {
    let h = Harness::new(vec![]).await;
    let mut options = CreateAgentOptions::new("setup-race-parent".into());
    options.agent_options = Some(AgentOptions {
        provider: Some(Live::Value("mock".into())),
        model: Some(Live::Value("mock".into())),
        ..Default::default()
    });
    let parent = h.agents().create(&h.ctx, options).await.unwrap();
    let before_agents = h.agents().list().len();
    let before_sessions = h.ctx.get::<Sessions>().unwrap().list().len();
    let published = observe_creation_events(&h.ctx);
    let starts = Arc::new(Mutex::new(Vec::new()));
    let ends = Arc::new(Mutex::new(Vec::new()));
    let start_out = starts.clone();
    h.ctx
        .on_contained::<SubagentRunInfo>(move |info| {
            lock(&start_out).push(info.id.clone());
            Ok(())
        })
        .unwrap();
    let end_out = ends.clone();
    h.ctx
        .on_contained::<SubagentRunEndInfo>(move |info| {
            lock(&end_out).push(info.id.clone());
            Ok(())
        })
        .unwrap();
    parent.dispose().await.unwrap();
    let mut spec = h.spec(AbortController::new().signal(), None, None, None);
    spec.request.parent = parent.agent;
    assert!(h.runtime().start_activation(spec).await.is_err());
    assert!(lock(&starts).is_empty());
    assert!(lock(&ends).is_empty());
    assert_eq!(h.agents().list().len(), before_agents - 1);
    assert_eq!(
        h.ctx.get::<Sessions>().unwrap().list().len(),
        before_sessions - 1
    );
    assert!(lock(&published).is_empty());
    h.close().await;
}
