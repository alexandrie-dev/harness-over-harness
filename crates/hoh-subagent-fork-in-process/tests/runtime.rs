use futures_util::{StreamExt, stream};
use hoh_agent::{Agent, AgentOptions, AgentSessionMeta, Agents, CancelOptions, Live};
use hoh_agent_loop_testkit::{
    AgentLoopTestDependenciesOptions, mount_agent_loop_test_dependencies,
    mount_agent_loop_test_harness,
};
use hoh_error::BoxError;
use hoh_kernel::{Context, Fiber, Kernel};
use hoh_llm::{
    AdapterRegistrationHandle, ContentBlock, GenerateOptions, Llm, LlmAdapter, LlmStream,
    MessageSource, NewUserMessage, StreamChunk, create_user_message,
};
use hoh_session::{AgentCancelCause, SessionHeader};
use hoh_subagent::{
    STRUCTURED_OUTPUT_TOOL, SubagentActivation, SubagentActivationSpec, SubagentDeliveryTarget,
    SubagentPlugin, SubagentRunInfo, SubagentStartRequest, SubagentStopReason, Subagents,
};
use hoh_subagent_fork_in_process::SubagentForkInProcess;
use hoh_subagent_spawn_in_process::SubagentSpawnInProcess;
use hoh_timeout::AbortController;
use hoh_tools::Tools;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
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

fn empty_stop() -> Vec<StreamChunk> {
    vec![chunk(json!({"type":"finish","reason":{"kind":"stop"}}))]
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
    requested: Notify,
}

impl ScriptedAdapter {
    fn new(script: Vec<Entry>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into()),
            requests: Mutex::new(Vec::new()),
            requested: Notify::new(),
        })
    }

    async fn wait_for_requests(&self, expected: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let notified = self.requested.notified();
                if lock(&self.requests).len() >= expected {
                    return;
                }
                notified.await;
            }
        })
        .await
        .expect("scripted model request");
    }
}

impl LlmAdapter for ScriptedAdapter {
    fn stream(&self, options: GenerateOptions) -> Result<LlmStream, BoxError> {
        let signal = options.signal.clone();
        lock(&self.requests).push(options);
        self.requested.notify_waiters();
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
    _fork_fiber: Fiber,
    _spawn_fiber: Option<Fiber>,
    _registration: AdapterRegistrationHandle,
}

impl Harness {
    async fn new(script: Vec<Entry>, with_spawn: bool) -> Self {
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
        ctx.plugin::<SubagentPlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let fork_fiber = ctx
            .plugin::<SubagentForkInProcess>(json!({"providerName":"fork"}))
            .unwrap();
        fork_fiber.settled().await.unwrap();
        let spawn_fiber = if with_spawn {
            let fiber = ctx
                .plugin::<SubagentSpawnInProcess>(json!({"providerName":"spawn"}))
                .unwrap();
            fiber.settled().await.unwrap();
            Some(fiber)
        } else {
            None
        };
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
            _fork_fiber: fork_fiber,
            _spawn_fiber: spawn_fiber,
            _registration: registration,
        }
    }

    fn runtime(&self) -> hoh_kernel::Service<Subagents> {
        self.ctx.get::<Subagents>().unwrap()
    }

    fn agents(&self) -> hoh_kernel::Service<Agents> {
        self.ctx.get::<Agents>().unwrap()
    }

    fn spec(&self, provider: &str, output_schema: Option<Value>) -> SubagentActivationSpec {
        SubagentActivationSpec {
            provider: provider.into(),
            label: format!("{provider} child"),
            name: None,
            child_id: None,
            delivery: SubagentDeliveryTarget::Caller,
            request: SubagentStartRequest {
                cwd: None,
                agent_preset: None,
                label: None,
                prompt: vec![ContentBlock::text(format!("{provider} task"))],
                parent: self.parent.clone(),
                signal: AbortController::new().signal(),
                agent_options: None,
                output_schema,
                max_depth: None,
            },
        }
    }

    async fn start(
        &self,
        provider: &str,
        output_schema: Option<Value>,
    ) -> Result<SubagentActivation, BoxError> {
        self.runtime()
            .start_activation(self.spec(provider, output_schema))
            .await
    }

    async fn close(self) {
        self.kernel.shutdown().await;
    }
}

