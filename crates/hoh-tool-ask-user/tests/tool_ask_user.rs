use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

use hoh_agent::{
    Agent, AgentOptions, AgentRegistryPlugin, AgentStatus, Agents, CancelOptions, Inbox,
    InboxTarget, MaintenanceFuture, MaintenanceTask, Model, ModelSelection,
};
use hoh_error::BoxError;
use hoh_kernel::{Context, Fiber, Kernel};
use hoh_llm::{ContentBlock, MessageId, ToolCallBlock, ToolCallId, UserMessage};
use hoh_scope::ScopeKey;
use hoh_session::{
    AgentCancelCause, CreateSessionOptions, Session, SessionEvent, SessionId, SessionStore,
};
use hoh_session_projection::SessionProjectionRegistry;
use hoh_system_prompt::SystemPrompt;
use hoh_timeout::{AbortController, AbortSignal};
use hoh_tool_ask_user::{Config, Mode, ToolAskUser, apply};
use hoh_tools::{
    ToolBatchInput, ToolExecutionPipeline, ToolExecutionResult, ToolRuntime, Tools,
    execute_tool_batch,
};
use hoh_user_questions::{
    AskUserQuestionAnswer, AskUserQuestionAnswerItem, AskUserQuestionRequestEvent, UserQuestions,
    UserQuestionsPlugin, fold_user_questions, is_timed_ask_user_question_schema,
};
use serde_json::{Value, json};

struct Stack {
    kernel: Kernel,
    ctx: Context,
    tool: Fiber,
    sessions: SessionStore,
}

async fn mount_dependencies() -> (Kernel, Context) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionProjectionRegistry>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<AgentRegistryPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<SystemPrompt>(json!({ "includeRuntimeContext": false }))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<ToolRuntime>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<UserQuestionsPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    (kernel, ctx)
}

async fn mount(config: Value) -> Stack {
    let (kernel, ctx) = mount_dependencies().await;
    let tool = ctx.plugin::<ToolAskUser>(config).unwrap();
    tool.settled().await.unwrap();
    Stack {
        kernel,
        ctx,
        tool,
        sessions: SessionStore::new(),
    }
}

async fn execute(ctx: &Context, arguments: Value) -> ToolExecutionResult {
    execute_with(ctx, arguments, None, AbortController::new().signal()).await
}

async fn execute_with(
    ctx: &Context,
    arguments: Value,
    agent: Option<Arc<dyn hoh_tools::Agent>>,
    signal: AbortSignal,
) -> ToolExecutionResult {
    let pipeline = ToolExecutionPipeline::new(ctx.clone()).unwrap();
    let mut input = ToolBatchInput::new(
        vec![ToolCallBlock {
            id: ToolCallId::from("ask-1"),
            name: "ask_user_question".into(),
            arguments: serde_json::to_string(&arguments).unwrap(),
        }],
        signal,
    );
    input.agent = agent;
    execute_tool_batch(input, &pipeline, &|| 1)
        .await
        .unwrap()
        .results
        .remove(0)
        .result
}

fn definition(ctx: &Context) -> Arc<hoh_tools::ToolDefinition> {
    ctx.get::<Tools>()
        .unwrap()
        .get("ask_user_question", None, None)
        .unwrap()
}

fn text(block: &ContentBlock) -> &str {
    block.text_value().expect("text result")
}

fn error_info(result: &ToolExecutionResult) -> Option<(&str, &str)> {
    result.error().and_then(|error| {
        error
            .info
            .as_ref()
            .map(|info| (info.name.as_str(), info.code.as_str()))
    })
}

struct UnusedModel;

impl Model for UnusedModel {
    fn current(&self) -> Option<ModelSelection> {
        None
    }

    fn select(&self, _: &ModelSelection) -> Result<(), BoxError> {
        Err("model fixture does not drive the agent".into())
    }
}

struct UnusedInbox;

impl Inbox for UnusedInbox {
    fn next_turn(&self) -> Vec<UserMessage> {
        vec![]
    }

    fn next_step(&self) -> Vec<UserMessage> {
        vec![]
    }

    fn clear(&self) -> Result<(), BoxError> {
        Ok(())
    }

