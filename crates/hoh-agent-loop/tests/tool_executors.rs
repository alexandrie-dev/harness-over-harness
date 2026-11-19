//! Port of tool-executors.spec.ts: scoped executor selection and lifetime through the real Agent loop.
//!
//! `independent` of the source builds a `ToolRunContext` by hand. In Rust a `ToolExecution` can only be
//! minted by the tools package, so the independent executor takes one from `ToolExecutionPipeline`
//! (with an executor-owned lookup for its transport) and calls the transport body itself, with no
//! pipeline policy, exactly as the source calls `transport.execute(args, exec)`.
//!
//! Not portable and left out of the matching tests (compile-time facts in Rust):
//! `expect(ctx.tools).toBeInstanceOf(ToolsService)`, `'registerExecutor' in ctx.tools`,
//! `expect(first).not.toHaveProperty('toolExecutor')`.
mod support_s17;

use hoh_agent::Agent;
use hoh_error::BoxError;
use hoh_kernel::Context;
use hoh_llm::{ContentBlock, ToolCallBlock, ToolCallId};
use hoh_scope::{CreateScopeOptions, Scope, ScopeKey, bind_scope_parent, create_scope};
use hoh_session::{SurfaceIntent, SurfaceOp};
use hoh_system_prompt::{
    AssembleContext, PromptAssembly, PromptPresentation, PromptSection, SystemPromptAssemble,
};
use hoh_timeout::AbortController;
use hoh_tools::shim::BoxFuture;
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, ToolBatchInput, ToolBatchResult, ToolCallResult,
    ToolDefinition, ToolExecution, ToolExecutionInput, ToolExecutionPipeline, ToolExecutionResult,
    ToolExecutionSource, ToolExecutionSuccess, ToolExecutor, ToolLogStep, ToolModelInput,
    ToolPromptSection, ToolPromptText, ToolRestriction, ToolsChange, define_tool,
    project_tool_schema,
};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicUsize, Ordering},
};
use support_s17::*;

type Trace = Arc<Mutex<Vec<String>>>;
type AgentAndTools = (Arc<dyn Agent>, hoh_kernel::Service<hoh_tools::Tools>);
type BeforeExecute = Arc<dyn Fn(&ToolBatchInput) -> BoxFuture<'static, ()> + Send + Sync>;