fn user_message(text: &str) -> hoh_llm::UserMessage {
    create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source: MessageSource::new("user"),
    })
}

fn output_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(ContentBlock::text_value)
        .collect::<Vec<_>>()
        .join("")
}

fn header(agent: &Arc<dyn Agent>) -> SessionHeader {
    serde_json::from_value((*agent.session().header()).clone()).unwrap()
}

fn turn_end_count(agent: &Arc<dyn Agent>) -> usize {
    agent
        .session()
        .snapshot_events(None, None)
        .iter()
        .filter(|event| event.event_type() == "turn/end")
        .count()
}

#[tokio::test]
async fn emits_start_only_after_the_seeded_child_is_published() {
    let h = Harness::new(
        vec![
            Entry::Chunks(text_response("parent answer")),
            Entry::Chunks(text_response("child answer")),
        ],
        false,
    )
    .await;
    h.parent.followup(user_message("parent question")).unwrap();
    h.parent.when_idle().await;

    let seen = Arc::new(Mutex::new(Vec::new()));
    let output = seen.clone();
    let agents = h.agents();
    h.ctx
        .on_contained::<SubagentRunInfo>(move |info| {
            if info.provider == "fork" {
                lock(&output).push((info.id.clone(), agents.get(&info.id)));
            }
            Ok(())
        })
        .unwrap();

    let run = h.start("fork", None).await.unwrap();
    let live = h.agents().get(&run.child_id).unwrap();
    assert_eq!(lock(&seen).len(), 1);
    assert_eq!(lock(&seen)[0].0, run.child_id);
    assert!(Arc::ptr_eq(lock(&seen)[0].1.as_ref().unwrap(), &live));
    run.result.await.unwrap();
    (run.dispose)().await.unwrap();
    h.close().await;
}

#[tokio::test]
async fn no_completed_parent_turn_runs_one_fresh_unseeded_child_turn() {
    let h = Harness::new(vec![Entry::Chunks(text_response("fresh child"))], false).await;
    let run = h.start("fork", None).await.unwrap();
    let child = h.agents().get(&run.child_id).unwrap();
    let result = run.result.await.unwrap();
    assert_eq!(result.stop_reason, SubagentStopReason::Completed);
    assert_eq!(output_text(&result.output), "fresh child");
    assert!(!header(&child).is_seeded);
    assert_eq!(u64::from(child.session().inherited_event_count()), 0);
    assert_eq!(turn_end_count(&child), 1);
    (run.dispose)().await.unwrap();
    h.close().await;
}

#[tokio::test]
async fn seeded_empty_child_does_not_borrow_parent_output() {
    let h = Harness::new(
        vec![
            Entry::Chunks(text_response("parent stale")),
            Entry::Chunks(empty_stop()),
        ],
        false,
    )
    .await;
    h.parent.followup(user_message("parent question")).unwrap();
    h.parent.when_idle().await;
    let prefix_len = h.parent.session().snapshot_events(None, None).len();

    let run = h.start("fork", None).await.unwrap();
    let child = h.agents().get(&run.child_id).unwrap();
    let result = run.result.await.unwrap();
    assert_eq!(result.stop_reason, SubagentStopReason::Completed);
    assert!(result.output.is_empty());
    assert!(header(&child).is_seeded);
    assert_eq!(
        usize::try_from(u64::from(child.session().inherited_event_count())).unwrap(),
        prefix_len
    );
    assert_eq!(header(&child).parent_session.as_ref(), Some(h.parent.id()));
    (run.dispose)().await.unwrap();
    h.close().await;
}

#[tokio::test]
async fn forking_during_an_open_parent_turn_keeps_one_balanced_seed_turn() {
    let h = Harness::new(
        vec![
            Entry::Chunks(text_response("parent done")),
            Entry::Hang,
            Entry::Chunks(text_response("child answer")),
        ],
        false,
    )
    .await;
    h.parent.followup(user_message("first")).unwrap();
    h.parent.when_idle().await;
    h.parent.followup(user_message("second")).unwrap();
    h.adapter.wait_for_requests(2).await;

    let run = h.start("fork", None).await.unwrap();
    let child = h.agents().get(&run.child_id).unwrap();
    let result = run.result.await.unwrap();
    assert_eq!(result.stop_reason, SubagentStopReason::Completed);
    assert_eq!(output_text(&result.output), "child answer");
    assert_eq!(turn_end_count(&child), 2);

    h.parent
        .cancel(AgentCancelCause::User, CancelOptions { keep_inbox: false })
        .unwrap();
    h.parent.when_idle().await;
    (run.dispose)().await.unwrap();
    h.close().await;
}