    fn append(&self, _: InboxTarget, _: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }

    fn prepend(&self, _: InboxTarget, _: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }

    fn replace(&self, _: &MessageId, _: UserMessage) -> Result<bool, BoxError> {
        Ok(false)
    }

    fn remove(&self, _: &MessageId) -> Result<bool, BoxError> {
        Ok(false)
    }

    fn splice(
        &self,
        _: InboxTarget,
        _: f64,
        _: f64,
        _: Vec<UserMessage>,
    ) -> Result<Vec<UserMessage>, BoxError> {
        Ok(vec![])
    }
}

struct LiveAgent {
    session: Session,
    ctx: Context,
    scope: ScopeKey,
    options: AgentOptions,
    model: UnusedModel,
    inbox: UnusedInbox,
}

impl Agent for LiveAgent {
    fn id(&self) -> &SessionId {
        self.session.id()
    }

    fn scope_key(&self) -> ScopeKey {
        self.scope.clone()
    }

    fn options(&self) -> &AgentOptions {
        &self.options
    }

    fn session(&self) -> &Session {
        &self.session
    }

    fn model(&self) -> &dyn Model {
        &self.model
    }

    fn inbox(&self) -> &dyn Inbox {
        &self.inbox
    }

    fn status(&self) -> AgentStatus {
        AgentStatus::Idle
    }

    fn ctx(&self) -> &Context {
        &self.ctx
    }

    fn cancel(&self, _: AgentCancelCause, _: CancelOptions) -> Result<(), BoxError> {
        Ok(())
    }

    fn when_idle(&self) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        Box::pin(async {})
    }

    fn run_maintenance_erased(&self, _: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        Err("maintenance is not used by ask-user tests".into())
    }

    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
        Ok(())
    }

    fn followup(&self, _: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }

    fn steer(&self, _: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }

    fn inject(&self, _: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }
}

struct AgentFixture {
    live: Arc<LiveAgent>,
    tool: Arc<dyn hoh_tools::Agent>,
}

fn agent_in(stack: &Stack, id: &str, delegation_depth: u64) -> AgentFixture {
    let session = stack
        .sessions
        .create(
            &stack.ctx,
            Some(id.into()),
            CreateSessionOptions {
                meta: Some(json!({ "delegationDepth": delegation_depth })),
                ..CreateSessionOptions::default()
            },
        )
        .unwrap();
    let live = Arc::new(LiveAgent {
        session: session.clone(),
        ctx: stack.ctx.clone(),
        scope: ScopeKey::new(),
        options: AgentOptions::default(),
        model: UnusedModel,
        inbox: UnusedInbox,
    });
    let registered: Arc<dyn Agent> = live.clone();
    std::mem::forget(
        stack
            .ctx
            .get::<Agents>()
            .unwrap()
            .enter(&stack.ctx, registered)
            .unwrap(),
    );
    AgentFixture {
        tool: live.clone(),
        live,
    }
}

#[test]
fn config_defaults_to_blocking_legacy_mode() {
    assert_eq!(Config::default().mode, Mode::Legacy);
    assert_eq!(Config::default().timeout, 120);
}

#[test]
fn config_deserialization_accepts_json_integer_floats() {
    let config: Config =
        serde_json::from_value(json!({ "mode": "timed", "timeout": 120.0 })).unwrap();
    assert_eq!(config.timeout, 120);
}

#[test]
fn config_schema_preserves_source_domain_and_defaults() {
    let schema = serde_json::to_value(schemars::schema_for!(Config)).unwrap();
    assert_eq!(
        schema,
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "Config",
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "mode": { "enum": ["legacy", "timed"], "default": "legacy" },
                "timeout": {
                    "oneOf": [
                        { "const": -1 },
                        { "type": "integer", "minimum": 1, "maximum": 2_147_483 }
                    ],
                    "default": 120
                }
            }
        })
    );
}

