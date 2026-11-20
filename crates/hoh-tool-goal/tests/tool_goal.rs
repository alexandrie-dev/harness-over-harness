use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentOptions, AgentStatus, Agents, CancelOptions, Inbox, InboxTarget, MaintenanceFuture,
    MaintenanceTask, Model,
};
use hoh_agent_loop_testkit::{
    create_inbox_stub, mount_agent_loop_test_dependencies, unsupported_model,
};
use hoh_error::BoxError;
use hoh_goal::{CreateGoalRequest, GoalActivation, GoalError, GoalPhase, GoalServicePlugin, Goals};
use hoh_kernel::{Context, Fiber, Kernel};
use hoh_llm::{
    ContentBlock, MessageSource, NewUserMessage, ToolCallId, UserMessage, create_user_message,
};
use hoh_scope::ScopeKey;
use hoh_session::{
    AgentCancelCause, CreateSessionOptions, Session, SessionId, SessionLogOffset, Sessions,
    SurfaceIntent,
};
use hoh_system_prompt::{AssembleContext, SystemPromptKey};
use hoh_timeout::AbortController;
use hoh_tool_goal::{Config, INJECT, NAME, ToolGoal, apply};
use hoh_tool_testkit::execute_tool_fixture;
use hoh_tools::{
    ToolCallKind, ToolCallView, ToolExecutionInput, ToolExecutionMode, ToolExecutionPipeline,
    ToolExecutionResult, Tools,
};
use serde_json::{Map, Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicU64, Ordering},
};

static NEXT_ENV: AtomicU64 = AtomicU64::new(1);
static NEXT_CALL: AtomicU64 = AtomicU64::new(1);

struct StubAgent {
    id: SessionId,
    session: Session,
    ctx: Context,
    options: AgentOptions,
    scope: ScopeKey,
    model: Arc<dyn Model>,
    inbox: Arc<dyn Inbox>,
    status: AtomicU8,
}

impl StubAgent {
    fn set_status(&self, status: AgentStatus) {
        self.status.store(
            match status {
                AgentStatus::Idle => 0,
                AgentStatus::Running => 1,
            },
            Ordering::SeqCst,
        );
    }
}

impl Agent for StubAgent {
    fn id(&self) -> &SessionId {
        &self.id
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
        self.model.as_ref()
    }
    fn inbox(&self) -> &dyn Inbox {
        self.inbox.as_ref()
    }
    fn status(&self) -> AgentStatus {
        match self.status.load(Ordering::SeqCst) {
            0 => AgentStatus::Idle,
            1 => AgentStatus::Running,
            _ => AgentStatus::Idle,
        }
    }
    fn ctx(&self) -> &Context {
        &self.ctx
    }
    fn cancel(&self, _: AgentCancelCause, _: CancelOptions) -> Result<(), BoxError> {
        Ok(())
    }
    fn when_idle(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance_erased(&self, task: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        Ok(task(AbortController::new().signal()))
    }
    fn send(&self, message: UserMessage, target: InboxTarget, _: bool) -> Result<(), BoxError> {
        self.inbox.append(target, message)
    }
    fn followup(&self, message: UserMessage) -> Result<(), BoxError> {
        self.inbox.append(InboxTarget::NextTurn, message)
    }
    fn steer(&self, message: UserMessage) -> Result<(), BoxError> {
        self.inbox.append(InboxTarget::NextStep, message)
    }
    fn inject(&self, message: UserMessage) -> Result<(), BoxError> {
        self.inbox.append(InboxTarget::NextStep, message)
    }
}

struct Harness {
    kernel: Kernel,
    ctx: Context,
    agent: Arc<StubAgent>,
    agent_dyn: Arc<dyn Agent>,
    tool_fiber: Fiber,
    turn: u64,
}

impl Harness {
    async fn new(blocked_after: u64) -> Self {
        Self::with_origin(blocked_after, None).await
    }

    async fn with_origin(blocked_after: u64, origin: Option<&str>) -> Self {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        mount_agent_loop_test_dependencies(&ctx, Default::default())
            .await
            .expect("test dependencies");
        let goal_fiber = ctx
            .plugin::<GoalServicePlugin>(json!({}))
            .expect("goal plugin");
        goal_fiber.settled().await.expect("goal settles");
        let tool_fiber = ctx
            .plugin::<ToolGoal>(json!({
                "blockedAfterConsecutiveRounds": blocked_after
            }))
            .expect("tool plugin");
        tool_fiber.settled().await.expect("tool settles");
        let serial = NEXT_ENV.fetch_add(1, Ordering::Relaxed);
        let sessions = ctx.get::<Sessions>().expect("sessions");
        let session_options = CreateSessionOptions {
            meta: origin.map(|origin| json!({ "origin": origin })),
            ..CreateSessionOptions::default()
        };
        let session = sessions
            .create(
                &ctx,
                Some(SessionId::from(format!("goal-tool-{serial}"))),
                session_options,
            )
            .expect("session");
        let agent = Arc::new(StubAgent {
            id: session.id().clone(),
            session,
            ctx: ctx.clone(),
            options: AgentOptions::default(),
            scope: ScopeKey::new(),
            model: unsupported_model(),
            inbox: create_inbox_stub(),
            status: AtomicU8::new(1),
        });
        let agent_dyn: Arc<dyn Agent> = agent.clone();
        ctx.get::<Agents>()
            .expect("agents")
            .register(&ctx, agent_dyn.clone())
            .await
            .expect("register agent");
        Self {
            kernel,
            ctx,
            agent,
            agent_dyn,
            tool_fiber,
            turn: 0,
        }
    }

    fn source(kind: &str) -> MessageSource {
        MessageSource::new(kind)
    }

    fn goal_source(id: &str, revision: u64, round: u64) -> MessageSource {
        MessageSource {
            kind: "goal".into(),
            data: Map::from_iter([
                ("goalId".into(), Value::String(id.into())),
                ("revision".into(), Value::from(revision)),
                ("round".into(), Value::from(round)),
            ]),
        }
    }