/// An independent execution implementation: no PTC executor inheritance or delegation.
struct Independent {
    name: String,
    trace: Trace,
    transport: Arc<ToolDefinition>,
    pipeline: ToolExecutionPipeline,
    sections: Vec<ToolPromptSection>,
    /// A wrapper around `execute` (the source reassigns `executor.execute`).
    before_execute: Option<BeforeExecute>,
    /// A wrapper around `describe`.
    on_describe: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Make `execute` reject with a plain error (not a `ToolBatchExecutionError`).
    plain_failure: Option<PlainFailure>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PlainFailure {
    BeforeCall,
    AfterRecordedCall,
}

fn independent(ctx: &Context, name: &str, trace: &Trace) -> Independent {
    let tools = ctx.get::<hoh_tools::Tools>().expect("tools service");
    let transport_name = name.to_owned();
    let transport = Arc::new(
        define_tool(DefineToolOptions::new(
            name,
            format!("Execute through {name}."),
            json!({}),
            DefineToolOutput {
                schema: json!({ "type": "string" }),
                render: Arc::new(|_, value| {
                    vec![ContentBlock::text(value.as_str().expect("string value"))]
                }),
                presentation_meta: None,
            },
            move |_, exec: Arc<ToolExecution>| {
                let (tools, name) = (tools.clone(), transport_name.clone());
                Box::pin(async move {
                    let scope = exec.agent.as_ref().map(|agent| agent.scope_key());
                    let echo = tools
                        .get("echo", scope.as_ref(), None)
                        .ok_or_else(|| BoxError::from("echo is not visible"))?;
                    let value = (echo.execute)(json!({}), exec.clone()).await?;
                    Ok(json!(format!(
                        "{name}:{}",
                        value.as_str().unwrap_or_default()
                    )))
                })
            },
        ))
        .expect("transport defines"),
    );
    let lookup = transport.clone();
    let pipeline = ToolExecutionPipeline::with_source(
        ctx.clone(),
        ToolExecutionSource {
            get: Some(Arc::new(move |called, _, _| {
                (called == lookup.name).then(|| lookup.clone())
            })),
            reject: None,
        },
    )
    .expect("pipeline");
    Independent {
        name: name.to_owned(),
        trace: trace.clone(),
        transport,
        pipeline,
        sections: vec![
            ToolPromptSection {
                name: "tools:transport".into(),
                order: 800.0,
                interpolate: Some(false),
                text: ToolPromptText::Static(format!("Call {name}.")),
            },
            ToolPromptSection {
                name: "tools:sdk".into(),
                order: 5000.0,
                interpolate: Some(false),
                text: ToolPromptText::Provider(Arc::new(|tools| {
                    tools
                        .iter()
                        .map(|tool| tool.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })),
            },
        ],
        before_execute: None,
        on_describe: None,
        plain_failure: None,
    }
}

impl Independent {
    async fn run(&self, input: ToolBatchInput) -> Result<ToolBatchResult, BoxError> {
        lock(&self.trace).push(format!("{}:execute", self.name));
        let mut results = Vec::new();
        for call in &input.calls {
            if call.name != self.name {
                return Err(format!("unexpected transport {}", call.name).into());
            }
            if self.plain_failure == Some(PlainFailure::BeforeCall) {
                return Err("plain executor failure".into());
            }
            let call_seq = match (&input.log, &input.agent) {
                (Some(log), Some(agent)) => Some(
                    agent
                        .session()
                        .append(
                            "tool/call",
                            json!({
                                "turn": log.turn, "step": log.step,
                                "callId": call.id.as_str(), "name": call.name,
                                "arguments": call.arguments,
                            }),
                            None,
                        )?
                        .seq(),
                ),
                _ => None,
            };
            if self.plain_failure == Some(PlainFailure::AfterRecordedCall) {
                return Err("plain executor failure".into());
            }
            let arguments: Value = serde_json::from_str(&call.arguments)?;
            let prepared = self
                .pipeline
                .prepare(ToolExecutionInput {
                    call_id: call.id.clone(),
                    root_call_id: None,
                    name: call.name.clone(),
                    schema: None,
                    arguments,
                    agent: input.agent.clone(),
                    tool_set: None,
                    parent: None,
                    signal: input.signal.clone(),
                })
                .await?;
            let exec = prepared.exec().clone();
            let value = (self.transport.execute)(exec.arguments.clone(), exec.clone()).await?;
            let text = value.as_str().unwrap_or_default().to_owned();
            let content = vec![ContentBlock::text(text)];
            if let (Some(log), Some(agent)) = (&input.log, &input.agent) {
                let message =
                    hoh_llm::create_tool_result_message(hoh_llm::ToolResultMessageInput {
                        call_id: call.id.clone(),
                        content: content.clone(),
                        is_error: false,
                    });
                agent.session().append(
                    "tool/result",
                    json!({ "turn": log.turn, "step": log.step, "message": message }),
                    Some(SurfaceIntent {
                        surface_op: Some(SurfaceOp::append()),
                        source_event_seqs: call_seq.map(|seq| vec![seq]),
                    }),
                )?;
            }
            results.push(ToolCallResult {
                call_id: call.id.clone(),
                result: ToolExecutionResult::Success(ToolExecutionSuccess::new(value, content)),
            });
        }
        Ok(ToolBatchResult {
            results,
            additional_contexts: Vec::new(),
            concluded: false,
        })
    }
}

impl ToolExecutor for Independent {
    fn sections(&self) -> Vec<ToolPromptSection> {
        self.sections.clone()
    }
    fn describe(&self, _tools: &[Arc<ToolDefinition>]) -> Result<ToolModelInput, BoxError> {
        if let Some(hook) = &self.on_describe {
            hook();
        }
        Ok(ToolModelInput {
            schemas: vec![project_tool_schema(&self.transport, false)?],
            known_names: Some(vec![self.name.clone()]),
        })
    }
    fn execute(&self, input: ToolBatchInput) -> BoxFuture<'_, Result<ToolBatchResult, BoxError>> {
        Box::pin(async move {
            if let Some(hook) = &self.before_execute {
                hook(&input).await;
            }
            self.run(input).await
        })
    }
}

fn trace() -> Trace {
    Arc::new(Mutex::new(Vec::new()))
}

fn echo_tool() -> ToolDefinition {
    define_tool(DefineToolOptions::new(
        "echo",
        "Shared capability.",
        json!({}),
        DefineToolOutput {
            schema: json!({ "type": "string" }),
            render: Arc::new(|_, value| vec![ContentBlock::text(value.as_str().expect("string"))]),
            presentation_meta: None,
        },
        |_, _| Box::pin(async { Ok(json!("shared")) }),
    ))
    .expect("echo defines")
}

async fn setup(adapter: Arc<dyn hoh_llm::LlmAdapter>, max_parallel: Option<u64>) -> Stack {
    let mut loop_config = json!({ "agents": [] });
    if let Some(limit) = max_parallel {
        loop_config["maxParallelToolCalls"] = json!(limit);
    }
    let stack = stack(
        adapter,
        StackOptions {
            loop_config,
            ..StackOptions::default()
        },
    )
    .await;
    stack
        .tools
        .register(&stack.ctx, echo_tool(), Default::default())
        .expect("echo registers");
    stack
}

async fn default_setup() -> Stack {
    setup(MockAdapter::chunks(vec![]), None).await
}

/// `scoped(ctx, key)`: a scope under the root context.
fn scoped(stack: &Stack, key: &ScopeKey) -> Scope {
    create_scope(&stack.ctx, key.clone(), CreateScopeOptions::default()).expect("scope")
}

async fn run(agent: &Arc<dyn Agent>) {
    send_and_wait(agent, "go").await;
}

fn call_block(id: &str, name: &str, arguments: &str) -> ToolCallBlock {
    ToolCallBlock {
        id: ToolCallId::from(id),
        name: name.into(),
        arguments: arguments.into(),
    }
}

fn batch(
    name: &str,
    id: &str,
    arguments: &str,
    agent: Option<&Arc<dyn Agent>>,
    log: Option<ToolLogStep>,
) -> ToolBatchInput {
    let mut input = ToolBatchInput::new(
        vec![call_block(id, name, arguments)],
        AbortController::new().signal(),
    );
    input.agent = agent.map(tools_agent);
    input.log = log;
    input
}

fn names(items: &[&str]) -> Option<Vec<String>> {
    Some(items.iter().map(|s| (*s).to_owned()).collect())
}

fn assembled_tool_names(assembly: &PromptAssembly) -> Vec<String> {
    assembly
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect()
}

async fn assemble(stack: &Stack, context: &AssembleContext) -> Result<PromptAssembly, BoxError> {
    stack.system_prompt.assemble(context.clone()).await
}

#[tokio::test]
async fn shares_the_registry_and_selects_an_independent_executor_through_the_preset_scope_chain() {
    let trace = trace();
    let adapter = MockAdapter::chunks(vec![
        tool_call_response("sdk-call", "sdk_eval", &json!({}), None),
        text_response("sdk done"),
        tool_call_response("native-call", "echo", &json!({}), None),
        text_response("native done"),
    ]);
    let stack = setup(adapter.clone(), None).await;
    let preset_key = ScopeKey::new();
    let preset = scoped(&stack, &preset_key);
    stack
        .tools
        .set_executor(
            &preset.ctx,
            Arc::new(independent(&preset.ctx, "sdk_eval", &trace)),
        )
        .expect("executor sets");
    let sdk = stack.create_agent("sdk", "mock").await;
    bind_scope_parent(&sdk.scope_key(), preset_key.clone()).expect("binds");
    let native = stack.create_agent("native", "mock").await;
    let global = stack.tools.get("echo", None, None).expect("echo");
    for agent in [&sdk, &native] {
        let seen = stack
            .tools
            .get("echo", Some(&agent.scope_key()), None)
            .expect("echo is visible");
        assert!(Arc::ptr_eq(&seen, &global));
    }
    run(&sdk).await;
    run(&native).await;
    {
        let requests = lock(&adapter.requests);
        assert_eq!(tool_names(&requests[0]), names(&["sdk_eval"]));
        assert_eq!(tool_names(&requests[2]), names(&["echo"]));
    }
    assert_eq!(*lock(&trace), ["sdk_eval:execute"]);
    let result = events_of(&sdk, "tool/result")
        .into_iter()
        .next()
        .expect("result");
    assert_match_object(
        &result,
        &json!({ "data": { "message": {
            "content": [{ "type": "text", "text": "sdk_eval:shared" }], "isError": false } } }),
    );
    stack.close().await;
}

#[tokio::test]
async fn dispatches_through_the_current_agent_scope_after_prompt_listeners_rebuild_the_assembly() {
    let trace = trace();
    let slot: Arc<OnceLock<AgentAndTools>> = Arc::new(OnceLock::new());
    let first_request = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let mut adapter = ModelAdapter::new(MockAdapter::chunks(vec![
        tool_call_response("held", "first_eval", &json!({}), None),
        text_response("done"),
    ]));
    let (hook_slot, hook_trace, first) = (slot.clone(), trace.clone(), first_request.clone());
    adapter.on_request = Some(Arc::new(move |_| {
        // The first request selects a nearer executor (`selectNearer`).
        if !first.swap(false, Ordering::SeqCst) {
            return;
        }
        let (agent, tools) = hook_slot.get().expect("agent is created");
        let mut nearer = independent(agent.ctx(), "first_eval", &hook_trace);
        let marker = hook_trace.clone();
        nearer.before_execute = Some(Arc::new(move |_| {
            lock(&marker).push("nearer".into());
            Box::pin(async {})
        }));
        tools
            .set_executor(agent.ctx(), Arc::new(nearer))
            .expect("nearer executor sets");
    }));
    let adapter_view = adapter.inner.clone();
    let stack = setup(Arc::new(adapter), None).await;
    stack
        .ctx
        .on_waterfall::<SystemPromptAssemble, PromptPresentation, _>(|_, next| async move {
            let presentation = next.run().await?;
            Ok(PromptPresentation::new(presentation.snapshot()))
        })
        .expect("assemble listener");
    let preset_key = ScopeKey::new();
    let preset = scoped(&stack, &preset_key);
    stack
        .tools
        .set_executor(
            &preset.ctx,
            Arc::new(independent(&preset.ctx, "first_eval", &trace)),
        )
        .expect("executor sets");
    let agent = stack.create_agent("changing", "mock").await;
    bind_scope_parent(&agent.scope_key(), preset_key).expect("binds");
    let tools = stack.ctx.get::<hoh_tools::Tools>().expect("tools");
    assert!(slot.set((agent.clone(), tools)).is_ok());
    run(&agent).await;
    {
        let requests = lock(&adapter_view.requests);
        assert_eq!(tool_names(&requests[0]), names(&["first_eval"]));
        assert_eq!(tool_names(&requests[1]), names(&["first_eval"]));
    }
    assert_eq!(*lock(&trace), ["nearer", "first_eval:execute"]);
    stack.close().await;
}

#[tokio::test]
async fn rejects_new_batches_after_unload_while_an_accepted_batch_settles_on_its_original_executor()
{
    let trace = trace();
    let stack = default_setup().await;
    let key = ScopeKey::new();
    let scope = scoped(&stack, &key);
    let agent = stack.create_agent("temporary", "mock").await;
    bind_scope_parent(&agent.scope_key(), key.clone()).expect("binds");
    let mut provider = independent(&scope.ctx, "old_eval", &trace);
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<()>();
    let entered_tx = Mutex::new(Some(entered_tx));
    let release = Arc::new(tokio::sync::Notify::new());
    let gate = release.clone();
    provider.before_execute = Some(Arc::new(move |_| {
        if let Some(entered) = lock(&entered_tx).take() {
            let _ = entered.send(());
        }
        let gate = gate.clone();
        Box::pin(async move { gate.notified().await })
    }));
    let dispose = stack
        .tools
        .set_executor(&scope.ctx, Arc::new(provider))
        .expect("executor sets");
    let tools = stack.tools.clone();
    let pending = tokio::spawn({
        let input = batch("old_eval", "accepted", "{}", Some(&agent), None);
        async move { tools.execute(input).await }
    });
    entered_rx.await.expect("batch entered the executor");
    dispose.dispose().await;
    let unavailable = "selected tool executor is unavailable";
    let error = assemble(&stack, &AssembleContext::new(Some(key.clone())))
        .await
        .expect_err("assemble rejects");
    assert!(error.to_string().contains(unavailable), "{error}");
    let error = stack
        .tools
        .execute(batch("old_eval", "accepted", "{}", Some(&agent), None))
        .await
        .expect_err("execute rejects");
    assert!(error.to_string().contains(unavailable), "{error}");
    stack
        .tools
        .set_executor(
            &scope.ctx,
            Arc::new(independent(&scope.ctx, "replacement_eval", &trace)),
        )
        .expect("replacement sets");
    release.notify_one();
    let result = pending
        .await
        .expect("joins")
        .expect("accepted batch settles");
    let outcome = &result.results[0].result;
    assert!(!outcome.is_error());
    assert_eq!(outcome.value(), Some(&json!("old_eval:shared")));
    let replacement = stack
        .tools
        .execute(batch(
            "replacement_eval",
            "replacement",
            "{}",
            Some(&agent),
            None,
        ))
        .await
        .expect("replacement runs");
    let outcome = &replacement.results[0].result;
    assert!(!outcome.is_error());
    assert_eq!(outcome.value(), Some(&json!("replacement_eval:shared")));
    assert_eq!(
        *lock(&trace),
        ["old_eval:execute", "replacement_eval:execute"]
    );
    stack.close().await;
}

#[tokio::test]
async fn refreshes_executor_projections_without_emitting_tool_registry_changes() {
    let stack = default_setup().await;
    let registry_changes = Arc::new(AtomicUsize::new(0));
    let counter = registry_changes.clone();
    stack
        .ctx
        .on::<ToolsChange>(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .expect("listener");
    let key = ScopeKey::new();
    let scope = scoped(&stack, &key);
    let context = AssembleContext::new(Some(key));
    assert_eq!(
        assembled_tool_names(&assemble(&stack, &context).await.expect("assembles")),
        ["echo"]
    );
    let dispose = stack
        .tools
        .set_executor(
            &scope.ctx,
            Arc::new(independent(&scope.ctx, "new_eval", &trace())),
        )
        .expect("executor sets");
    assert_eq!(
        assembled_tool_names(&assemble(&stack, &context).await.expect("assembles")),
        ["new_eval"]
    );
    assert_eq!(registry_changes.load(Ordering::SeqCst), 0);
    dispose.dispose().await;
    let error = assemble(&stack, &context).await.expect_err("rejects");
    assert!(
        error
            .to_string()
            .contains("selected tool executor is unavailable")
    );
    assert_eq!(registry_changes.load(Ordering::SeqCst), 0);
    stack.close().await;
}

async fn rolls_back_executor_section_registrations(name: &str, order: f64, message: &str) {
    let stack = default_setup().await;
    let key = ScopeKey::new();
    let scope = scoped(&stack, &key);
    stack
        .system_prompt
        .section(
            &scope.ctx,
            PromptSection {
                name: "occupied".into(),
                order: 1.0,
                text: "Existing guidance.".into(),
                ..PromptSection::default()
            },
        )
        .expect("occupied registers");
    let mut executor = independent(&scope.ctx, "rejected_eval", &trace());
    executor.sections = vec![
        ToolPromptSection {
            name: "transient".into(),
            order: 0.0,
            interpolate: None,
            text: ToolPromptText::Static("Must be rolled back.".into()),
        },
        ToolPromptSection {
            name: name.into(),
            order,
            interpolate: None,
            text: ToolPromptText::Static("Invalid contribution.".into()),
        },
    ];
    let error = stack
        .tools
        .set_executor(&scope.ctx, Arc::new(executor))
        .err()
        .expect("setExecutor rejects");
    assert!(error.to_string().contains(message), "{error}");
    let assembly = assemble(&stack, &AssembleContext::new(Some(key)))
        .await
        .expect("assembles");
    assert_eq!(assembled_tool_names(&assembly), ["echo"]);
    assert!(
        !assembly
            .sections
            .iter()
            .any(|section| section.name == "transient")
    );
    stack.close().await;
}

#[tokio::test]
async fn rolls_back_executor_section_registrations_when_occupied_is_rejected() {
    rolls_back_executor_section_registrations("occupied", 1.0, "already registered").await;
}

#[tokio::test]
async fn rolls_back_executor_section_registrations_when_invalid_order_is_rejected() {
    rolls_back_executor_section_registrations(
        "invalid-order",
        f64::NAN,
        "order must be a finite number",
    )
    .await;
}

#[tokio::test]
async fn assembles_independent_tools_and_section_text_for_overlapping_prompt_assemblies() {
    let stack = default_setup().await;
    let key = ScopeKey::new();
    let scope = scoped(&stack, &key);
    let context = AssembleContext::new(Some(key));
    let (reached_tx, reached_rx) = tokio::sync::oneshot::channel::<()>();
    let reached_tx = Mutex::new(Some(reached_tx));
    let resume = Arc::new(tokio::sync::Notify::new());
    let (calls, gate) = (Arc::new(AtomicUsize::new(0)), resume.clone());
    stack
        .ctx
        .on_waterfall::<SystemPromptAssemble, PromptPresentation, _>(move |_, next| {
            let first = calls.fetch_add(1, Ordering::SeqCst) == 0;
            let (entered, gate) = (lock(&reached_tx).take(), gate.clone());
            async move {
                if first {
                    if let Some(entered) = entered {
                        let _ = entered.send(());
                    }
                    gate.notified().await;
                }
                let presentation = next.run().await?;
                Ok(PromptPresentation::new(presentation.snapshot()))
            }
        })
        .expect("assemble listener");
    let pending = stack.system_prompt.assemble(context.clone());
    reached_rx
        .await
        .expect("first assembly reached the listener");
    let projections = Arc::new(AtomicUsize::new(0));
    let mut replacement = independent(&scope.ctx, "next_eval", &trace());
    let counter = projections.clone();
    replacement.on_describe = Some(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }));
    stack
        .tools
        .set_executor(&scope.ctx, Arc::new(replacement))
        .expect("replacement sets");
    let second = assemble(&stack, &context).await;
    resume.notify_one();
    let first = pending.await.expect("first assembles");
    let second = second.expect("second assembles");
    assert_eq!(assembled_tool_names(&first), ["echo"]);
    assert_eq!(assembled_tool_names(&second), ["next_eval"]);
    assert!(second.sections.iter().any(|section| {
        section.name == "tools:transport"
            && section.text == "Call next_eval."
            && section.interpolate == Some(false)
    }));
    assert!(second.sections.iter().any(|section| {
        section.name == "tools:sdk" && section.text == "echo" && section.interpolate == Some(false)
    }));
    assert_eq!(projections.load(Ordering::SeqCst), 1);
    stack.close().await;
}