#[tokio::test]
async fn seeded_child_captures_structured_output_and_removes_its_tool() {
    let release = Arc::new(Notify::new());
    let h = Harness::new(
        vec![
            Entry::Chunks(text_response("parent done")),
            Entry::Gated(
                tool_call_response("c1", STRUCTURED_OUTPUT_TOOL, json!({"answer":9})),
                release.clone(),
            ),
        ],
        false,
    )
    .await;
    h.parent.followup(user_message("warm up")).unwrap();
    h.parent.when_idle().await;
    let run = h
        .start(
            "fork",
            Some(json!({
                "type":"object",
                "properties":{"answer":{"type":"number"}},
                "required":["answer"]
            })),
        )
        .await
        .unwrap();
    let child = h.agents().get(&run.child_id).unwrap();
    let scope = child.scope_key();
    let tools = h.ctx.get::<Tools>().unwrap();
    assert!(
        tools
            .get(STRUCTURED_OUTPUT_TOOL, Some(&scope), None)
            .is_some()
    );
    release.notify_one();
    let result = run.result.await.unwrap();
    assert_eq!(result.stop_reason, SubagentStopReason::Completed);
    assert_eq!(result.structured, Some(json!({"answer":9})));
    assert!(
        tools
            .get(STRUCTURED_OUTPUT_TOOL, Some(&scope), None)
            .is_none()
    );
    (run.dispose)().await.unwrap();
    h.close().await;
}

#[tokio::test]
async fn one_parent_runs_spawn_then_fork_then_continues_its_own_work() {
    let h = Harness::new(
        vec![
            Entry::Chunks(text_response("parent turn one")),
            Entry::Chunks(text_response("spawn child reply")),
            Entry::Chunks(text_response("fork child reply")),
            Entry::Chunks(text_response("parent turn two")),
        ],
        true,
    )
    .await;
    h.parent.followup(user_message("parent q1")).unwrap();
    h.parent.when_idle().await;
    let prefix_len = h.parent.session().snapshot_events(None, None).len();

    let spawn = h.start("spawn", None).await.unwrap();
    let spawn_child = h.agents().get(&spawn.child_id).unwrap();
    let spawn_result = spawn.result.await.unwrap();
    assert_eq!(spawn_result.stop_reason, SubagentStopReason::Completed);
    assert_eq!(output_text(&spawn_result.output), "spawn child reply");

    let fork = h.start("fork", None).await.unwrap();
    let fork_child = h.agents().get(&fork.child_id).unwrap();
    let fork_result = fork.result.await.unwrap();
    assert_eq!(fork_result.stop_reason, SubagentStopReason::Completed);
    assert_eq!(output_text(&fork_result.output), "fork child reply");
    assert_ne!(spawn_child.id(), fork_child.id());
    assert_eq!(
        header(&spawn_child).parent_session.as_ref(),
        Some(h.parent.id())
    );
    assert_eq!(
        header(&fork_child).parent_session.as_ref(),
        Some(h.parent.id())
    );
    assert_eq!(u64::from(spawn_child.session().inherited_event_count()), 0);
    assert_eq!(
        usize::try_from(u64::from(fork_child.session().inherited_event_count())).unwrap(),
        prefix_len
    );

    (spawn.dispose)().await.unwrap();
    (fork.dispose)().await.unwrap();
    h.parent.followup(user_message("parent q2")).unwrap();
    h.parent.when_idle().await;
    assert_eq!(turn_end_count(&h.parent), 2);
    let parent_events = h.parent.session().snapshot_events(None, None);
    let last = parent_events
        .iter()
        .rev()
        .find(|event| event.event_type() == "assistant/message")
        .unwrap();
    let content: Vec<ContentBlock> =
        serde_json::from_value(last.data()["message"]["content"].clone()).unwrap();
    assert_eq!(output_text(&content), "parent turn two");
    h.close().await;
}