    fn open_turn(&mut self, source: MessageSource) {
        self.turn += 1;
        self.agent
            .session
            .append("turn/start", json!({ "turn": self.turn }), None)
            .expect("turn start");
        let message = create_user_message(NewUserMessage {
            content: vec![ContentBlock::text("prompt")],
            source,
        });
        self.agent
            .session
            .append(
                "user/message",
                serde_json::to_value(message).expect("message JSON"),
                Some(SurfaceIntent::append()),
            )
            .expect("user message");
        self.agent
            .session
            .append("step/start", json!({ "turn": self.turn, "step": 1 }), None)
            .expect("step start");
    }

    fn add_user_message(&self) {
        let message = create_user_message(NewUserMessage {
            content: vec![ContentBlock::text("human steering")],
            source: Self::source("user"),
        });
        self.agent
            .session
            .append(
                "user/message",
                serde_json::to_value(message).expect("message JSON"),
                Some(SurfaceIntent::append()),
            )
            .expect("user message");
    }

    fn close_turn(&self) {
        self.agent
            .session
            .append("step/end", json!({ "turn": self.turn, "step": 1 }), None)
            .expect("step end");
        self.agent
            .session
            .append(
                "turn/end",
                json!({ "turn": self.turn, "reason": { "kind": "completed" } }),
                None,
            )
            .expect("turn end");
    }

    fn goals(&self) -> hoh_kernel::Service<Goals> {
        self.ctx.get::<Goals>().expect("goals")
    }

    async fn execute_as(
        &self,
        name: &str,
        arguments: Value,
        agent: Option<Arc<dyn Agent>>,
        initiator: Option<Arc<dyn Agent>>,
    ) -> ToolExecutionResult {
        let mut input = ToolExecutionInput::new(
            ToolCallId::from(format!(
                "call-{}",
                NEXT_CALL.fetch_add(1, Ordering::Relaxed)
            )),
            name,
            arguments,
            AbortController::new().signal(),
        );
        input.agent = agent;
        let ctx = self.ctx.clone();
        let run = async move {
            execute_tool_fixture(&ctx, input)
                .await
                .expect("tool fixture")
        };
        match initiator {
            Some(agent) => {
                self.ctx
                    .get::<Agents>()
                    .expect("agents")
                    .with_initiator(agent, run)
                    .expect("initiator scope")
                    .await
            }
            None => run.await,
        }
    }

    async fn execute(&self, name: &str, arguments: Value) -> ToolExecutionResult {
        self.execute_as(
            name,
            arguments,
            Some(self.agent_dyn.clone()),
            Some(self.agent_dyn.clone()),
        )
        .await
    }