#[tokio::test]
async fn keeps_the_foundation_usable_for_registration_and_requires_an_explicit_executor_for_calls()
{
    let stack = stack(
        MockAdapter::chunks(vec![]),
        StackOptions {
            native_executor: false,
            ..StackOptions::default()
        },
    )
    .await;
    stack
        .tools
        .register(
            &stack.ctx,
            define_tool(DefineToolOptions::new(
                "echo",
                "Registered without an executor.",
                json!({}),
                DefineToolOutput {
                    schema: json!({ "type": "string" }),
                    render: Arc::new(|_, value| {
                        vec![ContentBlock::text(value.as_str().expect("string"))]
                    }),
                    presentation_meta: None,
                },
                |_, _| Box::pin(async { Ok(json!("shared")) }),
            ))
            .expect("echo defines"),
            Default::default(),
        )
        .expect("echo registers");
    let schemas = stack.tools.schemas(None, None).expect("schemas");
    assert_eq!(
        schemas
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        ["echo"]
    );
    let unavailable = "selected tool executor is unavailable";
    let error = assemble(&stack, &AssembleContext::new(None))
        .await
        .expect_err("assemble rejects");
    assert!(error.to_string().contains(unavailable), "{error}");
    let error = stack
        .tools
        .execute(batch("echo", "unconfigured", "{}", None, None))
        .await
        .expect_err("execute rejects");
    assert!(error.to_string().contains(unavailable), "{error}");
    stack.close().await;
}