#[tokio::test]
async fn plugin_config_is_validated_in_legacy_mode_and_accepts_integer_floats() {
    for timeout in [json!(0), json!(2_147_484)] {
        let (kernel, ctx) = mount_dependencies().await;
        let fiber = ctx
            .plugin::<ToolAskUser>(json!({ "mode": "legacy", "timeout": timeout }))
            .unwrap();
        let error = fiber
            .settled()
            .await
            .expect_err("invalid legacy timeout must fail before apply");
        assert!(error.to_string().contains("timeout"), "{error}");
        kernel.shutdown().await;
    }

    let (kernel, ctx) = mount_dependencies().await;
    let tool = ctx
        .plugin::<ToolAskUser>(json!({ "mode": "legacy", "timeout": 120.0 }))
        .unwrap();
    tool.settled().await.unwrap();
    kernel.shutdown().await;
}

#[tokio::test]
async fn public_apply_validates_legacy_config_too() {
    let stack = mount(json!({})).await;
    let error = apply(
        stack.ctx.clone(),
        (*stack.ctx.get::<Tools>().unwrap()).clone(),
        (*stack.ctx.get::<UserQuestions>().unwrap()).clone(),
        Config {
            mode: Mode::Legacy,
            timeout: 0,
        },
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "invalid arguments: timeout must be -1 or a positive integer up to 2147483 seconds"
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn default_plugin_registers_the_blocking_definition() {
    let stack = mount(json!({})).await;
    let tool = definition(&stack.ctx);
    assert_eq!(tool.name, "ask_user_question");
    assert_eq!(
        tool.description,
        "Ask the user a concise question when you need confirmation, a choice, or missing information before proceeding. An answer item with empty selected and no custom means the user skipped that question: continue without it, do not ask it again, and do not treat the skip as permission."
    );
    assert_eq!(tool.parameters["required"], json!(["questions"]));
    assert_eq!(
        tool.parameters["properties"]["questions"]["description"],
        "Questions to ask the user before continuing."
    );
    assert!(tool.parameters["properties"].get("timeout").is_none());
    let schemas = stack
        .ctx
        .get::<Tools>()
        .unwrap()
        .schemas(None, None)
        .unwrap();
    let schema = schemas
        .iter()
        .find(|schema| schema.name == "ask_user_question")
        .unwrap();
    assert!(!is_timed_ask_user_question_schema(schema));
    let events = [
        SessionEvent::from_value(json!({
            "type": "request/header",
            "seq": 0,
            "time": 0,
            "data": { "header": { "tools": schemas } }
        })),
        SessionEvent::from_value(json!({
            "type": "tool/call",
            "seq": 1,
            "time": 1,
            "data": {
                "callId": "ask-legacy",
                "name": "ask_user_question",
                "arguments": "{\"questions\":[{\"id\":\"continue\",\"question\":\"Continue?\"}]}"
            }
        })),
    ];
    assert_eq!(fold_user_questions(events.iter()), Default::default());
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn timed_mode_is_opt_in_and_describes_its_configured_default() {
    let stack = mount(json!({ "mode": "timed", "timeout": 45 })).await;
    let tool = definition(&stack.ctx);
    assert!(tool.parameters["properties"].get("timeout").is_some());
    assert_eq!(
        tool.parameters["properties"]["timeout"]["description"],
        "Wait seconds for the entire batch (default 45); omit unless the user specifies a duration. Use -1 only when an answer is required before proceeding."
    );
    assert_eq!(tool.parameters["properties"]["timeout"]["type"], "integer");
    assert!(tool.description.contains("A pending result instead"));
    assert_eq!(tool.parameters["required"], json!(["questions"]));
    let question = &tool.parameters["properties"]["questions"];
    assert_eq!(question["type"], "array");
    assert_eq!(question["description"], "Questions to ask the user.");
    assert_eq!(question["items"]["required"], json!(["id", "question"]));
    assert_eq!(
        question["items"]["properties"],
        json!({
            "id": {
                "type": "string",
                "description": "Stable id for this question; echoed in the answer."
            },
            "question": {
                "type": "string",
                "description": "The specific question to ask the user."
            },
            "header": {
                "type": "string",
                "description": "Optional short heading for the question, such as \"Confirm\" or \"Choose Mode\"."
            },
            "options": {
                "type": "array",
                "description": "Optional choices to show the user. If you recommend one, put it first and append \"(Recommended)\" to that label.",
                "items": {
                    "type": "object",
                    "additionalProperties": true,
                    "properties": {
                        "label": {
                            "type": "string",
                            "description": "Short user-facing option label."
                        },
                        "description": {
                            "type": "string",
                            "description": "One sentence explaining the tradeoff or impact."
                        }
                    },
                    "required": ["label"]
                }
            },
            "multi_select": {
                "type": "boolean",
                "description": "Whether the user may select more than one option. Defaults to false."
            }
        })
    );
    let option_properties = &question["items"]["properties"]["options"]["items"]["properties"];
    for absent in ["value", "recommended", "preview"] {
        assert!(option_properties.get(absent).is_none());
    }
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn timed_mode_uses_120_when_timeout_is_omitted() {
    let stack = mount(json!({ "mode": "timed" })).await;
    assert!(
        definition(&stack.ctx).parameters["properties"]["timeout"]["description"]
            .as_str()
            .unwrap()
            .contains("default 120")
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn legacy_simple_request_is_exactly_unkeyed_and_anonymous() {
    let stack = mount(json!({ "mode": "legacy" })).await;
    let observed = Arc::new(Mutex::new(None));
    let captured = observed.clone();
    stack
        .ctx
        .on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(move |request, _| {
            *captured.lock().unwrap() = Some((
                request.questions.clone(),
                request.agent.clone(),
                request.wait.clone(),
            ));
            async {
                Ok(AskUserQuestionAnswer {
                    answers: vec![AskUserQuestionAnswerItem {
                        id: "continue".into(),
                        selected: vec!["yes".into()],
                        custom: None,
                    }],
                })
            }
        })
        .unwrap();
    let result = execute(
        &stack.ctx,
        json!({ "questions": [{ "id": "continue", "question": "Continue?" }] }),
    )
    .await;
    assert_eq!(
        text(&result.content()[0]),
        "{\"answers\":[{\"id\":\"continue\",\"selected\":[\"yes\"]}]}"
    );
    let (questions, agent, wait) = observed.lock().unwrap().take().unwrap();
    assert_eq!(
        serde_json::to_value(questions).unwrap(),
        json!([{ "id": "continue", "question": "Continue?" }])
    );
    assert!(agent.is_none());
    assert!(wait.is_none());
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn legacy_maps_questions_and_answers_without_losing_optional_fields() {
    let stack = mount(json!({})).await;
    let observed = Arc::new(Mutex::new(None));
    let captured = observed.clone();
    stack
        .ctx
        .on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(move |request, _| {
            *captured.lock().unwrap() = Some((
                request.questions.clone(),
                request.agent.clone(),
                request.signal.clone(),
                request.wait.clone(),
            ));
            async {
                Ok(AskUserQuestionAnswer {
                    answers: vec![AskUserQuestionAnswerItem {
                        id: "deploy".into(),
                        selected: vec!["Staging".into(), "Canary".into()],
                        custom: Some("after lunch".into()),
                    }],
                })
            }
        })
        .unwrap();
    let controller = AbortController::new();
    let caller_signal = controller.signal();
    let result = execute_with(
        &stack.ctx,
        json!({
            "questions": [{
                "id": "deploy",
                "question": "Where should this go?",
                "header": "Choose target",
                "options": [{ "label": "Staging", "description": "Safer", "extra": true }],
                "multi_select": true,
                "extra": "accepted"
            }]
        }),
        None,
        caller_signal.clone(),
    )
    .await;
    assert_eq!(
        result.value(),
        Some(&json!({
            "answers": [{
                "id": "deploy",
                "selected": ["Staging", "Canary"],
                "custom": "after lunch"
            }]
        }))
    );
    assert_eq!(
        text(&result.content()[0]),
        "{\"answers\":[{\"id\":\"deploy\",\"selected\":[\"Staging\",\"Canary\"],\"custom\":\"after lunch\"}]}"
    );
    let (questions, request_agent, signal, wait) = observed.lock().unwrap().take().unwrap();
    assert_eq!(
        serde_json::to_value(&questions).unwrap(),
        json!([{
            "id": "deploy",
            "question": "Where should this go?",
            "header": "Choose target",
            "options": [{ "label": "Staging", "description": "Safer" }],
            "multiSelect": true
        }])
    );
    assert!(request_agent.is_none());
    let request_signal = signal.expect("request signal");
    assert!(!request_signal.aborted());
    assert!(wait.is_none());
    controller.abort();
    let caller_reason = caller_signal.reason().unwrap();
    let request_reason = request_signal.reason().unwrap();
    assert!(request_reason.ptr_eq(&caller_reason));
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn legacy_preserves_recommended_labels_and_resumed_root_identity() {
    let stack = mount(json!({})).await;
    let agent = agent_in(&stack, "resumed-root", 1);
    let observed = Arc::new(Mutex::new(None));
    let captured = observed.clone();
    stack
        .ctx
        .on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(move |request, _| {
            *captured.lock().unwrap() = Some((
                request.questions.clone(),
                request.agent.clone(),
                request.wait.clone(),
            ));
            async {
                Ok(AskUserQuestionAnswer {
                    answers: vec![AskUserQuestionAnswerItem {
                        id: "pkg".into(),
                        selected: vec![],
                        custom: Some("bun".into()),
                    }],
                })
            }
        })
        .unwrap();

    let result = execute_with(
        &stack.ctx,
        json!({
            "questions": [{
                "id": "pkg",
                "question": "Which package manager?",
                "header": "Setup",
                "multi_select": false,
                "options": [
                    { "label": "pnpm (Recommended)", "description": "Workspace default." },
                    { "label": "npm" }
                ]
            }]
        }),
        Some(agent.tool.clone()),
        AbortController::new().signal(),
    )
    .await;

    assert!(!result.is_error(), "{}", result.text());
    assert_eq!(
        text(&result.content()[0]),
        "{\"answers\":[{\"id\":\"pkg\",\"selected\":[],\"custom\":\"bun\"}]}"
    );
    let (questions, request_agent, wait) = observed.lock().unwrap().take().unwrap();
    assert_eq!(
        serde_json::to_value(&questions).unwrap(),
        json!([{
            "id": "pkg",
            "question": "Which package manager?",
            "header": "Setup",
            "options": [
                { "label": "pnpm (Recommended)", "description": "Workspace default." },
                { "label": "npm" }
            ],
            "multiSelect": false
        }])
    );
    let expected: Arc<dyn Agent> = agent.live;
    assert!(Arc::ptr_eq(request_agent.as_ref().unwrap(), &expected));
    assert!(wait.is_none());
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn legacy_keeps_a_skipped_answer_compact() {
    let stack = mount(json!({})).await;
    stack
        .ctx
        .on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(
            |request, _| async move {
                Ok(AskUserQuestionAnswer {
                    answers: vec![AskUserQuestionAnswerItem {
                        id: request.questions[0].id.clone(),
                        selected: vec![],
                        custom: None,
                    }],
                })
            },
        )
        .unwrap();
    let result = execute(
        &stack.ctx,
        json!({ "questions": [{ "id": "skip", "question": "Optional?" }] }),
    )
    .await;
    assert_eq!(
        result.value(),
        Some(&json!({ "answers": [{ "id": "skip", "selected": [] }] }))
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn legacy_projects_all_answer_shapes_to_exact_json_text() {
    let stack = mount(json!({})).await;
    stack
        .ctx
        .on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(|_, _| async {
            Ok(AskUserQuestionAnswer {
                answers: vec![
                    AskUserQuestionAnswerItem {
                        id: "targets".into(),
                        selected: vec!["tests".into(), "docs".into()],
                        custom: Some("release notes".into()),
                    },
                    AskUserQuestionAnswerItem {
                        id: "labels-only".into(),
                        selected: vec!["tests".into()],
                        custom: None,
                    },
                    AskUserQuestionAnswerItem {
                        id: "notes".into(),
                        selected: vec![],
                        custom: Some("ship today".into()),
                    },
                    AskUserQuestionAnswerItem {
                        id: "optional".into(),
                        selected: vec![],
                        custom: None,
                    },
                ],
            })
        })
        .unwrap();
    let result = execute(
        &stack.ctx,
        json!({
            "questions": [
                { "id": "targets", "question": "What should I update?" },
                { "id": "labels-only", "question": "Which labels?" },
                { "id": "notes", "question": "Any note?" },
                { "id": "optional", "question": "Anything else?" }
            ]
        }),
    )
    .await;
    assert_eq!(
        text(&result.content()[0]),
        "{\"answers\":[{\"id\":\"targets\",\"selected\":[\"tests\",\"docs\"],\"custom\":\"release notes\"},{\"id\":\"labels-only\",\"selected\":[\"tests\"]},{\"id\":\"notes\",\"selected\":[],\"custom\":\"ship today\"},{\"id\":\"optional\",\"selected\":[]}]}"
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn timed_indefinite_wait_keys_the_request_by_call_id() {
    let stack = mount(json!({ "mode": "timed" })).await;
    let observed = Arc::new(Mutex::new(None));
    let captured = observed.clone();
    stack
        .ctx
        .on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(move |request, _| {
            *captured.lock().unwrap() = Some((request.wait.clone(), request.signal.clone()));
            async move {
                Ok(AskUserQuestionAnswer {
                    answers: vec![AskUserQuestionAnswerItem {
                        id: request.questions[0].id.clone(),
                        selected: vec!["Yes".into()],
                        custom: None,
                    }],
                })
            }
        })
        .unwrap();
    let controller = AbortController::new();
    let caller_signal = controller.signal();
    let result = execute_with(
        &stack.ctx,
        json!({
            "questions": [{ "id": "confirm", "question": "Proceed?" }],
            "timeout": -1
        }),
        None,
        caller_signal.clone(),
    )
    .await;
    assert_eq!(
        result.value(),
        Some(&json!({ "answers": [{ "id": "confirm", "selected": ["Yes"] }] }))
    );
    let (wait, request_signal) = observed.lock().unwrap().take().unwrap();
    let wait = wait.unwrap();
    assert_eq!(wait.call_id, ToolCallId::from("ask-1"));
    assert_eq!(wait.timed, None);
    let request_signal = request_signal.unwrap();
    controller.abort();
    assert!(
        request_signal
            .reason()
            .unwrap()
            .ptr_eq(&caller_signal.reason().unwrap())
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn timed_integer_float_returns_an_in_time_answer_with_a_live_agent() {
    let stack = mount(json!({ "mode": "timed", "timeout": 30 })).await;
    let agent = agent_in(&stack, "timed-in-time", 0);
    let observed = Arc::new(Mutex::new(None));
    let captured = observed.clone();
    stack
        .ctx
        .on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(move |request, _| {
            *captured.lock().unwrap() = request.wait.clone();
            async {
                Ok(AskUserQuestionAnswer {
                    answers: vec![AskUserQuestionAnswerItem {
                        id: "continue".into(),
                        selected: vec!["yes".into()],
                        custom: None,
                    }],
                })
            }
        })
        .unwrap();
    let result = execute_with(
        &stack.ctx,
        json!({
            "questions": [{ "id": "continue", "question": "Continue?" }],
            "timeout": 3.0
        }),
        Some(agent.tool),
        AbortController::new().signal(),
    )
    .await;
    assert!(!result.is_error(), "{}", result.text());
    assert_eq!(
        text(&result.content()[0]),
        "{\"answers\":[{\"id\":\"continue\",\"selected\":[\"yes\"]}]}"
    );
    let wait = observed.lock().unwrap().take().unwrap();
    assert_eq!(wait.call_id, ToolCallId::from("ask-1"));
    assert_eq!(wait.timed, Some(true));
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn timed_positive_wait_requires_a_live_agent() {
    let stack = mount(json!({ "mode": "timed" })).await;
    let result = execute(
        &stack.ctx,
        json!({ "questions": [{ "id": "q", "question": "Choose?" }], "timeout": 1 }),
    )
    .await;
    assert!(result.is_error());
    assert_eq!(
        result.error().unwrap().message,
        "timed questions require a live agent"
    );
    assert_eq!(
        error_info(&result),
        Some(("ToolContextUnavailableError", "TOOL_CONTEXT_UNAVAILABLE"))
    );
    assert_eq!(result.text(), "Error: timed questions require a live agent");
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn unregistered_agent_matches_user_questions_caller_not_live_error() {
    let stack = mount(json!({})).await;
    let session = stack
        .sessions
        .create(
            &stack.ctx,
            Some("unregistered".into()),
            CreateSessionOptions::default(),
        )
        .unwrap();
    let tool_agent: Arc<dyn hoh_tools::Agent> = Arc::new(LiveAgent {
        session,
        ctx: stack.ctx.clone(),
        scope: ScopeKey::new(),
        options: AgentOptions::default(),
        model: UnusedModel,
        inbox: UnusedInbox,
    });
    let result = execute_with(
        &stack.ctx,
        json!({ "questions": [{ "id": "q", "question": "Choose?" }] }),
        Some(tool_agent),
        AbortController::new().signal(),
    )
    .await;
    assert_eq!(
        result.error().unwrap().message,
        "human interaction requires the exact live calling agent when an agent is supplied"
    );
    assert_eq!(
        error_info(&result),
        Some(("UserQuestionError", "CALLER_NOT_LIVE"))
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn zero_timeout_is_rejected_by_runtime_validation() {
    let stack = mount(json!({ "mode": "timed" })).await;
    let result = execute(
        &stack.ctx,
        json!({ "questions": [{ "id": "q", "question": "Choose?" }], "timeout": 0 }),
    )
    .await;
    assert_eq!(
        result.error().unwrap().message,
        "invalid arguments: timeout must be -1 or a positive integer up to 2147483 seconds"
    );
    assert_eq!(error_info(&result), Some(("ToolArgsError", "INVALID_ARGS")));
    assert!(result.text().to_lowercase().contains("timeout"));
    assert!(result.text().to_lowercase().contains("integer"));
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn oversized_timeout_is_rejected_by_runtime_validation() {
    let stack = mount(json!({ "mode": "timed" })).await;
    let result = execute(
        &stack.ctx,
        json!({ "questions": [{ "id": "q", "question": "Choose?" }], "timeout": 2_147_484 }),
    )
    .await;
    assert_eq!(
        result.error().unwrap().message,
        "invalid arguments: timeout must be -1 or a positive integer up to 2147483 seconds"
    );
    assert_eq!(error_info(&result), Some(("ToolArgsError", "INVALID_ARGS")));
    assert!(result.text().to_lowercase().contains("timeout"));
    assert!(result.text().to_lowercase().contains("integer"));
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn fractional_timeout_uses_the_timeout_specific_runtime_error() {
    let stack = mount(json!({ "mode": "timed" })).await;
    let result = execute(
        &stack.ctx,
        json!({ "questions": [{ "id": "q", "question": "Choose?" }], "timeout": 1.5 }),
    )
    .await;
    assert!(result.is_error());
    assert_eq!(
        result.error().unwrap().message,
        "invalid arguments: timeout must be -1 or a positive integer up to 2147483 seconds"
    );
    assert_eq!(error_info(&result), Some(("ToolArgsError", "INVALID_ARGS")));
    assert!(result.text().to_lowercase().contains("timeout"));
    assert!(result.text().to_lowercase().contains("integer"));
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn explicit_null_and_non_numeric_timeouts_are_not_treated_as_missing() {
    let stack = mount(json!({ "mode": "timed", "timeout": -1 })).await;
    let calls = Arc::new(Mutex::new(0));
    let observed = calls.clone();
    stack
        .ctx
        .on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(move |_, _| {
            *observed.lock().unwrap() += 1;
            async {
                Ok(AskUserQuestionAnswer {
                    answers: vec![AskUserQuestionAnswerItem {
                        id: "q".into(),
                        selected: vec!["yes".into()],
                        custom: None,
                    }],
                })
            }
        })
        .unwrap();

    for timeout in [Value::Null, json!("5"), json!(true)] {
        let result = execute(
            &stack.ctx,
            json!({
                "questions": [{ "id": "q", "question": "Continue?" }],
                "timeout": timeout
            }),
        )
        .await;
        assert_eq!(
            result.error().unwrap().message,
            "invalid arguments: timeout must be -1 or a positive integer up to 2147483 seconds"
        );
        assert_eq!(error_info(&result), Some(("ToolArgsError", "INVALID_ARGS")));
    }
    assert_eq!(*calls.lock().unwrap(), 0);
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn oversized_integer_floats_use_the_timeout_specific_error() {
    let stack = mount(json!({ "mode": "timed" })).await;
    for timeout in [json!(2_147_484.0), json!(1e21)] {
        let result = execute(
            &stack.ctx,
            json!({
                "questions": [{ "id": "q", "question": "Choose?" }],
                "timeout": timeout
            }),
        )
        .await;
        assert_eq!(
            result.error().unwrap().message,
            "invalid arguments: timeout must be -1 or a positive integer up to 2147483 seconds"
        );
        assert_eq!(error_info(&result), Some(("ToolArgsError", "INVALID_ARGS")));
    }
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn duplicate_question_ids_fail_before_dispatching_to_a_provider() {
    let stack = mount(json!({ "mode": "timed" })).await;
    let calls = Arc::new(Mutex::new(0));
    let observed = calls.clone();
    stack
        .ctx
        .on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(move |_, _| {
            *observed.lock().unwrap() += 1;
            async { unreachable!() }
        })
        .unwrap();
    let result = execute(
        &stack.ctx,
        json!({
            "questions": [
                { "id": "scope", "question": "First?" },
                { "id": "scope", "question": "Second?" }
            ],
            "timeout": -1
        }),
    )
    .await;
    assert_eq!(*calls.lock().unwrap(), 0);
    assert_eq!(
        result.text(),
        "Error: invalid arguments: question id \"scope\" must be unique within this call"
    );
    assert_eq!(error_info(&result), Some(("ToolArgsError", "INVALID_ARGS")));
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn separate_calls_may_reuse_the_same_question_id() {
    let stack = mount(json!({ "mode": "timed" })).await;
    stack
        .ctx
        .on_waterfall::<AskUserQuestionRequestEvent, AskUserQuestionAnswer, _>(
            |request, _| async move {
                Ok(AskUserQuestionAnswer {
                    answers: vec![AskUserQuestionAnswerItem {
                        id: request.questions[0].id.clone(),
                        selected: vec!["ok".into()],
                        custom: None,
                    }],
                })
            },
        )
        .unwrap();
    let args = json!({
        "questions": [{ "id": "same", "question": "Again?" }],
        "timeout": -1
    });
    assert!(!execute(&stack.ctx, args.clone()).await.is_error());
    assert!(!execute(&stack.ctx, args).await.is_error());
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn missing_provider_stays_a_structured_tool_failure() {
    let stack = mount(json!({})).await;
    let result = execute(
        &stack.ctx,
        json!({ "questions": [{ "id": "q", "question": "Anyone?" }] }),
    )
    .await;
    assert!(result.is_error());
    assert_eq!(
        result.error().unwrap().message,
        "no user-questions answerer accepted the request"
    );
    assert_eq!(
        error_info(&result),
        Some(("UserQuestionError", "NO_PROVIDER"))
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn empty_question_batches_stay_a_structured_tool_failure() {
    let stack = mount(json!({})).await;
    let result = execute(&stack.ctx, json!({ "questions": [] })).await;
    assert!(result.is_error());
    assert_eq!(
        result.error().unwrap().message,
        "ask_user_question requires at least one question"
    );
    assert_eq!(
        error_info(&result),
        Some(("UserQuestionError", "EMPTY_QUESTIONS"))
    );
    stack.kernel.shutdown().await;
}

#[tokio::test]
async fn disposing_the_plugin_unregisters_the_tool() {
    let stack = mount(json!({})).await;
    assert!(
        stack
            .ctx
            .get::<Tools>()
            .unwrap()
            .get("ask_user_question", None, None)
            .is_some()
    );
    stack.tool.disposer().unwrap().dispose().await;
    assert!(
        stack
            .ctx
            .get::<Tools>()
            .unwrap()
            .get("ask_user_question", None, None)
            .is_none()
    );
    stack.kernel.shutdown().await;
}