    async fn close(self) {
        self.kernel.shutdown().await;
    }
}

fn code(result: &ToolExecutionResult) -> Option<&str> {
    result
        .error()
        .and_then(|failure| failure.info.as_ref())
        .map(|info| info.code.as_str())
}

fn goal(result: &ToolExecutionResult) -> &Value {
    result
        .value()
        .unwrap_or_else(|| panic!("success value, got {result:?}"))
        .get("goal")
        .expect("goal")
}

#[tokio::test]
async fn registers_three_tools_guidance_and_disposes_contributions() {
    let h = Harness::new(5).await;
    let tools = h.ctx.get::<Tools>().unwrap();
    for name in ["get_goal", "create_goal", "update_goal"] {
        assert!(tools.get(name, None, None).is_some(), "missing {name}");
        let input = ToolExecutionInput::new(
            ToolCallId::from(format!("mode-{name}")),
            name,
            json!({}),
            AbortController::new().signal(),
        );
        assert_eq!(
            ToolExecutionPipeline::new(h.ctx.clone())
                .unwrap()
                .execution_mode(&input)
                .unwrap(),
            ToolExecutionMode::Exclusive
        );
    }
    let prompt = h.ctx.get::<SystemPromptKey>().unwrap();
    let assembly = prompt.assemble(AssembleContext::default()).await.unwrap();
    let section = assembly
        .sections
        .iter()
        .find(|section| section.name == "tool:goal")
        .unwrap();
    assert!(section.text.contains("at least 5 consecutive goal rounds"));
    h.tool_fiber.dispose().await;
    assert!(tools.get("get_goal", None, None).is_none());
    assert!(
        prompt
            .assemble(AssembleContext::default())
            .await
            .unwrap()
            .sections
            .iter()
            .all(|section| section.name != "tool:goal")
    );
    h.close().await;
}

#[tokio::test]
async fn exposes_source_plugin_name_and_default_config() {
    assert_eq!(NAME, "tool-goal");
    assert_eq!(INJECT, ["agents", "goals", "tools", "systemPrompt"]);
    assert_eq!(Config::default().blocked_after_consecutive_rounds, 3);
    let schema = serde_json::to_value(schemars::schema_for!(Config)).unwrap();
    let threshold = &schema["properties"]["blockedAfterConsecutiveRounds"];
    assert_eq!(threshold["minimum"], 1);
    assert_eq!(threshold["maximum"], 9_007_199_254_740_991_u64);
    assert_eq!(threshold["default"], 3);
}

#[tokio::test]
async fn rejects_zero_block_threshold_before_tool_registration() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    mount_agent_loop_test_dependencies(&ctx, Default::default())
        .await
        .unwrap();
    let goal = ctx.plugin::<GoalServicePlugin>(json!({})).unwrap();
    goal.settled().await.unwrap();
    let tool = ctx
        .plugin::<ToolGoal>(json!({ "blockedAfterConsecutiveRounds": 0 }))
        .unwrap();
    let error = tool.settled().await.expect_err("invalid config must fail");
    let message = error.to_string();
    assert!(message.contains("expected number >= 1"), "{message}");
    assert!(
        message.contains("blockedAfterConsecutiveRounds"),
        "{message}"
    );
    assert!(
        ctx.get::<Tools>()
            .unwrap()
            .get("get_goal", None, None)
            .is_none()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn direct_apply_rejects_invalid_config_before_registering_anything() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    mount_agent_loop_test_dependencies(&ctx, Default::default())
        .await
        .unwrap();
    let goal = ctx.plugin::<GoalServicePlugin>(json!({})).unwrap();
    goal.settled().await.unwrap();
    let agents = (*ctx.get::<Agents>().unwrap()).clone();
    let goals = (*ctx.get::<Goals>().unwrap()).clone();
    let tools = (*ctx.get::<Tools>().unwrap()).clone();
    let prompt = ctx.get::<SystemPromptKey>().unwrap();
    let error = apply(
        ctx.clone(),
        agents,
        goals,
        tools,
        &prompt,
        Config {
            blocked_after_consecutive_rounds: 0,
        },
    )
    .expect_err("invalid direct config");
    assert_eq!(
        error.to_string(),
        "blockedAfterConsecutiveRounds must be a positive safe integer"
    );
    assert!(
        ctx.get::<Tools>()
            .unwrap()
            .get("get_goal", None, None)
            .is_none()
    );
    assert!(
        prompt
            .assemble(AssembleContext::default())
            .await
            .unwrap()
            .sections
            .iter()
            .all(|section| section.name != "tool:goal")
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn direct_apply_materializes_the_default_before_registration() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    mount_agent_loop_test_dependencies(&ctx, Default::default())
        .await
        .unwrap();
    let goal = ctx.plugin::<GoalServicePlugin>(json!({})).unwrap();
    goal.settled().await.unwrap();
    apply(
        ctx.clone(),
        (*ctx.get::<Agents>().unwrap()).clone(),
        (*ctx.get::<Goals>().unwrap()).clone(),
        (*ctx.get::<Tools>().unwrap()).clone(),
        &ctx.get::<SystemPromptKey>().unwrap(),
        Config::default(),
    )
    .unwrap();
    let assembly = ctx
        .get::<SystemPromptKey>()
        .unwrap()
        .assemble(AssembleContext::default())
        .await
        .unwrap();
    assert!(
        assembly
            .sections
            .iter()
            .any(|section| section.name == "tool:goal"
                && section.text.contains("at least 3 consecutive goal rounds"))
    );
    assert!(
        ctx.get::<Tools>()
            .unwrap()
            .get("get_goal", None, None)
            .is_some()
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn presents_args_only_generic_cards_and_soft_fails_malformed_replay() {
    let h = Harness::new(3).await;
    let tools = h.ctx.get::<Tools>().unwrap();
    let get = tools.get("get_goal", None, None).unwrap();
    let view = get.present_call.as_ref().unwrap()(&json!({})).unwrap();
    assert_eq!(
        view,
        ToolCallView::Generic(hoh_tools::GenericCallView {
            title: "Read current goal".into(),
            kind: Some(ToolCallKind::Read),
            raw_input: None,
            content: None,
            locations: None,
        })
    );
    let create = tools.get("create_goal", None, None).unwrap();
    let ToolCallView::Generic(create) =
        create.present_call.as_ref().unwrap()(&json!({ "objective": "ship" })).unwrap()
    else {
        panic!("generic view")
    };
    assert_eq!(create.title, "Create goal");
    assert_eq!(create.kind, Some(ToolCallKind::Other));
    assert_eq!(create.raw_input, Some(json!("ship")));
    let update = tools.get("update_goal", None, None).unwrap();
    let blocked = update.present_call.as_ref().unwrap()(&json!({
        "goal_id": "goal-1", "revision": 2, "action": "blocked",
        "blocked_reason": "Waiting for a human choice."
    }))
    .unwrap();
    let ToolCallView::Generic(blocked) = blocked else {
        panic!("generic view")
    };
    assert_eq!(blocked.title, "Mark goal");
    assert_eq!(
        blocked.raw_input,
        Some(json!("Waiting for a human choice."))
    );
    for (arguments, title, raw_input) in [
        (
            json!({
                "goal_id": "goal-1", "revision": 2, "action": "edit",
                "objective": "ship", "max_goal_rounds": 0, "blocked_reason": ""
            }),
            "Edit goal",
            json!("ship"),
        ),
        (
            json!({
                "goal_id": "goal-1", "revision": 2, "action": "edit",
                "objective": "", "max_goal_rounds": 8, "blocked_reason": ""
            }),
            "Edit goal",
            json!(8),
        ),
        (
            json!({
                "goal_id": "goal-1", "revision": 2, "action": "resume",
                "objective": "ignored", "max_goal_rounds": 12,
                "blocked_reason": "ignored"
            }),
            "Resume goal",
            json!("goal-1"),
        ),
        (
            json!({
                "goal_id": "goal-1", "revision": 2, "action": "pause",
                "objective": "ignored", "max_goal_rounds": 12,
                "blocked_reason": "ignored"
            }),
            "Pause goal",
            json!("goal-1"),
        ),
        (
            json!({
                "goal_id": "goal-1", "revision": 2, "action": "complete",
                "objective": "ignored", "max_goal_rounds": 12,
                "blocked_reason": "ignored"
            }),
            "Complete goal",
            json!("goal-1"),
        ),
        (
            json!({
                "goal_id": "goal-1", "revision": 2, "action": "edit",
                "objective": "", "max_goal_rounds": 0,
                "blocked_reason": "ignored"
            }),
            "Edit goal",
            json!("goal-1"),
        ),
        (
            json!({
                "goal_id": "goal-1", "revision": 2, "action": "blocked",
                "blocked_reason": ""
            }),
            "Mark goal",
            json!("goal-1"),
        ),
    ] {
        let ToolCallView::Generic(view) =
            update.present_call.as_ref().unwrap()(&arguments).unwrap()
        else {
            panic!("generic view")
        };
        assert_eq!(view.title, title);
        assert_eq!(view.kind, Some(ToolCallKind::Other));
        assert_eq!(view.raw_input, Some(raw_input));
    }
    assert!(update.present_call.as_ref().unwrap()(&json!({ "wrong": true })).is_none());
    h.close().await;
}

#[tokio::test]
async fn lets_root_model_create_from_an_accepted_human_turn() {
    let mut h = Harness::new(3).await;
    h.open_turn(Harness::source("user"));
    let result = h
        .execute(
            "create_goal",
            json!({ "objective": "Finish the feature", "max_goal_rounds": 9 }),
        )
        .await;
    assert_eq!(goal(&result)["objective"], "Finish the feature");
    assert_eq!(goal(&result)["revision"], 1);
    assert_eq!(goal(&result)["maxGoalRounds"], 9);
    assert_eq!(result.value().unwrap()["activation"], "armed");
    h.close().await;
}

#[tokio::test]
async fn rejects_direct_human_creation_from_a_live_subagent_session() {
    let mut h = Harness::with_origin(3, Some("subagent")).await;
    h.open_turn(Harness::source("user"));
    let result = h
        .execute("create_goal", json!({ "objective": "child goal" }))
        .await;
    assert_eq!(code(&result), Some("GOAL_TOOL_AUTHORITY_REQUIRED"));
    assert!(h.goals().get(&h.agent_dyn).unwrap().is_none());
    h.close().await;
}

#[tokio::test]
async fn rejects_agentless_non_human_and_idle_calls() {
    let mut h = Harness::new(3).await;
    let agentless = h.execute_as("get_goal", json!({}), None, None).await;
    assert_eq!(code(&agentless), Some("GOAL_TOOL_AGENT_REQUIRED"));
    h.open_turn(Harness::source("user"));
    h.close_turn();
    h.open_turn(Harness::source("test"));
    let non_human = h
        .execute("create_goal", json!({ "objective": "forged" }))
        .await;
    assert_eq!(code(&non_human), Some("GOAL_TOOL_AUTHORITY_REQUIRED"));
    h.agent.set_status(AgentStatus::Idle);
    let idle = h.execute("get_goal", json!({})).await;
    assert_eq!(code(&idle), Some("GOAL_TOOL_DRIVER_REQUIRED"));
    h.close().await;
}

#[tokio::test]
async fn rejects_terminal_reporting_without_human_or_current_goal_round_authority() {
    let mut h = Harness::new(3).await;
    h.open_turn(Harness::source("test"));
    let complete = h
        .execute(
            "update_goal",
            json!({ "goal_id": "goal-missing", "revision": 1, "action": "complete" }),
        )
        .await;
    assert_eq!(code(&complete), Some("GOAL_TOOL_AUTHORITY_REQUIRED"));
    let pause = h
        .execute(
            "update_goal",
            json!({
                "goal_id": "goal-missing", "revision": 1,
                "action": "pause", "objective": "probe"
            }),
        )
        .await;
    assert_eq!(code(&pause), Some("GOAL_TOOL_AUTHORITY_REQUIRED"));
    h.close().await;
}

#[tokio::test]
async fn rejects_calls_before_and_after_an_open_turn() {
    let mut h = Harness::new(3).await;
    assert_eq!(
        code(&h.execute("get_goal", json!({})).await),
        Some("GOAL_TOOL_DRIVER_REQUIRED")
    );
    h.open_turn(Harness::source("user"));
    h.close_turn();
    assert_eq!(
        code(&h.execute("get_goal", json!({})).await),
        Some("GOAL_TOOL_DRIVER_REQUIRED")
    );
    h.close().await;
}

#[tokio::test]
async fn rejects_after_step_end_even_while_the_turn_remains_open() {
    let mut h = Harness::new(3).await;
    h.open_turn(Harness::source("user"));
    h.agent
        .session
        .append("step/end", json!({ "turn": h.turn, "step": 1 }), None)
        .unwrap();
    let result = h.execute("get_goal", json!({})).await;
    assert_eq!(code(&result), Some("GOAL_TOOL_DRIVER_REQUIRED"));
    h.close().await;
}

#[tokio::test]
async fn rejects_a_stale_agent_object_over_the_live_session() {
    let mut h = Harness::new(3).await;
    h.open_turn(Harness::source("user"));
    let stale: Arc<dyn Agent> = Arc::new(StubAgent {
        id: h.agent.id.clone(),
        session: h.agent.session.clone(),
        ctx: h.ctx.clone(),
        options: AgentOptions::default(),
        scope: ScopeKey::new(),
        model: unsupported_model(),
        inbox: create_inbox_stub(),
        status: AtomicU8::new(1),
    });
    let result = h
        .execute_as("get_goal", json!({}), Some(stale.clone()), Some(stale))
        .await;
    assert_eq!(code(&result), Some("GOAL_TOOL_DRIVER_REQUIRED"));
    h.close().await;
}

#[tokio::test]
async fn does_not_carry_human_authority_into_a_later_turn() {
    let mut h = Harness::new(3).await;
    h.open_turn(Harness::source("user"));
    h.close_turn();
    h.open_turn(Harness::source("test"));
    let result = h
        .execute("create_goal", json!({ "objective": "unauthorized" }))
        .await;
    assert_eq!(code(&result), Some("GOAL_TOOL_AUTHORITY_REQUIRED"));
    assert!(h.goals().get(&h.agent_dyn).unwrap().is_none());
    h.close().await;
}

#[tokio::test]
async fn reconstructs_current_turn_authority_when_mounted_after_input() {
    let mut h = Harness::new(3).await;
    h.tool_fiber.dispose().await;
    h.open_turn(Harness::source("user"));
    let tool = h.ctx.plugin::<ToolGoal>(json!({})).unwrap();
    tool.settled().await.unwrap();
    let result = h
        .execute(
            "create_goal",
            json!({ "objective": "accepted before tool load" }),
        )
        .await;
    assert_eq!(goal(&result)["objective"], "accepted before tool load");
    h.close().await;
}

#[tokio::test]
async fn a_fork_resumed_as_a_runtime_root_has_direct_human_authority() {
    let mut h = Harness::new(3).await;
    h.open_turn(Harness::source("user"));
    let created = h
        .goals()
        .create(
            &h.agent_dyn,
            CreateGoalRequest {
                objective: "resume the fork".into(),
                max_goal_rounds: None,
            },
        )
        .unwrap();
    h.close_turn();
    let inherited = SessionLogOffset::new(h.agent.session.seq().get()).unwrap();
    let fork_session = h
        .ctx
        .get::<Sessions>()
        .unwrap()
        .create(
            &h.ctx,
            Some(SessionId::from(format!(
                "goal-tool-fork-{}",
                NEXT_ENV.fetch_add(1, Ordering::Relaxed)
            ))),
            CreateSessionOptions {
                seed: Some(h.agent.session.snapshot_events(None, None).to_vec()),
                inherited_event_count: Some(inherited),
                meta: Some(json!({
                    "parentSession": h.agent.session.id().as_str(),
                    "isSeeded": true
                })),
            },
        )
        .unwrap();
    let fork = Arc::new(StubAgent {
        id: fork_session.id().clone(),
        session: fork_session,
        ctx: h.ctx.clone(),
        options: AgentOptions::default(),
        scope: ScopeKey::new(),
        model: unsupported_model(),
        inbox: create_inbox_stub(),
        status: AtomicU8::new(1),
    });
    let fork_dyn: Arc<dyn Agent> = fork.clone();
    h.ctx
        .get::<Agents>()
        .unwrap()
        .register(&h.ctx, fork_dyn.clone())
        .await
        .unwrap();
    let restored = h.goals().get(&fork_dyn).unwrap().unwrap();
    assert_eq!(restored.goal.id, created.goal.id);
    assert_eq!(restored.activation, GoalActivation::Disarmed);
    fork.session
        .append("turn/start", json!({ "turn": 2 }), None)
        .unwrap();
    fork.session
        .append(
            "user/message",
            serde_json::to_value(create_user_message(NewUserMessage {
                content: vec![ContentBlock::text("continue this goal")],
                source: Harness::source("user"),
            }))
            .unwrap(),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    fork.session
        .append("step/start", json!({ "turn": 2, "step": 1 }), None)
        .unwrap();
    let result = h
        .execute_as(
            "update_goal",
            json!({
                "goal_id": created.goal.id.as_str(),
                "revision": created.goal.revision,
                "action": "resume"
            }),
            Some(fork_dyn.clone()),
            Some(fork_dyn),
        )
        .await;
    assert_eq!(goal(&result)["phase"], "active");
    assert_eq!(goal(&result)["revision"], 2);
    h.close().await;
}

#[tokio::test]
async fn accepts_direct_human_steering_in_a_goal_round() {
    let mut h = Harness::new(3).await;
    h.open_turn(Harness::source("user"));
    let created = h
        .goals()
        .create(
            &h.agent_dyn,
            CreateGoalRequest {
                objective: "steer me".into(),
                max_goal_rounds: None,
            },
        )
        .unwrap();
    h.close_turn();
    h.open_turn(Harness::goal_source(
        created.goal.id.as_str(),
        created.goal.revision,
        1,
    ));
    h.add_user_message();
    let paused = h
        .execute(
            "update_goal",
            json!({
                "goal_id": created.goal.id.as_str(),
                "revision": created.goal.revision,
                "action": "pause"
            }),
        )
        .await;
    assert_eq!(goal(&paused)["phase"], "paused");
    h.close().await;
}

#[tokio::test]
async fn reads_null_then_edits_and_pauses_by_exact_revision() {
    let mut h = Harness::new(3).await;
    h.open_turn(Harness::source("user"));
    assert_eq!(
        h.execute("get_goal", json!({})).await.value(),
        Some(&json!({ "goal": null }))
    );
    let created = h
        .execute("create_goal", json!({ "objective": "old" }))
        .await;
    let id = goal(&created)["id"].as_str().unwrap();
    let edited = h
        .execute(
            "update_goal",
            json!({
                "goal_id": id, "revision": 1, "action": "edit",
                "objective": "new", "max_goal_rounds": 8
            }),
        )
        .await;
    assert_eq!(goal(&edited)["objective"], "new");
    assert_eq!(goal(&edited)["revision"], 2);
    let paused = h
        .execute(
            "update_goal",
            json!({ "goal_id": id, "revision": 2, "action": "pause" }),
        )
        .await;
    assert_eq!(goal(&paused)["phase"], "paused");
    assert_eq!(goal(&paused)["revision"], 3);
    assert!(!paused.concludes_turn());
    assert!(paused.additional_contexts().is_none());
    h.close().await;
}

#[tokio::test]
async fn model_cannot_resume_a_durable_paused_goal() {
    let mut h = Harness::new(3).await;
    h.open_turn(Harness::source("user"));
    let created = h
        .goals()
        .create(
            &h.agent_dyn,
            CreateGoalRequest {
                objective: "pause me".into(),
                max_goal_rounds: None,
            },
        )
        .unwrap();
    let paused = h
        .execute(
            "update_goal",
            json!({
                "goal_id": created.goal.id.as_str(), "revision": 1, "action": "pause"
            }),
        )
        .await;
    h.close_turn();
    h.open_turn(Harness::source("user"));
    let result = h
        .execute(
            "update_goal",
            json!({
                "goal_id": created.goal.id.as_str(),
                "revision": goal(&paused)["revision"],
                "action": "resume"
            }),
        )
        .await;
    assert_eq!(code(&result), Some("GOAL_TOOL_RESUME_PAUSED"));
    let current = h.goals().get(&h.agent_dyn).unwrap().unwrap();
    assert_eq!(current.goal.phase, GoalPhase::Paused);
    assert_eq!(current.activation, GoalActivation::Disarmed);
    h.close().await;
}

#[tokio::test]
async fn populated_fields_unused_by_each_action_do_not_mutate_other_goal_values() {
    for action in ["edit", "pause", "resume", "complete", "blocked"] {
        let mut h = Harness::new(3).await;
        h.open_turn(Harness::source("user"));
        let created = h
            .goals()
            .create(
                &h.agent_dyn,
                CreateGoalRequest {
                    objective: "keep this objective".into(),
                    max_goal_rounds: Some(4),
                },
            )
            .unwrap();
        if action == "resume" {
            h.goals().disarm(&h.agent_dyn).unwrap();
        }
        let changes_before = h
            .agent
            .session
            .snapshot_events(None, None)
            .iter()
            .filter(|event| event.event_type() == "goal/change")
            .count();
        let result = h
            .execute(
                "update_goal",
                json!({
                    "goal_id": created.goal.id.as_str(),
                    "revision": created.goal.revision,
                    "action": action,
                    "objective": "replacement objective",
                    "max_goal_rounds": 12,
                    "blocked_reason": "a concrete blocker"
                }),
            )
            .await;
        assert_eq!(
            goal(&result)["objective"],
            if action == "edit" {
                "replacement objective"
            } else {
                "keep this objective"
            },
            "action {action}"
        );
        assert_eq!(
            goal(&result)["maxGoalRounds"],
            if action == "edit" { 12 } else { 4 },
            "action {action}"
        );
        let expected_phase = match action {
            "pause" => "paused",
            "complete" => "complete",
            "blocked" => "blocked",
            _ => "active",
        };
        assert_eq!(goal(&result)["phase"], expected_phase, "action {action}");
        assert_eq!(goal(&result)["revision"], 2, "action {action}");
        if action == "blocked" {
            assert_eq!(
                goal(&result)["blockedReason"],
                json!({ "code": "model-reported", "message": "a concrete blocker" })
            );
        }
        assert!(!result.concludes_turn(), "action {action}");
        assert!(result.additional_contexts().is_none(), "action {action}");
        let current = h.goals().get(&h.agent_dyn).unwrap().unwrap();
        assert_eq!(
            current
                .goal
                .blocked_reason
                .as_ref()
                .map(|reason| reason.message.as_str()),
            (action == "blocked").then_some("a concrete blocker")
        );
        assert_eq!(
            h.agent
                .session
                .snapshot_events(None, None)
                .iter()
                .filter(|event| event.event_type() == "goal/change")
                .count(),
            changes_before + 1
        );
        h.close().await;
    }
}

#[tokio::test]
async fn direct_human_resume_rearms_an_active_disarmed_goal() {
    let mut h = Harness::new(3).await;
    h.open_turn(Harness::source("user"));
    let created = h
        .goals()
        .create(
            &h.agent_dyn,
            CreateGoalRequest {
                objective: "continue later".into(),
                max_goal_rounds: None,
            },
        )
        .unwrap();
    h.goals().disarm(&h.agent_dyn).unwrap();
    let result = h
        .execute(
            "update_goal",
            json!({
                "goal_id": created.goal.id.as_str(), "revision": 1, "action": "resume"
            }),
        )
        .await;
    assert_eq!(goal(&result)["phase"], "active");
    assert_eq!(goal(&result)["revision"], 2);
    assert_eq!(result.value().unwrap()["activation"], "armed");
    h.close().await;
}

#[tokio::test]
async fn autonomous_completion_defers_wrapup_but_human_completion_does_not() {
    let mut h = Harness::new(3).await;
    h.open_turn(Harness::source("user"));
    let created = h
        .goals()
        .create(
            &h.agent_dyn,
            CreateGoalRequest {
                objective: "finish cleanly".into(),
                max_goal_rounds: None,
            },
        )
        .unwrap();
    h.close_turn();
    h.open_turn(Harness::goal_source(
        created.goal.id.as_str(),
        created.goal.revision,
        1,
    ));
    let complete = h
        .execute(
            "update_goal",
            json!({
                "goal_id": created.goal.id.as_str(), "revision": 1, "action": "complete"
            }),
        )
        .await;
    let contexts = complete.additional_contexts().unwrap();
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0].source.kind, "tool-goal");
    assert_eq!(contexts[0].source.data["form"], "notice");
    assert_eq!(
        contexts[0].source.data["summary"],
        "complete: finish cleanly"
    );
    assert!(
        contexts[0].content[0]
            .text_value()
            .unwrap()
            .contains("<goal_complete>")
    );
    assert!(
        contexts[0].content[0]
            .text_value()
            .unwrap()
            .contains("Do not call any more tools in this run; further work waits for the user's next instruction.")
    );
    assert!(!complete.concludes_turn());
    h.close().await;
}

#[tokio::test]
async fn direct_human_completion_has_no_wrapup_context() {
    let mut h = Harness::new(3).await;
    h.open_turn(Harness::source("user"));
    let created = h
        .goals()
        .create(
            &h.agent_dyn,
            CreateGoalRequest {
                objective: "finish now".into(),
                max_goal_rounds: None,
            },
        )
        .unwrap();
    let result = h
        .execute(
            "update_goal",
            json!({
                "goal_id": created.goal.id.as_str(), "revision": 1, "action": "complete"
            }),
        )
        .await;
    assert!(result.additional_contexts().is_none());
    assert!(!result.concludes_turn());
    h.close().await;
}

#[tokio::test]
async fn create_invalid_objective_wins_over_invalid_cap() {
    let mut h = Harness::new(3).await;
    h.open_turn(Harness::source("user"));
    let result = h
        .execute(
            "create_goal",
            json!({ "objective": " ", "max_goal_rounds": -1 }),
        )
        .await;
    assert_eq!(code(&result), Some("GOAL_INVALID_OBJECTIVE"));
    assert!(h.goals().get(&h.agent_dyn).unwrap().is_none());
    h.close().await;
}

#[tokio::test]
async fn edit_invalid_objective_wins_over_invalid_cap() {
    let mut h = Harness::new(3).await;
    h.open_turn(Harness::source("user"));
    let created = h
        .goals()
        .create(
            &h.agent_dyn,
            CreateGoalRequest {
                objective: "valid".into(),
                max_goal_rounds: None,
            },
        )
        .unwrap();
    let result = h
        .execute(
            "update_goal",
            json!({
                "goal_id": created.goal.id.as_str(), "revision": created.goal.revision,
                "action": "edit", "objective": " ", "max_goal_rounds": -1
            }),
        )
        .await;
    assert_eq!(code(&result), Some("GOAL_INVALID_OBJECTIVE"));
    assert_eq!(
        h.goals().get(&h.agent_dyn).unwrap().unwrap().goal.revision,
        1
    );
    h.close().await;
}

#[tokio::test]
async fn edit_stale_revision_wins_over_invalid_cap() {
    let mut h = Harness::new(3).await;
    h.open_turn(Harness::source("user"));
    let created = h
        .goals()
        .create(
            &h.agent_dyn,
            CreateGoalRequest {
                objective: "valid".into(),
                max_goal_rounds: None,
            },
        )
        .unwrap();
    let result = h
        .execute(
            "update_goal",
            json!({
                "goal_id": created.goal.id.as_str(), "revision": 2,
                "action": "edit", "objective": "replacement", "max_goal_rounds": -1
            }),
        )
        .await;
    assert_eq!(code(&result), Some("GOAL_STALE_REVISION"));
    assert_eq!(
        h.goals().get(&h.agent_dyn).unwrap().unwrap().goal.revision,
        1
    );
    h.close().await;
}

#[tokio::test]
async fn returns_structured_domain_and_conditional_argument_failures() {
    let mut h = Harness::new(3).await;
    h.open_turn(Harness::source("user"));
    assert_eq!(
        code(&h.execute("create_goal", json!({ "objective": " " })).await),
        Some("GOAL_INVALID_OBJECTIVE")
    );
    let created = h
        .goals()
        .create(
            &h.agent_dyn,
            CreateGoalRequest {
                objective: "valid".into(),
                max_goal_rounds: None,
            },
        )
        .unwrap();
    let invalid_cap = h
        .execute(
            "update_goal",
            json!({
                "goal_id": created.goal.id.as_str(), "revision": 1,
                "action": "edit", "max_goal_rounds": -1
            }),
        )
        .await;
    assert_eq!(code(&invalid_cap), Some("GOAL_INVALID_MAX_ROUNDS"));
    assert_eq!(
        invalid_cap
            .error()
            .and_then(|failure| failure.info.as_ref())
            .map(|info| info.name.as_str()),
        Some(GoalError::NAME)
    );
    let invalid_objective = h
        .execute(
            "update_goal",
            json!({
                "goal_id": created.goal.id.as_str(), "revision": 1,
                "action": "edit", "objective": " "
            }),
        )
        .await;
    assert_eq!(code(&invalid_objective), Some("GOAL_INVALID_OBJECTIVE"));
    let stale = h
        .execute(
            "update_goal",
            json!({
                "goal_id": created.goal.id.as_str(), "revision": 2, "action": "complete",
                "objective": "ignored", "max_goal_rounds": 12,
                "blocked_reason": "ignored"
            }),
        )
        .await;
    assert_eq!(code(&stale), Some("GOAL_STALE_REVISION"));
    let wrong_type = h
        .execute(
            "update_goal",
            json!({
                "goal_id": created.goal.id.as_str(), "revision": 1,
                "action": "complete", "max_goal_rounds": "12"
            }),
        )
        .await;
    assert!(wrong_type.is_error());
    let missing_reason = h
        .execute(
            "update_goal",
            json!({
                "goal_id": created.goal.id.as_str(), "revision": 1, "action": "blocked"
            }),
        )
        .await;
    assert_eq!(code(&missing_reason), Some("GOAL_TOOL_INVALID_UPDATE"));
    let blank_reason = h
        .execute(
            "update_goal",
            json!({
                "goal_id": created.goal.id.as_str(), "revision": 1,
                "action": "blocked", "blocked_reason": " "
            }),
        )
        .await;
    assert_eq!(code(&blank_reason), Some("GOAL_TOOL_INVALID_UPDATE"));
    let malformed_ref = h
        .execute(
            "update_goal",
            json!({ "goal_id": "", "revision": 0, "action": "edit", "objective": "x" }),
        )
        .await;
    assert_eq!(code(&malformed_ref), Some("GOAL_TOOL_INVALID_UPDATE"));
    let unsafe_revision = h
        .execute(
            "update_goal",
            json!({
                "goal_id": created.goal.id.as_str(),
                "revision": 9_007_199_254_740_992_u64,
                "action": "complete"
            }),
        )
        .await;
    assert_eq!(code(&unsafe_revision), Some("GOAL_TOOL_INVALID_UPDATE"));
    h.close().await;
}

#[tokio::test]
async fn empty_edit_fillers_are_ignored_and_unused_fields_do_not_mutate_goal() {
    let mut h = Harness::new(3).await;
    h.open_turn(Harness::source("user"));
    let created = h
        .goals()
        .create(
            &h.agent_dyn,
            CreateGoalRequest {
                objective: "keep this objective".into(),
                max_goal_rounds: Some(4),
            },
        )
        .unwrap();
    let edited = h
        .execute(
            "update_goal",
            json!({
                "goal_id": created.goal.id.as_str(), "revision": 1, "action": "edit",
                "objective": "replacement", "max_goal_rounds": 0, "blocked_reason": ""
            }),
        )
        .await;
    assert_eq!(goal(&edited)["objective"], "replacement");
    assert_eq!(goal(&edited)["maxGoalRounds"], 4);
    let current = h.goals().get(&h.agent_dyn).unwrap().unwrap();
    let capped = h
        .execute(
            "update_goal",
            json!({
                "goal_id": current.goal.id.as_str(), "revision": current.goal.revision,
                "action": "edit", "objective": "", "max_goal_rounds": 8,
                "blocked_reason": ""
            }),
        )
        .await;
    assert_eq!(goal(&capped)["objective"], "replacement");
    assert_eq!(goal(&capped)["maxGoalRounds"], 8);
    let current = h.goals().get(&h.agent_dyn).unwrap().unwrap();
    let paused = h
        .execute(
            "update_goal",
            json!({
                "goal_id": current.goal.id.as_str(), "revision": current.goal.revision,
                "action": "pause", "objective": "", "max_goal_rounds": 0,
                "blocked_reason": ""
            }),
        )
        .await;
    assert_eq!(goal(&paused)["phase"], "paused");
    let current = h.goals().get(&h.agent_dyn).unwrap().unwrap();
    let resumed = h.goals().resume(&h.agent_dyn, &current.as_ref()).unwrap();
    h.goals().disarm(&h.agent_dyn).unwrap();
    let resumed = h
        .execute(
            "update_goal",
            json!({
                "goal_id": resumed.goal.id.as_str(), "revision": resumed.goal.revision,
                "action": "resume", "objective": "", "max_goal_rounds": 0,
                "blocked_reason": ""
            }),
        )
        .await;
    assert_eq!(goal(&resumed)["phase"], "active");
    let current = h.goals().get(&h.agent_dyn).unwrap().unwrap();
    let blocked = h
        .execute(
            "update_goal",
            json!({
                "goal_id": current.goal.id.as_str(), "revision": current.goal.revision,
                "action": "blocked", "objective": "", "max_goal_rounds": 0,
                "blocked_reason": "actual blocker"
            }),
        )
        .await;
    assert_eq!(goal(&blocked)["phase"], "blocked");
    let current = h.goals().get(&h.agent_dyn).unwrap().unwrap();
    let resumed = h.goals().resume(&h.agent_dyn, &current.as_ref()).unwrap();
    let complete = h
        .execute(
            "update_goal",
            json!({
                "goal_id": resumed.goal.id.as_str(), "revision": resumed.goal.revision,
                "action": "complete", "objective": "", "max_goal_rounds": 0,
                "blocked_reason": ""
            }),
        )
        .await;
    assert_eq!(goal(&complete)["objective"], "replacement");
    assert_eq!(goal(&complete)["maxGoalRounds"], 8);
    h.close().await;
}

#[tokio::test]
async fn exact_goal_round_can_complete_but_cannot_edit() {
    let mut h = Harness::new(3).await;
    h.open_turn(Harness::source("user"));
    let created = h
        .goals()
        .create(
            &h.agent_dyn,
            CreateGoalRequest {
                objective: "round-owned".into(),
                max_goal_rounds: None,
            },
        )
        .unwrap();
    h.close_turn();
    h.open_turn(Harness::goal_source(
        created.goal.id.as_str(),
        created.goal.revision,
        1,
    ));
    let edit = h
        .execute(
            "update_goal",
            json!({
                "goal_id": created.goal.id.as_str(), "revision": 1,
                "action": "edit", "objective": "forbidden"
            }),
        )
        .await;
    assert_eq!(code(&edit), Some("GOAL_TOOL_AUTHORITY_REQUIRED"));
    let complete = h
        .execute(
            "update_goal",
            json!({
                "goal_id": created.goal.id.as_str(), "revision": 1, "action": "complete"
            }),
        )
        .await;
    assert_eq!(goal(&complete)["phase"], "complete");
    assert_eq!(goal(&complete)["revision"], 2);
    assert_eq!(goal(&complete)["roundsStarted"], 1);
    assert_eq!(goal(&complete)["objective"], "round-owned");
    assert_eq!(goal(&complete)["maxGoalRounds"], 256);
    h.close().await;
}

#[tokio::test]
async fn model_blocking_obeys_round_threshold_and_defers_blocked_wrapup() {
    let mut h = Harness::new(3).await;
    h.open_turn(Harness::source("user"));
    let created = h
        .goals()
        .create(
            &h.agent_dyn,
            CreateGoalRequest {
                objective: "blocked eventually".into(),
                max_goal_rounds: Some(8),
            },
        )
        .unwrap();
    h.close_turn();
    for round in 1..=2 {
        h.open_turn(Harness::goal_source(
            created.goal.id.as_str(),
            created.goal.revision,
            round,
        ));
        let result = h
            .execute(
                "update_goal",
                json!({
                    "goal_id": created.goal.id.as_str(), "revision": 1, "action": "blocked",
                    "blocked_reason": "The required credential is still unavailable."
                }),
            )
            .await;
        assert_eq!(code(&result), Some("GOAL_TOOL_BLOCK_THRESHOLD"));
        h.close_turn();
    }
    h.open_turn(Harness::goal_source(
        created.goal.id.as_str(),
        created.goal.revision,
        3,
    ));
    let result = h
        .execute(
            "update_goal",
            json!({
                "goal_id": created.goal.id.as_str(), "revision": 1, "action": "blocked",
                "blocked_reason": "The required credential is still unavailable."
            }),
        )
        .await;
    assert_eq!(goal(&result)["phase"], "blocked");
    assert_eq!(goal(&result)["roundsStarted"], 3);
    assert_eq!(
        goal(&result)["blockedReason"],
        json!({
            "code": "model-reported",
            "message": "The required credential is still unavailable."
        })
    );
    assert!(!result.concludes_turn());
    let text = result.additional_contexts().unwrap()[0].content[0]
        .text_value()
        .unwrap();
    assert!(text.contains("<goal_blocked>"));
    assert!(text.contains("The required credential is still unavailable."));
    assert!(text.contains(
        "Do not call any more tools in this run; further work waits for the user's next instruction."
    ));
    h.close().await;
}

#[tokio::test]
async fn direct_human_can_block_before_model_threshold() {
    let mut h = Harness::new(9).await;
    h.open_turn(Harness::source("user"));
    let created = h
        .goals()
        .create(
            &h.agent_dyn,
            CreateGoalRequest {
                objective: "human stop".into(),
                max_goal_rounds: None,
            },
        )
        .unwrap();
    let result = h
        .execute(
            "update_goal",
            json!({
                "goal_id": created.goal.id.as_str(), "revision": 1, "action": "blocked",
                "blocked_reason": "The user asked to stop."
            }),
        )
        .await;
    assert_eq!(goal(&result)["phase"], "blocked");
    assert_eq!(goal(&result)["roundsStarted"], 0);
    assert!(result.additional_contexts().is_none());
    assert!(!result.concludes_turn());
    h.close().await;
}