struct Recorded {
    arguments: Vec<String>,
    agent_scope: Option<ScopeKey>,
    log: Option<ToolLogStep>,
    max_parallel: Option<u64>,
}

async fn hands_the_executor_raw_calls_and_the_resolved_loop_limit(max_parallel: Option<u64>) {
    let raw = ["{ \"value\": 1 }", "[2, 3]"];
    let adapter = MockAdapter::chunks(vec![
        vec![
            chunk(json!({"type": "block-start", "index": 0, "blockType": "tool-call"})),
            chunk(json!({"type": "block-end", "index": 0, "block": {
                "type": "tool-call", "id": "one", "name": "custom", "arguments": raw[0] }})),
            chunk(json!({"type": "block-start", "index": 1, "blockType": "tool-call"})),
            chunk(json!({"type": "block-end", "index": 1, "block": {
                "type": "tool-call", "id": "two", "name": "custom", "arguments": raw[1] }})),
            chunk(json!({"type": "finish", "reason": {"kind": "tool-calls"}})),
        ],
        text_response("done"),
    ]);
    let stack = setup(adapter, max_parallel).await;
    let agent = stack.create_agent("raw-batch", "mock").await;
    let mut executor = independent(agent.ctx(), "custom", &trace());
    let batches = Arc::new(Mutex::new(Vec::<Recorded>::new()));
    let recorder = batches.clone();
    executor.before_execute = Some(Arc::new(move |input| {
        lock(&recorder).push(Recorded {
            arguments: input
                .calls
                .iter()
                .map(|call| call.arguments.clone())
                .collect(),
            agent_scope: input.agent.as_ref().map(|agent| agent.scope_key()),
            log: input.log,
            max_parallel: input.max_parallel_tool_calls,
        });
        Box::pin(async {})
    }));
    stack
        .tools
        .set_executor(agent.ctx(), Arc::new(executor))
        .expect("executor sets");
    run(&agent).await;
    {
        let batches = lock(&batches);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].arguments, raw);
        assert_eq!(batches[0].agent_scope, Some(agent.scope_key()));
        assert_eq!(batches[0].log, Some(ToolLogStep { turn: 1, step: 1 }));
        assert_eq!(batches[0].max_parallel, Some(max_parallel.unwrap_or(10)));
    }
    let ids: Vec<_> = events_of(&agent, "tool/result")
        .iter()
        .map(|event| {
            event["data"]["message"]["toolCallId"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    assert_eq!(ids, ["one", "two"]);
    stack.close().await;
}

#[tokio::test]
async fn hands_the_executor_raw_calls_and_the_resolved_loop_limit_default() {
    hands_the_executor_raw_calls_and_the_resolved_loop_limit(None).await;
}

#[tokio::test]
async fn hands_the_executor_raw_calls_and_the_resolved_loop_limit_1() {
    hands_the_executor_raw_calls_and_the_resolved_loop_limit(Some(1)).await;
}

#[tokio::test]
async fn hands_the_executor_raw_calls_and_the_resolved_loop_limit_6() {
    hands_the_executor_raw_calls_and_the_resolved_loop_limit(Some(6)).await;
}

fn error_code(result: &ToolExecutionResult) -> Option<String> {
    result
        .error()
        .and_then(|error| error.info.as_ref())
        .map(|info| info.code.clone())
}

#[tokio::test]
async fn keeps_agent_scope_without_a_model_step_and_logs_raw_calls_only_with_a_logging_step() {
    let stack = default_setup().await;
    let agent = stack.create_agent("batch-scope", "mock").await;
    stack
        .tools
        .restrict(
            agent.ctx(),
            ToolRestriction {
                allow: None,
                deny: Some(vec!["echo".into()]),
            },
        )
        .expect("restricts");
    let scoped = stack
        .tools
        .execute(batch("echo", "scoped", "{ }", Some(&agent), None))
        .await
        .expect("executes");
    assert_eq!(
        error_code(&scoped.results[0].result).as_deref(),
        Some("UNKNOWN_TOOL")
    );
    assert_eq!(events_of(&agent, "tool/call").len(), 0);
    let owned = stack
        .tools
        .execute(batch(
            "echo",
            "scoped",
            "{ }",
            Some(&agent),
            Some(ToolLogStep { turn: 1, step: 1 }),
        ))
        .await
        .expect("executes");
    assert_eq!(
        error_code(&owned.results[0].result).as_deref(),
        Some("UNKNOWN_TOOL")
    );
    let calls = events_of(&agent, "tool/call");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["data"]["arguments"], "{ }");
    let unowned = stack
        .tools
        .execute(batch("echo", "scoped", "{ }", None, None))
        .await
        .expect("executes");
    let outcome = &unowned.results[0].result;
    assert!(!outcome.is_error());
    assert_eq!(outcome.value(), Some(&json!("shared")));
    assert_eq!(events_of(&agent, "tool/call").len(), 1);
    stack.close().await;
}

/// Minimal reproduction of the driver hang of the `hands the executor raw calls…` cases: the executor
/// rejects with a plain error after the step recorded a `tool/call` that has no result yet.
async fn plain_executor_failure(failure: PlainFailure) -> (Stack, Arc<dyn Agent>) {
    let adapter = MockAdapter::chunks(vec![
        tool_call_response("c1", "custom", &json!({}), None),
        text_response("unused"),
    ]);
    let stack = setup(adapter, None).await;
    let agent = stack.create_agent("plain-failure", "mock").await;
    let mut executor = independent(agent.ctx(), "custom", &trace());
    executor.plain_failure = Some(failure);
    stack
        .tools
        .set_executor(agent.ctx(), Arc::new(executor))
        .expect("executor sets");
    (stack, agent)
}

#[tokio::test]
async fn a_plain_executor_error_without_a_recorded_call_closes_the_turn_with_an_error() {
    let (stack, agent) = plain_executor_failure(PlainFailure::BeforeCall).await;
    run(&agent).await;
    assert!(events_of(&agent, "tool/call").is_empty());
    let end = events_of(&agent, "turn/end")
        .into_iter()
        .next()
        .expect("turn end");
    assert_match_object(&end["data"], &json!({ "reason": { "kind": "error" } }));
    stack.close().await;
}

#[tokio::test]
async fn a_plain_executor_error_after_a_recorded_call_records_the_interrupted_result() {
    let (stack, agent) = plain_executor_failure(PlainFailure::AfterRecordedCall).await;
    run(&agent).await;
    assert_eq!(events_of(&agent, "tool/call").len(), 1);
    let results = events_of(&agent, "tool/result");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["data"]["message"]["toolCallId"], "c1");
    let end = events_of(&agent, "turn/end")
        .into_iter()
        .next()
        .expect("turn end");
    assert_match_object(&end["data"], &json!({ "reason": { "kind": "error" } }));
    stack.close().await;
}
