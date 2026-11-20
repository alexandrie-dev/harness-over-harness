use futures_util::{StreamExt, future::BoxFuture, stream};
use hoh_agent::{
    Agent, AgentCreated, AgentError, AgentInboxClaimed, AgentInboxInserted, AgentOptions,
    AgentPreStep, AgentRequestError, AgentStatus, Agents, CancelOptions, CreateAgentOptions, Inbox,
    InboxTarget, Live, MaintenanceFuture, MaintenanceTask, Model, PreStepDecision,
    RequestErrorAction, SessionStartSource, agent_events,
};
use hoh_agent_loop_testkit::{
    AgentLoopTestDependenciesOptions, create_inbox_stub, mount_agent_loop_test_dependencies,
    mount_agent_loop_test_harness, unsupported_model,
};
use hoh_error::BoxError;
use hoh_goal::{
    CreateGoalRequest, EditGoalRequest, GoalActivation, GoalChanged, GoalChangedEvent,
    GoalOperation, GoalPhase, GoalRef, GoalServicePlugin, GoalView, Goals,
};
use hoh_goal_round_driver::{GoalRoundDriver, render_goal_round_prompt};
use hoh_kernel::{Cfg, Context, Fiber, Inject, Kernel, NoConfig, Plugin, Service};
use hoh_llm::{
    AdapterRegistrationHandle, BuiltinLlmFailureCode, ContentBlock, GenerateOptions, Llm,
    LlmAdapter, LlmError, LlmStream, MessageSource, NewUserMessage, StreamChunk,
    create_user_message,
};
use hoh_scope::ScopeKey;
use hoh_session::{
    AgentCancelCause, CreateSessionOptions, Session, SessionEventEmitted, SessionId, SessionStore,
    Sessions,
};
use hoh_subagent::{
    ProviderRegistration, ResolvedSubagentStartRequest, SubagentActivation, SubagentActivationSpec,
    SubagentCapabilities, SubagentDeliveryTarget, SubagentProvider, SubagentResult, SubagentRun,
    SubagentRuntime, Subagents,
};
use serde_json::{Map, json};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Notify, oneshot};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn chunk(value: serde_json::Value) -> StreamChunk {
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
    let mut response = text_response(text);
    response.pop();
    response.push(chunk(
        json!({"type":"finish","reason":{"kind":"max-tokens"}}),
    ));
    response
}

enum Entry {
    Chunks(Vec<StreamChunk>),
    Fail(&'static str),
    RateLimit,
    Hang,
    Callback(Arc<dyn Fn() -> Vec<StreamChunk> + Send + Sync>),
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

    fn request_count(&self) -> usize {
        lock(&self.requests).len()
    }

    fn request_text(&self, index: usize) -> String {
        lock(&self.requests)[index]
            .messages
            .iter()
            .flat_map(|message| message.content())
            .filter_map(ContentBlock::text_value)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl LlmAdapter for ScriptedAdapter {
    fn stream(&self, options: GenerateOptions) -> Result<LlmStream, BoxError> {
        let signal = options.signal.clone();
        lock(&self.requests).push(options);
        match lock(&self.script).pop_front() {
            Some(Entry::Chunks(chunks)) => Ok(LlmStream::chunks(chunks)),
            Some(Entry::Fail(message)) => Ok(LlmStream::new(stream::once(async move {
                Err::<StreamChunk, BoxError>(message.into())
            }))),
            Some(Entry::RateLimit) => Ok(LlmStream::new(stream::once(async move {
                let error = LlmError::new("slow down", BuiltinLlmFailureCode::RateLimit)
                    .expect("valid rate-limit error");
                Err::<StreamChunk, BoxError>(Box::new(error))
            }))),
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
            Some(Entry::Callback(callback)) => Ok(LlmStream::chunks(callback())),
            None => Ok(LlmStream::new(stream::once(async {
                Err::<StreamChunk, BoxError>("ScriptedAdapter: script exhausted".into())
            }))),
        }
    }
}

struct Harness {
    kernel: Kernel,
    ctx: Context,
    adapter: Arc<ScriptedAdapter>,
    agent: Arc<dyn Agent>,
    driver: Fiber,
    _registration: AdapterRegistrationHandle,
}

static NEXT_SESSION: AtomicUsize = AtomicUsize::new(1);
static OWNED_AGENT: Mutex<Option<Arc<dyn Agent>>> = Mutex::new(None);

#[derive(Inject)]
struct AgentOwnerDeps {
    agents: Service<Agents>,
}

struct AgentOwner;

impl Plugin for AgentOwner {
    const NAME: &'static str = "goal-round-driver-test-agent-owner";
    type Config = NoConfig;
    type Deps = AgentOwnerDeps;

    async fn apply(
        ctx: Context,
        deps: AgentOwnerDeps,
        _config: Cfg<NoConfig>,
    ) -> Result<(), BoxError> {
        let mut options = CreateAgentOptions::new("goal-session-owner-unload".into());
        options.agent_options = Some(AgentOptions {
            provider: Some(Live::Value("mock".into())),
            model: Some(Live::Value("mock".into())),
            ..Default::default()
        });
        let handle = deps.agents.create(&ctx, options).await?;
        *lock(&OWNED_AGENT) = Some(handle.agent);
        Ok(())
    }
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
        .expect("test dependencies");
        let goal = ctx
            .plugin::<GoalServicePlugin>(json!({}))
            .expect("goal plugin");
        goal.settled().await.expect("goal settles");
        let driver = ctx
            .plugin::<GoalRoundDriver>(json!({}))
            .expect("driver plugin");
        driver.settled().await.expect("driver settles");
        let loop_harness = mount_agent_loop_test_harness(&ctx)
            .await
            .expect("agent loop");
        let adapter = ScriptedAdapter::new(script);
        let registration = ctx
            .get::<Llm>()
            .expect("llm")
            .register_adapter(&ctx, vec!["mock".into()], adapter.clone())
            .expect("register adapter");
        let agent = loop_harness
            .create_with(
                SessionId::from(format!(
                    "goal-round-{}",
                    NEXT_SESSION.fetch_add(1, Ordering::Relaxed)
                )),
                AgentOptions {
                    provider: Some(Live::Value("mock".into())),
                    model: Some(Live::Value("mock".into())),
                    ..AgentOptions::default()
                },
                Default::default(),
            )
            .await
            .expect("create agent");
        Self {
            kernel,
            ctx,
            adapter,
            agent,
            driver,
            _registration: registration,
        }
    }

    fn goals(&self) -> hoh_kernel::Service<Goals> {
        self.ctx.get::<Goals>().expect("goals")
    }

    async fn close(self) {
        self.kernel.shutdown().await;
    }
}

#[derive(Clone, Copy)]
enum FollowupFault {
    Reject,
    DisarmThenReject,
    Panic,
    CancelReject,
    CancelOk,
}

struct FaultAgent {
    id: SessionId,
    session: Session,
    ctx: Context,
    options: AgentOptions,
    scope: ScopeKey,
    model: Arc<dyn Model>,
    inbox: Arc<dyn Inbox>,
    agents: Service<Agents>,
    goals: Service<Goals>,
    fault: FollowupFault,
    running: Arc<AtomicBool>,
    cancel_calls: Arc<AtomicUsize>,
}

impl Agent for FaultAgent {
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
        if self.running.load(Ordering::Acquire) {
            AgentStatus::Running
        } else {
            AgentStatus::Idle
        }
    }

    fn ctx(&self) -> &Context {
        &self.ctx
    }

    fn cancel(&self, _: AgentCancelCause, _: CancelOptions) -> Result<(), BoxError> {
        self.cancel_calls.fetch_add(1, Ordering::Relaxed);
        if matches!(self.fault, FollowupFault::CancelReject) {
            return Err("cancel rejected".into());
        }
        self.running.store(false, Ordering::Release);
        Ok(())
    }

    fn when_idle(&self) -> BoxFuture<'static, ()> {
        if matches!(self.fault, FollowupFault::CancelReject) {
            Box::pin(std::future::pending())
        } else {
            Box::pin(async {})
        }
    }

    fn run_maintenance_erased(&self, task: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        Ok(task(hoh_timeout::AbortController::new().signal()))
    }

    fn send(
        &self,
        message: hoh_llm::UserMessage,
        target: InboxTarget,
        _: bool,
    ) -> Result<(), BoxError> {
        self.inbox.append(target, message)
    }

    fn followup(&self, message: hoh_llm::UserMessage) -> Result<(), BoxError> {
        if message.source.kind == "goal" {
            match self.fault {
                FollowupFault::Reject => return Err("queue rejected".into()),
                FollowupFault::DisarmThenReject => {
                    let live = self
                        .agents
                        .get(&self.id)
                        .ok_or_else(|| -> BoxError { "fault agent is not live".into() })?;
                    self.goals.disarm(&live)?;
                    return Err("queue rejected after disarm".into());
                }
                FollowupFault::Panic => panic!("queue panicked"),
                FollowupFault::CancelReject | FollowupFault::CancelOk => {
                    self.running.store(true, Ordering::Release);
                }
            }
        }
        self.inbox.append(InboxTarget::NextTurn, message)
    }

    fn steer(&self, message: hoh_llm::UserMessage) -> Result<(), BoxError> {
        self.inbox.append(InboxTarget::NextStep, message)
    }

    fn inject(&self, message: hoh_llm::UserMessage) -> Result<(), BoxError> {
        self.inbox.append(InboxTarget::NextStep, message)
    }
}

struct FaultHarness {
    kernel: Kernel,
    ctx: Context,
    agent: Arc<dyn Agent>,
    driver: Fiber,
    running: Arc<AtomicBool>,
    cancel_calls: Arc<AtomicUsize>,
}

impl FaultHarness {
    async fn register_agent(
        ctx: &Context,
        fault: FollowupFault,
    ) -> (Arc<dyn Agent>, Arc<AtomicBool>, Arc<AtomicUsize>) {
        let id = SessionId::from(format!(
            "goal-round-fault-{}",
            NEXT_SESSION.fetch_add(1, Ordering::Relaxed)
        ));
        let sessions = ctx.get::<Sessions>().expect("sessions");
        let session = sessions
            .create(ctx, Some(id.clone()), CreateSessionOptions::default())
            .expect("session");
        let agents = ctx.get::<Agents>().expect("agents");
        let goals = ctx.get::<Goals>().expect("goals");
        let running = Arc::new(AtomicBool::new(false));
        let cancel_calls = Arc::new(AtomicUsize::new(0));
        let agent: Arc<dyn Agent> = Arc::new(FaultAgent {
            id,
            session,
            ctx: ctx.clone(),
            options: AgentOptions::default(),
            scope: ScopeKey::new(),
            model: unsupported_model(),
            inbox: create_inbox_stub(),
            agents: agents.clone(),
            goals,
            fault,
            running: running.clone(),
            cancel_calls: cancel_calls.clone(),
        });
        agents
            .register(ctx, agent.clone())
            .await
            .expect("register fault agent");
        (agent, running, cancel_calls)
    }

    async fn new(fault: FollowupFault) -> Self {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        mount_agent_loop_test_dependencies(&ctx, Default::default())
            .await
            .expect("test dependencies");
        let goal = ctx
            .plugin::<GoalServicePlugin>(json!({}))
            .expect("goal plugin");
        goal.settled().await.expect("goal settles");
        let driver = ctx
            .plugin::<GoalRoundDriver>(json!({}))
            .expect("driver plugin");
        driver.settled().await.expect("driver settles");

        let (agent, running, cancel_calls) = Self::register_agent(&ctx, fault).await;
        Self {
            kernel,
            ctx,
            agent,
            driver,
            running,
            cancel_calls,
        }
    }

    fn goals(&self) -> Service<Goals> {
        self.ctx.get::<Goals>().expect("goals")
    }

    fn create_goal(&self, objective: &str) {
        self.create_goal_for(&self.agent, objective);
    }

    fn create_goal_for(&self, agent: &Arc<dyn Agent>, objective: &str) {
        self.goals()
            .create(
                agent,
                CreateGoalRequest {
                    objective: objective.into(),
                    max_goal_rounds: Some(8),
                },
            )
            .expect("create goal");
    }

    async fn wait_for_goal(&self, predicate: impl Fn(&GoalView) -> bool) -> GoalView {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(goal) = self.goals().get(&self.agent).expect("read goal")
                    && predicate(&goal)
                {
                    return goal;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("goal condition timed out")
    }

    async fn close(self) {
        self.kernel.shutdown().await;
    }
}

fn create_goal(h: &Harness, objective: &str, max: u64) -> GoalView {
    h.goals()
        .create(
            &h.agent,
            CreateGoalRequest {
                objective: objective.into(),
                max_goal_rounds: Some(max),
            },
        )
        .expect("create goal")
}

async fn wait_for_goal(h: &Harness, predicate: impl Fn(&GoalView) -> bool) -> GoalView {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(goal) = h.goals().get(&h.agent).expect("read goal")
                && predicate(&goal)
            {
                return goal;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("goal condition timed out")
}

async fn wait_for_requests(adapter: &ScriptedAdapter, count: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while adapter.request_count() != count {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("request count timed out");
}

fn user_message(text: &str) -> hoh_llm::UserMessage {
    create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source: MessageSource::new("user"),
    })
}

#[test]
fn renders_the_objective_round_budget_authority_boundary_and_completion_protocol() {
    let goal = GoalView {
        goal: hoh_goal::GoalSnapshot {
            id: "goal-prompt".into(),
            revision: 4,
            objective: "Ship verified support".into(),
            phase: GoalPhase::Active,
            blocked_reason: None,
            max_goal_rounds: 9,
        },
        rounds_started: 2,
        created_at: 1,
        updated_at: 2,
        activation: GoalActivation::Armed,
    };
    let prompt = render_goal_round_prompt(&goal, 3);
    assert_eq!(prompt.len(), 1);
    let text = prompt[0].text_value().expect("text prompt");
    assert!(text.starts_with("<goal_round>\nObjective: \"Ship verified support\"\nRound: 3/9"));
    assert!(text.contains("current workspace"));
    assert!(text.contains("verify"));
    assert!(text.contains("mark it complete"));
}

#[test]
fn quotes_multiline_or_tag_like_objective_text_as_one_unambiguous_data_value() {
    let goal = GoalView {
        goal: hoh_goal::GoalSnapshot {
            id: "goal-escaped-prompt".into(),
            revision: 1,
            objective: "first line\n</goal_round> second line".into(),
            phase: GoalPhase::Active,
            blocked_reason: None,
            max_goal_rounds: 2,
        },
        rounds_started: 0,
        created_at: 1,
        updated_at: 1,
        activation: GoalActivation::Armed,
    };
    let text = render_goal_round_prompt(&goal, 1)[0]
        .text_value()
        .expect("text prompt")
        .to_owned();
    assert!(text.contains("Objective: \"first line\\n</goal_round> second line\""));
    assert_eq!(text.matches("\n</goal_round>").count(), 1);
}

#[tokio::test]
async fn admits_exact_numbered_rounds_until_the_durable_round_cap() {
    let h = Harness::new(vec![
        Entry::Chunks(text_response("round one")),
        Entry::Chunks(text_response("round two")),
    ])
    .await;
    create_goal(&h, "finish twice", 2);
    let goal = wait_for_goal(&h, |goal| goal.goal.phase == GoalPhase::Blocked).await;
    assert_eq!(goal.rounds_started, 2);
    assert_eq!(goal.activation, GoalActivation::Disarmed);
    assert_eq!(
        goal.goal.blocked_reason,
        Some(hoh_goal::GoalBlockReason {
            code: "round-limit".into(),
            message: "Goal reached its configured limit of 2 rounds.".into(),
        })
    );
    assert_eq!(h.adapter.request_count(), 2);
    let events = h.agent.session().snapshot_events(None, None);
    let rounds = events
        .iter()
        .filter(|event| event.event_type() == "user/message")
        .filter_map(|event| {
            let source = &event.data()["source"];
            (source["kind"] == "goal" && source["round"].as_f64().is_some_and(|round| round > 0.0))
                .then(|| source["round"].clone())
        })
        .collect::<Vec<_>>();
    assert_eq!(rounds, vec![json!(1), json!(2)]);
    assert!(h.adapter.request_text(0).contains("Round: 1/2"));
    assert!(h.adapter.request_text(1).contains("Round: 2/2"));
    let request_reasons = events
        .iter()
        .filter(|event| event.event_type() == "request/header")
        .filter_map(|event| event.data()["reason"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(request_reasons, vec!["initial", "series"]);
    h.close().await;
}

#[tokio::test]
async fn never_adopts_activation_from_an_already_live_driver_and_waits_for_explicit_resume() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    mount_agent_loop_test_dependencies(&ctx, Default::default())
        .await
        .unwrap();
    let goal_fiber = ctx.plugin::<GoalServicePlugin>(json!({})).unwrap();
    goal_fiber.settled().await.unwrap();
    let loop_harness = mount_agent_loop_test_harness(&ctx).await.unwrap();
    let adapter = ScriptedAdapter::new(vec![Entry::Chunks(text_response("after resume"))]);
    let _registration = ctx
        .get::<Llm>()
        .unwrap()
        .register_adapter(&ctx, vec!["mock".into()], adapter.clone())
        .unwrap();
    let agent = loop_harness
        .create_with(
            "goal-hot-load".into(),
            AgentOptions {
                provider: Some(Live::Value("mock".into())),
                model: Some(Live::Value("mock".into())),
                ..Default::default()
            },
            Default::default(),
        )
        .await
        .unwrap();
    let goals = ctx.get::<Goals>().unwrap();
    let created = goals
        .create(
            &agent,
            CreateGoalRequest {
                objective: "wait for a human".into(),
                max_goal_rounds: Some(1),
            },
        )
        .unwrap();
    let driver = ctx.plugin::<GoalRoundDriver>(json!({})).unwrap();
    driver.settled().await.unwrap();
    tokio::task::yield_now().await;
    assert_eq!(
        goals.get(&agent).unwrap().unwrap().activation,
        GoalActivation::Disarmed
    );
    assert_eq!(adapter.request_count(), 0);
    goals.resume(&agent, &created.as_ref()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if goals.get(&agent).unwrap().unwrap().goal.phase == GoalPhase::Blocked {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(adapter.request_count(), 1);
    kernel.shutdown().await;
}

#[tokio::test]
async fn disarms_automatic_continuation_after_max_tokens() {
    let h = Harness::new(vec![Entry::Chunks(max_tokens_response("unfinished"))]).await;
    create_goal(&h, "stop safely", 8);
    let goal = wait_for_goal(&h, |goal| goal.activation == GoalActivation::Disarmed).await;
    assert_eq!(goal.goal.phase, GoalPhase::Active);
    assert_eq!(goal.rounds_started, 1);
    assert_eq!(h.adapter.request_count(), 1);
    h.close().await;
}

#[tokio::test]
async fn disarms_automatic_continuation_after_a_rate_limit() {
    let h = Harness::new(vec![Entry::RateLimit]).await;
    create_goal(&h, "stop safely", 8);
    let goal = wait_for_goal(&h, |goal| {
        goal.goal.phase == GoalPhase::Active && goal.activation == GoalActivation::Disarmed
    })
    .await;
    assert_eq!(goal.rounds_started, 1);
    assert_eq!(h.adapter.request_count(), 1);
    h.close().await;
}

#[tokio::test]
async fn blocks_the_goal_when_a_custom_agent_rejects_the_valid_followup() {
    let h = FaultHarness::new(FollowupFault::Reject).await;
    h.create_goal("handle queue failure");

    let goal = h
        .wait_for_goal(|goal| goal.goal.phase == GoalPhase::Blocked)
        .await;
    assert_eq!(goal.rounds_started, 0);
    assert_eq!(goal.activation, GoalActivation::Disarmed);
    assert_eq!(
        goal.goal.blocked_reason,
        Some(hoh_goal::GoalBlockReason {
            code: "queue-failed".into(),
            message: "Could not queue goal round 1: queue rejected".into(),
        })
    );
    h.close().await;
}

#[tokio::test]
async fn preserves_a_custom_agent_disarm_before_followup_rejects() {
    let h = FaultHarness::new(FollowupFault::DisarmThenReject).await;
    h.create_goal("preserve the newer activation state");

    let goal = h
        .wait_for_goal(|goal| goal.activation == GoalActivation::Disarmed)
        .await;
    assert_eq!(goal.goal.phase, GoalPhase::Active);
    assert_eq!(goal.rounds_started, 0);
    assert_eq!(goal.goal.blocked_reason, None);
    h.close().await;
}

#[tokio::test]
async fn contains_a_panicking_driver_task_disarms_and_does_not_pin_teardown() {
    let h = FaultHarness::new(FollowupFault::Panic).await;
    h.create_goal("contain a panicking followup");

    let goal = h
        .wait_for_goal(|goal| goal.activation == GoalActivation::Disarmed)
        .await;
    assert_eq!(goal.goal.phase, GoalPhase::Active);
    assert!(h.ctx.logger().buffer().iter().any(|record| {
        record.message
            == format!(
                "goal-round-driver: driver task rejected for agent \"{}\": queue panicked",
                h.agent.id()
            )
    }));
    tokio::time::timeout(Duration::from_secs(2), h.driver.clone().dispose())
        .await
        .expect("driver teardown must not wait forever after a panic");
    h.close().await;
}

#[tokio::test]
async fn a_rejected_cancel_does_not_pin_teardown_or_skip_other_running_agents() {
    let h = FaultHarness::new(FollowupFault::CancelReject).await;
    let (other, other_running, other_cancel_calls) =
        FaultHarness::register_agent(&h.ctx, FollowupFault::CancelOk).await;
    h.create_goal("first running goal");
    h.create_goal_for(&other, "second running goal");
    tokio::time::timeout(Duration::from_secs(5), async {
        while h.agent.status() != AgentStatus::Running || other.status() != AgentStatus::Running {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both fault agents should accept a queued goal round");

    tokio::time::timeout(Duration::from_secs(2), h.driver.clone().dispose())
        .await
        .expect("a failed cancel must not wait forever for that agent to become idle");

    assert_eq!(h.cancel_calls.load(Ordering::Relaxed), 1);
    assert_eq!(other_cancel_calls.load(Ordering::Relaxed), 1);
    assert!(h.ctx.logger().buffer().iter().any(|record| {
        record.message
            == format!(
                "goal-round-driver: could not cancel agent \"{}\" during teardown: cancel rejected",
                h.agent.id()
            )
    }));
    h.running.store(false, Ordering::Release);
    other_running.store(false, Ordering::Release);
    h.close().await;
}

#[tokio::test]
async fn maps_a_downstream_step_rejection_to_blocked_without_entering_the_round() {
    let h = Harness::new(vec![]).await;
    h.ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(|event, next| async move {
            if event
                .messages
                .first()
                .is_some_and(|message| message.source.kind == "goal")
            {
                Ok(PreStepDecision::Reject)
            } else {
                next.run().await
            }
        })
        .unwrap();
    create_goal(&h, "respect policy", 256);
    let goal = wait_for_goal(&h, |goal| goal.goal.phase == GoalPhase::Blocked).await;
    assert_eq!(goal.rounds_started, 0);
    let reason = goal.goal.blocked_reason.unwrap();
    assert_eq!(reason.code, "prompt-rejected");
    assert_eq!(
        reason.message,
        "Goal round was rejected before entering its step."
    );
    assert_eq!(h.adapter.request_count(), 0);
    h.close().await;
}

#[tokio::test]
async fn pauses_an_admitted_round_when_cancellation_aborts_an_active_step() {
    let h = Harness::new(vec![Entry::Hang]).await;
    create_goal(&h, "stop in flight", 8);
    wait_for_requests(&h.adapter, 1).await;
    h.agent
        .cancel(AgentCancelCause::User, CancelOptions::default())
        .unwrap();
    h.agent.when_idle().await;
    let goal = wait_for_goal(&h, |goal| goal.goal.phase == GoalPhase::Paused).await;
    assert_eq!(goal.rounds_started, 1);
    assert_eq!(goal.activation, GoalActivation::Disarmed);
    h.close().await;
}

#[tokio::test]
async fn aborts_an_in_flight_round_when_a_host_initiated_pause_lands_mid_step() {
    let h = Harness::new(vec![Entry::Hang]).await;
    create_goal(&h, "stop on host pause", 8);
    wait_for_requests(&h.adapter, 1).await;
    let current = h.goals().get(&h.agent).unwrap().unwrap();
    h.goals().pause(&h.agent, &current.as_ref()).unwrap();
    h.agent.when_idle().await;
    let goal = h.goals().get(&h.agent).unwrap().unwrap();
    assert_eq!(goal.goal.phase, GoalPhase::Paused);
    assert_eq!(goal.rounds_started, 1);
    h.close().await;
}

#[tokio::test]
async fn lets_already_queued_human_work_finish_before_reserving_the_next_round() {
    let h = Harness::new(vec![
        Entry::Chunks(text_response("human answer")),
        Entry::Chunks(text_response("goal answer")),
    ])
    .await;
    create_goal(&h, "continue after the human", 1);
    h.agent.followup(user_message("human goes first")).unwrap();
    wait_for_goal(&h, |goal| goal.goal.phase == GoalPhase::Blocked).await;
    assert_eq!(h.adapter.request_count(), 2);
    assert!(h.adapter.request_text(0).contains("human goes first"));
    assert!(!h.adapter.request_text(0).contains("<goal_round>"));
    assert!(h.adapter.request_text(1).contains("<goal_round>"));
    h.close().await;
}

#[tokio::test]
async fn rechecks_revision_after_downstream_prompt_hooks_before_admitting() {
    let h = Harness::new(vec![Entry::Chunks(text_response("new revision"))]).await;
    let edited = Arc::new(Mutex::new(false));
    let goals = h.goals();
    let changed = edited.clone();
    h.ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| {
            let goals = goals.clone();
            let changed = changed.clone();
            async move {
                if event
                    .messages
                    .first()
                    .is_some_and(|message| message.source.kind == "goal")
                    && !*lock(&changed)
                {
                    *lock(&changed) = true;
                    let current = goals.get(&event.agent)?.expect("goal");
                    goals.edit(
                        &event.agent,
                        &current.as_ref(),
                        EditGoalRequest {
                            objective: Some("edited downstream".into()),
                            max_goal_rounds: None,
                        },
                    )?;
                }
                next.run().await
            }
        })
        .unwrap();
    create_goal(&h, "edit during pre-step", 1);
    let goal = wait_for_goal(&h, |goal| goal.goal.phase == GoalPhase::Blocked).await;
    assert_eq!(goal.goal.objective, "edited downstream");
    assert_eq!(goal.rounds_started, 1);
    assert_eq!(h.adapter.request_count(), 1);
    h.close().await;
}

#[tokio::test]
async fn disarms_without_dispatch_when_a_durability_checkpoint_fails() {
    let h = Harness::new(vec![]).await;
    SessionStore::on_flush(&h.ctx, |_| async { Err("disk unavailable".into()) }).unwrap();
    create_goal(&h, "do not outrun storage", 8);
    let goal = wait_for_goal(&h, |goal| goal.activation == GoalActivation::Disarmed).await;
    assert_eq!(goal.rounds_started, 0);
    assert_eq!(h.adapter.request_count(), 0);
    h.close().await;
}

struct GateRun {
    id: SessionId,
    result_calls: Arc<AtomicUsize>,
    released: Arc<AtomicUsize>,
    notify: Arc<Notify>,
}

impl SubagentRun for GateRun {
    fn id(&self) -> &SessionId {
        &self.id
    }

    fn result(&self) -> BoxFuture<'static, Result<SubagentResult, BoxError>> {
        self.result_calls.fetch_add(1, Ordering::SeqCst);
        let released = self.released.clone();
        let notify = self.notify.clone();
        Box::pin(async move {
            loop {
                let notified = notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if released.load(Ordering::SeqCst) != 0 {
                    break;
                }
                notified.await;
            }
            Ok(SubagentResult::completed(Vec::new()))
        })
    }

    fn dispose(&self) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async { Ok(()) })
    }
}

struct GateProvider {
    run: Arc<GateRun>,
}

impl SubagentProvider for GateProvider {
    fn name(&self) -> &str {
        "goal-round-driver-test-gate"
    }

    fn capabilities(&self) -> SubagentCapabilities {
        SubagentCapabilities::default()
    }

    fn inherits_parent_context(&self) -> bool {
        false
    }

    fn supports_start(&self) -> bool {
        true
    }

    fn start(
        &self,
        _request: ResolvedSubagentStartRequest,
    ) -> Option<BoxFuture<'static, Result<Arc<dyn SubagentRun>, BoxError>>> {
        let run: Arc<dyn SubagentRun> = self.run.clone();
        Some(Box::pin(async move { Ok(run) }))
    }
}

struct DescendantGate {
    result_calls: Arc<AtomicUsize>,
    released: Arc<AtomicUsize>,
    notify: Arc<Notify>,
    _registration: ProviderRegistration,
    _activation: SubagentActivation,
}

impl DescendantGate {
    async fn install(h: &Harness) -> Self {
        let result_calls = Arc::new(AtomicUsize::new(0));
        let released = Arc::new(AtomicUsize::new(0));
        let notify = Arc::new(Notify::new());
        let run = Arc::new(GateRun {
            id: SessionId::from(format!(
                "goal-round-child-{}",
                NEXT_SESSION.fetch_add(1, Ordering::Relaxed)
            )),
            result_calls: result_calls.clone(),
            released: released.clone(),
            notify: notify.clone(),
        });
        let runtime = Arc::new(SubagentRuntime::new(h.ctx.clone()));
        h.ctx
            .provide::<Subagents>(runtime.clone())
            .await
            .expect("provide subagent runtime");
        let registration = runtime
            .register_provider(Arc::new(GateProvider { run }))
            .expect("register gate provider");
        let activation = runtime
            .start_activation(SubagentActivationSpec {
                provider: "goal-round-driver-test-gate".into(),
                label: "test descendant".into(),
                name: None,
                child_id: None,
                delivery: SubagentDeliveryTarget::Caller,
                request: hoh_subagent::SubagentStartRequest {
                    cwd: None,
                    agent_preset: None,
                    label: Some("test descendant".into()),
                    prompt: Vec::new(),
                    parent: h.agent.clone(),
                    signal: hoh_timeout::AbortController::new().signal(),
                    agent_options: None,
                    output_schema: None,
                    max_depth: None,
                },
            })
            .await
            .expect("start gate descendant");
        Self {
            result_calls,
            released,
            notify,
            _registration: registration,
            _activation: activation,
        }
    }

    async fn wait_until_joined(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.result_calls.load(Ordering::SeqCst) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("driver did not join descendant");
    }

    fn release(&self) {
        self.released.store(1, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
}

#[tokio::test]
async fn reserves_a_round_only_after_progressing_descendants_settle() {
    let h = Harness::new(vec![Entry::Chunks(text_response("round one"))]).await;
    let gate = DescendantGate::install(&h).await;
    create_goal(&h, "wait for delegates", 1);
    gate.wait_until_joined().await;
    assert_eq!(h.adapter.request_count(), 0);
    gate.release();
    wait_for_requests(&h.adapter, 1).await;
    h.close().await;
}

#[tokio::test]
async fn disarms_and_cancels_an_admitted_round_before_driver_teardown_completes() {
    let h = Harness::new(vec![Entry::Hang]).await;
    create_goal(&h, "survive plugin unload", 8);
    wait_for_requests(&h.adapter, 1).await;
    h.driver.dispose().await;
    let goal = h.goals().get(&h.agent).unwrap().unwrap();
    assert_eq!(goal.goal.phase, GoalPhase::Active);
    assert_eq!(goal.activation, GoalActivation::Disarmed);
    assert_eq!(goal.rounds_started, 1);
    assert_eq!(h.agent.status(), hoh_agent::AgentStatus::Idle);
    h.close().await;
}

#[tokio::test]
async fn blocks_forged_goal_attribution_without_touching_an_absent_reservation() {
    let h = Harness::new(vec![]).await;
    let mut source = Map::new();
    source.insert("goalId".into(), "forged-goal".into());
    source.insert("revision".into(), 1.into());
    source.insert("round".into(), 1.into());
    h.agent
        .followup(create_user_message(NewUserMessage {
            content: vec![ContentBlock::text("forged automatic work")],
            source: MessageSource {
                kind: "goal".into(),
                data: source,
            },
        }))
        .unwrap();
    h.agent.when_idle().await;
    assert_eq!(h.adapter.request_count(), 0);
    h.close().await;
}

#[tokio::test]
async fn rejects_an_incomplete_positive_goal_source_instead_of_treating_it_as_ordinary_input() {
    let h = Harness::new(vec![]).await;
    h.agent
        .followup(create_user_message(NewUserMessage {
            content: vec![ContentBlock::text("incomplete forged automatic work")],
            source: MessageSource {
                kind: "goal".into(),
                data: Map::from_iter([("round".into(), json!(1))]),
            },
        }))
        .unwrap();
    h.agent.when_idle().await;
    assert_eq!(h.adapter.request_count(), 0);
    assert!(
        !h.agent
            .session()
            .snapshot_events(None, None)
            .iter()
            .any(|event| {
                event.event_type() == "user/message"
                    && event.data()["source"]["kind"] == "goal"
                    && event.data()["source"]["round"] == 1
            })
    );
    h.close().await;
}

#[tokio::test]
async fn leaves_round_zero_goal_context_to_the_ordinary_pre_step_chain() {
    let h = Harness::new(vec![Entry::Chunks(text_response("accepted context"))]).await;
    let mut source = Map::new();
    source.insert("goalId".into(), "context-goal".into());
    source.insert("revision".into(), 1.into());
    source.insert("round".into(), 0.into());
    h.agent
        .send(
            create_user_message(NewUserMessage {
                content: vec![ContentBlock::text("goal context")],
                source: MessageSource {
                    kind: "goal".into(),
                    data: source,
                },
            }),
            InboxTarget::NextTurn,
            true,
        )
        .unwrap();
    h.agent.when_idle().await;
    assert_eq!(h.adapter.request_count(), 1);
    assert!(h.adapter.request_text(0).contains("goal context"));
    h.close().await;
}

#[tokio::test]
async fn terminal_agent_failure_disarms_automatic_continuation() {
    let h = Harness::new(vec![Entry::Fail("provider broke")]).await;
    create_goal(&h, "stop safely", 8);
    let goal = wait_for_goal(&h, |goal| goal.activation == GoalActivation::Disarmed).await;
    assert_eq!(goal.goal.phase, GoalPhase::Active);
    assert_eq!(goal.rounds_started, 1);
    assert_eq!(h.adapter.request_count(), 1);
    h.close().await;
}

#[tokio::test]
async fn does_not_invent_goal_state_when_ordinary_queued_work_is_cancelled() {
    let h = Harness::new(vec![]).await;
    h.agent
        .send(
            user_message("cancel ordinary work"),
            InboxTarget::NextTurn,
            false,
        )
        .unwrap();
    h.agent
        .cancel(AgentCancelCause::User, CancelOptions::default())
        .unwrap();
    h.agent.when_idle().await;
    assert!(h.goals().get(&h.agent).unwrap().is_none());
    assert_eq!(h.adapter.request_count(), 0);
    h.close().await;
}

#[tokio::test]
async fn disarms_without_durably_pausing_when_cancellation_belongs_to_unrelated_human_work() {
    let h = Harness::new(vec![Entry::Hang]).await;
    h.agent
        .followup(user_message("inspect something first"))
        .unwrap();
    wait_for_requests(&h.adapter, 1).await;
    let created = create_goal(&h, "continue after inspection", 8);
    h.agent
        .cancel(AgentCancelCause::User, CancelOptions::default())
        .unwrap();
    h.agent.when_idle().await;
    let goal = h.goals().get(&h.agent).unwrap().unwrap();
    assert_eq!(goal.goal.id, created.goal.id);
    assert_eq!(goal.goal.revision, created.goal.revision);
    assert_eq!(goal.goal.phase, GoalPhase::Active);
    assert_eq!(goal.activation, GoalActivation::Disarmed);
    assert_eq!(goal.rounds_started, 0);
    h.close().await;
}

#[tokio::test]
async fn pauses_and_drops_a_reserved_round_when_cancellation_lands_before_pre_step() {
    let h = Harness::new(vec![]).await;
    let agent = h.agent.clone();
    let fired = Arc::new(Mutex::new(false));
    let once = fired.clone();
    h.ctx
        .on_contained::<AgentInboxClaimed>(move |event| {
            if Arc::ptr_eq(&event.agent, &agent)
                && event.message.source.kind == "goal"
                && !*lock(&once)
            {
                *lock(&once) = true;
                event
                    .agent
                    .cancel(AgentCancelCause::User, CancelOptions::default())?;
            }
            Ok(())
        })
        .unwrap();
    create_goal(&h, "do not start yet", 8);
    let goal = wait_for_goal(&h, |goal| goal.goal.phase == GoalPhase::Paused).await;
    assert_eq!(goal.rounds_started, 0);
    assert_eq!(goal.activation, GoalActivation::Disarmed);
    assert_eq!(h.adapter.request_count(), 0);
    h.close().await;
}

#[tokio::test]
async fn lets_a_model_initiated_pause_finish_its_own_turn() {
    type PauseTarget = (hoh_kernel::Service<Goals>, Arc<dyn Agent>);
    let target = Arc::new(Mutex::new(None::<PauseTarget>));
    let callback_target = target.clone();
    let h = Harness::new(vec![Entry::Callback(Arc::new(move || {
        if let Some((goals, agent)) = lock(&callback_target).clone() {
            let goal = goals.get(&agent).unwrap().unwrap();
            goals.pause(&agent, &goal.as_ref()).unwrap();
        }
        text_response("paused myself")
    }))])
    .await;
    *lock(&target) = Some((h.goals(), h.agent.clone()));
    create_goal(&h, "pause myself", 2);
    let goal = wait_for_goal(&h, |goal| goal.goal.phase == GoalPhase::Paused).await;
    h.agent.when_idle().await;
    assert_eq!(goal.rounds_started, 1);
    assert_eq!(h.adapter.request_count(), 1);
    let events = h.agent.session().snapshot_events(None, None);
    let reasons = events
        .iter()
        .filter(|event| event.event_type() == "turn/end")
        .filter_map(|event| event.data()["reason"]["kind"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    assert!(reasons.iter().any(|reason| reason == "completed"));
    assert!(!reasons.iter().any(|reason| reason == "aborted"));
    h.close().await;
}

#[tokio::test]
async fn makes_a_reserved_round_stale_when_a_listener_queues_human_work_behind_it() {
    let h = Harness::new(vec![
        Entry::Chunks(text_response("human batch")),
        Entry::Chunks(text_response("later goal")),
    ])
    .await;
    let inserted = Arc::new(Mutex::new(false));
    let once = inserted.clone();
    let agent = h.agent.clone();
    h.ctx
        .on_contained::<AgentInboxInserted>(move |event| {
            if Arc::ptr_eq(&event.agent, &agent)
                && event.message.source.kind == "goal"
                && !*lock(&once)
            {
                *lock(&once) = true;
                event
                    .agent
                    .followup(user_message("human joined the pending batch"))?;
            }
            Ok(())
        })
        .unwrap();
    create_goal(&h, "yield to nested human input", 1);
    wait_for_goal(&h, |goal| goal.goal.phase == GoalPhase::Blocked).await;
    assert_eq!(h.adapter.request_count(), 2);
    assert!(
        h.adapter
            .request_text(0)
            .contains("human joined the pending batch")
    );
    assert!(!h.adapter.request_text(0).contains("<goal_round>"));
    assert!(h.adapter.request_text(1).contains("<goal_round>"));
    h.close().await;
}

#[tokio::test]
async fn blocks_a_queued_reservation_made_stale_by_a_goal_edit_and_continues_the_new_revision() {
    let h = Harness::new(vec![Entry::Chunks(text_response("new revision"))]).await;
    let edited = Arc::new(Mutex::new(false));
    let once = edited.clone();
    let goals = h.goals();
    h.ctx
        .on_contained::<AgentInboxInserted>(move |event| {
            if event.message.source.kind == "goal" && !*lock(&once) {
                *lock(&once) = true;
                let current = goals.get(&event.agent)?.expect("goal");
                goals.edit(
                    &event.agent,
                    &current.as_ref(),
                    EditGoalRequest {
                        objective: Some("new objective".into()),
                        max_goal_rounds: None,
                    },
                )?;
            }
            Ok(())
        })
        .unwrap();
    create_goal(&h, "old objective", 1);
    let goal = wait_for_goal(&h, |goal| goal.goal.phase == GoalPhase::Blocked).await;
    assert_eq!(goal.goal.objective, "new objective");
    assert_eq!(goal.goal.revision, 3);
    assert_eq!(goal.rounds_started, 1);
    h.close().await;
}

#[tokio::test]
async fn does_not_block_a_goal_that_downstream_paused_before_rejecting_its_prompt() {
    let h = Harness::new(vec![]).await;
    let goals = h.goals();
    h.ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| {
            let goals = goals.clone();
            async move {
                if event
                    .messages
                    .iter()
                    .any(|message| message.source.kind == "goal")
                {
                    let goal = goals.get(&event.agent)?.expect("goal");
                    goals.pause(&event.agent, &goal.as_ref())?;
                    Ok(PreStepDecision::Reject)
                } else {
                    next.run().await
                }
            }
        })
        .unwrap();
    create_goal(&h, "pause before rejection", 8);
    let goal = wait_for_goal(&h, |goal| goal.goal.phase == GoalPhase::Paused).await;
    assert!(goal.goal.blocked_reason.is_none());
    assert_eq!(h.adapter.request_count(), 0);
    h.close().await;
}

#[tokio::test]
async fn fails_closed_when_a_downstream_pre_step_hook_throws() {
    let h = Harness::new(vec![]).await;
    h.ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(|event, next| async move {
            if event
                .messages
                .first()
                .is_some_and(|message| message.source.kind == "goal")
            {
                Err("downstream pre-step hook exploded".into())
            } else {
                next.run().await
            }
        })
        .unwrap();
    create_goal(&h, "survive a throwing hook", 1);
    let goal = wait_for_goal(&h, |goal| goal.activation == GoalActivation::Disarmed).await;
    assert_eq!(goal.goal.phase, GoalPhase::Active);
    assert_eq!(goal.rounds_started, 0);
    assert_eq!(h.adapter.request_count(), 0);
    assert!(h.agent.inbox().next_turn().is_empty());
    h.close().await;
}

#[tokio::test]
async fn disarms_instead_of_reserving_another_round_when_the_round_checkpoint_fails() {
    let h = Harness::new(vec![Entry::Chunks(text_response("round one ran"))]).await;
    let flushes = Arc::new(AtomicUsize::new(0));
    let seen = flushes.clone();
    SessionStore::on_flush(&h.ctx, move |_| {
        let number = seen.fetch_add(1, Ordering::SeqCst) + 1;
        async move {
            if number >= 2 {
                Err("round checkpoint failed".into())
            } else {
                Ok(())
            }
        }
    })
    .unwrap();
    create_goal(&h, "no autonomous rounds without durability", 5);
    let goal = wait_for_goal(&h, |goal| goal.activation == GoalActivation::Disarmed).await;
    assert_eq!(goal.rounds_started, 1);
    assert_eq!(goal.goal.phase, GoalPhase::Active);
    assert_eq!(h.adapter.request_count(), 1);
    h.close().await;
}

#[tokio::test]
async fn reserves_the_next_round_only_after_the_settled_round_checkpoint_succeeds() {
    let h = Harness::new(vec![
        Entry::Chunks(text_response("round one")),
        Entry::Chunks(text_response("round two")),
    ])
    .await;
    let observed = Arc::new(Mutex::new(Vec::new()));
    let counts = observed.clone();
    let adapter = h.adapter.clone();
    SessionStore::on_flush(&h.ctx, move |_| {
        lock(&counts).push(adapter.request_count());
        async { Ok(()) }
    })
    .unwrap();
    create_goal(&h, "checkpoint between rounds", 2);
    let goal = wait_for_goal(&h, |goal| goal.goal.phase == GoalPhase::Blocked).await;
    assert_eq!(goal.rounds_started, 2);
    assert!(lock(&observed).contains(&1));
    h.close().await;
}

#[tokio::test]
async fn does_not_join_descendants_for_an_agent_without_an_armed_goal() {
    let h = Harness::new(vec![Entry::Chunks(text_response("plain answer"))]).await;
    let gate = DescendantGate::install(&h).await;
    h.agent.followup(user_message("no goal here")).unwrap();
    wait_for_requests(&h.adapter, 1).await;
    h.agent.when_idle().await;
    assert_eq!(gate.result_calls.load(Ordering::SeqCst), 1);
    gate.release();
    h.close().await;
}

#[tokio::test]
async fn reserves_no_round_for_a_goal_paused_while_descendants_progress() {
    let h = Harness::new(vec![]).await;
    let gate = DescendantGate::install(&h).await;
    let created = create_goal(&h, "pause during the wait", 8);
    gate.wait_until_joined().await;
    h.goals().pause(&h.agent, &created.as_ref()).unwrap();
    gate.release();
    h.agent.when_idle().await;
    let goal = h.goals().get(&h.agent).unwrap().unwrap();
    assert_eq!(goal.goal.phase, GoalPhase::Paused);
    assert_eq!(goal.rounds_started, 0);
    assert_eq!(h.adapter.request_count(), 0);
    h.close().await;
}

#[tokio::test]
async fn lets_the_driver_unload_while_descendants_progress() {
    let h = Harness::new(vec![]).await;
    let gate = DescendantGate::install(&h).await;
    create_goal(&h, "unload while delegates run", 8);
    gate.wait_until_joined().await;
    h.driver.dispose().await;
    let goal = h.goals().get(&h.agent).unwrap().unwrap();
    assert_eq!(goal.rounds_started, 0);
    assert_eq!(goal.activation, GoalActivation::Disarmed);
    assert_eq!(h.adapter.request_count(), 0);
    gate.release();
    h.close().await;
}

#[tokio::test]
async fn resets_process_local_scheduling_state_at_a_session_start_edge() {
    let h = Harness::new(vec![Entry::Chunks(text_response("after explicit resume"))]).await;
    let gate = DescendantGate::install(&h).await;
    let created = create_goal(&h, "restart safely", 1);
    gate.wait_until_joined().await;
    agent_events(&h.ctx, &h.agent)
        .serial::<AgentCreated, ()>(|agent| AgentCreated {
            agent,
            source: SessionStartSource::Resume,
            signal: None,
        })
        .await
        .unwrap();
    gate.release();
    tokio::task::yield_now().await;
    assert_eq!(
        h.goals().get(&h.agent).unwrap().unwrap().activation,
        GoalActivation::Disarmed
    );
    assert_eq!(h.adapter.request_count(), 0);
    h.goals().resume(&h.agent, &created.as_ref()).unwrap();
    wait_for_goal(&h, |goal| goal.goal.phase == GoalPhase::Blocked).await;
    assert_eq!(h.adapter.request_count(), 1);
    h.close().await;
}

#[tokio::test]
async fn does_not_reserve_again_when_a_stopped_goal_observer_queues_cancel_scoped_work() {
    let h = Harness::new(vec![Entry::Chunks(text_response("human follow-up"))]).await;
    h.ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(|event, next| async move {
            if event
                .messages
                .first()
                .is_some_and(|message| message.source.kind == "goal")
            {
                Ok(PreStepDecision::Reject)
            } else {
                next.run().await
            }
        })
        .unwrap();
    h.ctx
        .on_contained::<GoalChangedEvent>(|event| {
            if event.change.operation == GoalOperation::Block {
                event.agent.followup(user_message("inspect the blocker"))?;
            }
            Ok(())
        })
        .unwrap();
    create_goal(&h, "stop and inspect", 8);
    wait_for_goal(&h, |goal| goal.goal.phase == GoalPhase::Blocked).await;
    h.agent.when_idle().await;
    assert_eq!(h.adapter.request_count(), 0);
    assert_eq!(h.agent.inbox().next_turn().len(), 1);
    assert_eq!(
        h.agent.inbox().next_turn()[0].content[0].text_value(),
        Some("inspect the blocker")
    );
    h.close().await;
}

#[tokio::test]
async fn keeps_a_resumed_goal_running_when_the_pause_turn_has_not_yet_converged() {
    let h = Harness::new(vec![
        Entry::Hang,
        Entry::Chunks(text_response("resumed round")),
    ])
    .await;
    create_goal(&h, "pause then resume", 2);
    wait_for_requests(&h.adapter, 1).await;
    let current = h.goals().get(&h.agent).unwrap().unwrap();
    let paused = h.goals().pause(&h.agent, &current.as_ref()).unwrap();
    h.goals().resume(&h.agent, &paused.as_ref()).unwrap();
    let goal = wait_for_goal(&h, |goal| goal.goal.phase == GoalPhase::Blocked).await;
    assert_eq!(goal.rounds_started, 2);
    assert_eq!(goal.goal.blocked_reason.unwrap().code, "round-limit");
    assert_eq!(h.adapter.request_count(), 2);
    h.close().await;
}

#[tokio::test]
async fn settles_a_goal_round_from_its_successful_retry_turn_not_the_failed_original() {
    let h = Harness::new(vec![
        Entry::Fail("transient"),
        Entry::Chunks(text_response("retry succeeded")),
    ])
    .await;
    let retried = Arc::new(Mutex::new(false));
    let once = retried.clone();
    h.ctx
        .on_waterfall::<AgentRequestError, Option<RequestErrorAction>, _>(move |_, next| {
            let once = once.clone();
            async move {
                if !*lock(&once) {
                    *lock(&once) = true;
                    Ok(Some(RequestErrorAction::Retry))
                } else {
                    next.run().await
                }
            }
        })
        .unwrap();
    create_goal(&h, "survive a transient failure", 1);
    let goal = wait_for_goal(&h, |goal| goal.goal.phase == GoalPhase::Blocked).await;
    assert_eq!(goal.rounds_started, 1);
    assert_eq!(goal.goal.blocked_reason.unwrap().code, "round-limit");
    assert_eq!(h.adapter.request_count(), 2);
    h.close().await;
}

#[tokio::test]
async fn does_not_double_clear_when_a_throwing_hook_already_cancelled_the_round() {
    let h = Harness::new(vec![]).await;
    let fired = Arc::new(Mutex::new(false));
    let once = fired.clone();
    h.ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| {
            let once = once.clone();
            async move {
                if event
                    .messages
                    .first()
                    .is_some_and(|message| message.source.kind == "goal")
                    && !*lock(&once)
                {
                    *lock(&once) = true;
                    event
                        .agent
                        .cancel(AgentCancelCause::User, CancelOptions::default())?;
                    Err("hook cancelled then exploded".into())
                } else {
                    next.run().await
                }
            }
        })
        .unwrap();
    create_goal(&h, "cancel then throw", 8);
    let goal = wait_for_goal(&h, |goal| goal.goal.phase == GoalPhase::Paused).await;
    h.agent.when_idle().await;
    assert_eq!(goal.rounds_started, 0);
    assert_eq!(h.adapter.request_count(), 0);
    h.close().await;
}

#[tokio::test]
async fn rejects_the_step_when_downstream_cancellation_clears_the_reservation() {
    let h = Harness::new(vec![]).await;
    let cancelled = Arc::new(Mutex::new(false));
    let once = cancelled.clone();
    h.ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| {
            let once = once.clone();
            async move {
                if event
                    .messages
                    .first()
                    .is_some_and(|message| message.source.kind == "goal")
                    && !*lock(&once)
                {
                    *lock(&once) = true;
                    event
                        .agent
                        .cancel(AgentCancelCause::User, CancelOptions::default())?;
                }
                next.run().await
            }
        })
        .unwrap();
    create_goal(&h, "cancel during pre-step", 8);
    let goal = wait_for_goal(&h, |goal| goal.goal.phase == GoalPhase::Paused).await;
    h.agent.when_idle().await;
    assert_eq!(goal.rounds_started, 0);
    assert_eq!(h.adapter.request_count(), 0);
    h.close().await;
}

#[tokio::test]
async fn runs_human_work_that_arrives_while_descendants_progress_before_the_next_round() {
    let h = Harness::new(vec![
        Entry::Chunks(text_response("human answer")),
        Entry::Chunks(text_response("goal answer")),
    ])
    .await;
    let gate = DescendantGate::install(&h).await;
    create_goal(&h, "yield to human input", 1);
    gate.wait_until_joined().await;
    h.agent
        .followup(user_message("human during the wait"))
        .unwrap();
    wait_for_requests(&h.adapter, 1).await;
    gate.release();
    wait_for_goal(&h, |goal| goal.rounds_started == 1).await;
    wait_for_requests(&h.adapter, 2).await;
    assert!(!h.adapter.request_text(0).contains("<goal_round>"));
    assert!(h.adapter.request_text(1).contains("<goal_round>"));
    h.close().await;
}

#[tokio::test]
async fn contains_a_checkpoint_failure_after_a_clear_notification_leaves_no_current_goal() {
    let h = Harness::new(vec![]).await;
    SessionStore::on_flush(&h.ctx, |_| async { Err("clear checkpoint failed".into()) }).unwrap();
    agent_events(&h.ctx, &h.agent).emit(|agent| GoalChangedEvent {
        agent,
        change: GoalChanged {
            operation: GoalOperation::Clear,
            r#ref: GoalRef {
                id: "cleared-goal".into(),
                revision: 2,
            },
            goal: None,
        },
    });
    tokio::task::yield_now().await;
    assert!(h.goals().get(&h.agent).unwrap().is_none());
    assert_eq!(h.adapter.request_count(), 0);
    h.close().await;
}

#[tokio::test]
async fn disarms_instead_of_continuing_when_a_plugin_reports_a_post_turn_persistence_failure() {
    let h = Harness::new(vec![Entry::Chunks(text_response("round one"))]).await;
    let subject = h.agent.clone();
    let ctx = h.ctx.clone();
    h.ctx
        .on_contained::<SessionEventEmitted>(move |event| {
            if event.session == *subject.session() && event.event.event_type() == "turn/end" {
                let turn = event.event.data()["turn"].as_u64().unwrap_or(1);
                agent_events(&ctx, &subject).emit(|agent| AgentError {
                    agent,
                    turn,
                    step: 1,
                    error: hoh_error::SharedError::new("post-turn flush failed"),
                });
            }
            Ok(())
        })
        .unwrap();
    create_goal(&h, "stop when durability is lost", 8);
    let goal = wait_for_goal(&h, |goal| goal.activation == GoalActivation::Disarmed).await;
    h.agent.when_idle().await;
    assert_eq!(goal.goal.phase, GoalPhase::Active);
    assert_eq!(goal.rounds_started, 1);
    assert_eq!(h.adapter.request_count(), 1);
    h.close().await;
}

#[tokio::test]
async fn keeps_a_running_goal_active_when_its_agent_is_disposed_mid_round() {
    let h = Harness::new(vec![Entry::Hang]).await;
    let mut options = CreateAgentOptions::new("goal-agent-disposed".into());
    options.agent_options = Some(AgentOptions {
        provider: Some(Live::Value("mock".into())),
        model: Some(Live::Value("mock".into())),
        ..Default::default()
    });
    let handle = h
        .ctx
        .get::<Agents>()
        .unwrap()
        .create(&h.ctx, options)
        .await
        .unwrap();
    h.goals()
        .create(
            &handle.agent,
            CreateGoalRequest {
                objective: "survive agent disposal".into(),
                max_goal_rounds: Some(8),
            },
        )
        .unwrap();
    wait_for_requests(&h.adapter, 1).await;
    handle.dispose().await.unwrap();
    let events = handle.agent.session().snapshot_events(None, None);
    let operations = events
        .iter()
        .filter(|event| event.event_type() == "goal/change")
        .filter_map(|event| event.data()["operation"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(operations, ["create"]);
    h.close().await;
}

#[tokio::test]
async fn waits_for_work_queued_by_a_pause_observer_before_considering_the_next_round() {
    let h = Harness::new(vec![
        Entry::Hang,
        Entry::Chunks(text_response("inspection answer")),
    ])
    .await;
    let subject = h.agent.clone();
    h.ctx
        .on_contained::<GoalChangedEvent>(move |event| {
            if Arc::ptr_eq(&event.agent, &subject) && event.change.operation == GoalOperation::Pause
            {
                event.agent.followup(user_message("inspect the pause"))?;
            }
            Ok(())
        })
        .unwrap();
    create_goal(&h, "pause then inspect", 8);
    wait_for_requests(&h.adapter, 1).await;
    h.agent
        .cancel(AgentCancelCause::User, CancelOptions::default())
        .unwrap();
    wait_for_requests(&h.adapter, 2).await;
    h.agent.when_idle().await;
    let goal = h.goals().get(&h.agent).unwrap().unwrap();
    assert_eq!(goal.goal.phase, GoalPhase::Paused);
    assert_eq!(goal.rounds_started, 1);
    assert!(h.adapter.request_text(1).contains("inspect the pause"));
    h.close().await;
}

#[tokio::test]
async fn ignores_session_events_without_an_exact_owning_agent_and_retires_disposed_agent_state() {
    let h = Harness::new(vec![]).await;
    let sessions = h.ctx.get::<Sessions>().unwrap();
    let orphan = sessions
        .create(
            &h.ctx,
            Some("goal-session-orphan".into()),
            CreateSessionOptions::default(),
        )
        .unwrap();
    orphan
        .append("turn/start", json!({"turn":1}), None)
        .unwrap();
    orphan
        .append(
            "turn/end",
            json!({"turn":1,"reason":{"kind":"completed"}}),
            None,
        )
        .unwrap();
    let mut options = CreateAgentOptions::new("goal-session-disposed".into());
    options.agent_options = Some(AgentOptions {
        provider: Some(Live::Value("mock".into())),
        model: Some(Live::Value("mock".into())),
        ..Default::default()
    });
    let handle = h
        .ctx
        .get::<Agents>()
        .unwrap()
        .create(&h.ctx, options)
        .await
        .unwrap();
    handle.dispose().await.unwrap();
    assert!(
        h.ctx
            .get::<Agents>()
            .unwrap()
            .get(handle.agent.id())
            .is_none()
    );
    h.close().await;
}

#[tokio::test]
async fn withdraws_a_round_parked_behind_cancelled_human_work_so_later_input_runs() {
    let h = Harness::new(vec![
        Entry::Hang,
        Entry::Hang,
        Entry::Chunks(text_response("answered 1")),
    ])
    .await;
    create_goal(&h, "park behind human work", 8);
    wait_for_requests(&h.adapter, 1).await;
    h.agent.followup(user_message("human X")).unwrap();
    h.agent
        .cancel(AgentCancelCause::User, CancelOptions { keep_inbox: true })
        .unwrap();
    let paused = wait_for_goal(&h, |goal| goal.goal.phase == GoalPhase::Paused).await;
    h.goals().resume(&h.agent, &paused.as_ref()).unwrap();
    wait_for_requests(&h.adapter, 2).await;
    assert_eq!(
        h.agent
            .inbox()
            .next_turn()
            .iter()
            .map(|message| message.source.kind.as_str())
            .collect::<Vec<_>>(),
        ["goal"]
    );

    h.agent
        .cancel(AgentCancelCause::User, CancelOptions { keep_inbox: true })
        .unwrap();
    h.agent.when_idle().await;
    assert!(h.agent.inbox().next_turn().is_empty());
    let goal = h.goals().get(&h.agent).unwrap().unwrap();
    assert_eq!(goal.goal.phase, GoalPhase::Active);
    assert_eq!(goal.activation, GoalActivation::Disarmed);
    h.agent.followup(user_message("human 1")).unwrap();
    wait_for_requests(&h.adapter, 3).await;
    assert!(h.adapter.request_text(2).contains("human 1"));
    assert!(h.agent.inbox().next_turn().is_empty());
    h.close().await;
}

#[tokio::test]
async fn a_retry_turn_on_a_non_goal_failure_leaves_the_goal_reservation_untouched() {
    let h = Harness::new(vec![
        Entry::Fail("transient on human turn"),
        Entry::Chunks(text_response("human retry succeeded")),
        Entry::Chunks(text_response("goal round ran")),
    ])
    .await;
    let retried = Arc::new(Mutex::new(false));
    let once = retried.clone();
    h.ctx
        .on_waterfall::<AgentRequestError, Option<RequestErrorAction>, _>(move |_, next| {
            let once = once.clone();
            async move {
                if !*lock(&once) {
                    *lock(&once) = true;
                    Ok(Some(RequestErrorAction::Retry))
                } else {
                    next.run().await
                }
            }
        })
        .unwrap();
    create_goal(&h, "ignore foreign retries", 1);
    h.agent.followup(user_message("human work")).unwrap();
    let goal = wait_for_goal(&h, |goal| goal.goal.phase == GoalPhase::Blocked).await;
    assert_eq!(goal.rounds_started, 1);
    assert_eq!(goal.goal.blocked_reason.unwrap().code, "round-limit");
    assert_eq!(h.adapter.request_count(), 3);
    h.close().await;
}

#[tokio::test]
async fn does_not_reblock_a_goal_the_downstream_veto_already_saw_cancelled() {
    let h = Harness::new(vec![]).await;
    let vetoed = Arc::new(Mutex::new(false));
    let once = vetoed.clone();
    h.ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>(move |event, next| {
            let once = once.clone();
            async move {
                if event
                    .messages
                    .first()
                    .is_some_and(|message| message.source.kind == "goal")
                    && !*lock(&once)
                {
                    *lock(&once) = true;
                    event
                        .agent
                        .cancel(AgentCancelCause::User, CancelOptions::default())?;
                    Ok(PreStepDecision::Reject)
                } else {
                    next.run().await
                }
            }
        })
        .unwrap();
    create_goal(&h, "veto after cancellation", 8);
    let goal = wait_for_goal(&h, |goal| goal.goal.phase == GoalPhase::Paused).await;
    h.agent.when_idle().await;
    assert_eq!(goal.rounds_started, 0);
    assert_eq!(goal.activation, GoalActivation::Disarmed);
    assert!(goal.goal.blocked_reason.is_none());
    assert_eq!(h.adapter.request_count(), 0);
    h.close().await;
}

#[tokio::test]
async fn disarms_when_a_round_turn_end_cannot_commit() {
    let h = Harness::new(vec![Entry::Chunks(text_response("round ran"))]).await;
    let session = h.agent.session().clone();
    h.ctx
        .on_dispatch(move |_, name, payload| {
            if name != "session/event" {
                return;
            }
            let Some(emitted) = payload.downcast_ref::<SessionEventEmitted>() else {
                return;
            };
            if emitted.session == session && emitted.event.event_type() == "turn/end" {
                emitted
                    .validation
                    .reject("turn close permanently rejected".into());
            }
        })
        .unwrap();
    create_goal(&h, "survive a lost turn end", 8);
    wait_for_requests(&h.adapter, 1).await;
    h.agent.when_idle().await;
    tokio::task::yield_now().await;
    let goal = wait_for_goal(&h, |goal| goal.activation == GoalActivation::Disarmed).await;
    assert_eq!(goal.goal.phase, GoalPhase::Active);
    assert_eq!(h.adapter.request_count(), 1);
    h.close().await;
}

#[tokio::test]
async fn ignores_a_post_turn_failure_reported_for_a_retired_agent() {
    let h = Harness::new(vec![Entry::Chunks(text_response("ordinary work"))]).await;
    let mut options = CreateAgentOptions::new("goal-session-retired".into());
    options.agent_options = Some(AgentOptions {
        provider: Some(Live::Value("mock".into())),
        model: Some(Live::Value("mock".into())),
        ..Default::default()
    });
    let handle = h
        .ctx
        .get::<Agents>()
        .unwrap()
        .create(&h.ctx, options)
        .await
        .unwrap();
    handle
        .agent
        .followup(user_message("one ordinary turn"))
        .unwrap();
    handle.agent.when_idle().await;
    let events = handle.agent.session().snapshot_events(None, None);
    let turn = events
        .iter()
        .rev()
        .find(|event| event.event_type() == "turn/end")
        .and_then(|event| event.data()["turn"].as_u64())
        .unwrap();
    handle.dispose().await.unwrap();
    agent_events(&h.ctx, &handle.agent).emit(|agent| AgentError {
        agent,
        turn,
        step: 1,
        error: hoh_error::SharedError::new("late flush failure"),
    });
    assert!(
        h.ctx
            .get::<Agents>()
            .unwrap()
            .get(handle.agent.id())
            .is_none()
    );
    assert!(
        h.ctx
            .logger()
            .buffer()
            .iter()
            .all(|record| !record.message.contains("goal-round-driver"))
    );
    h.close().await;
}

#[tokio::test]
async fn cancels_an_accepted_queued_round_and_awaits_its_driver_task_during_teardown() {
    let h = Harness::new(vec![]).await;
    let (send, receive) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(send)));
    let output = sender.clone();
    let driver = h.driver.clone();
    h.ctx
        .on_contained::<AgentInboxInserted>(move |event| {
            if event.message.source.kind == "goal"
                && let Some(send) = lock(&output).take()
            {
                let driver = driver.clone();
                drop(tokio::spawn(async move {
                    driver.dispose().await;
                    let _ = send.send(());
                }));
            }
            Ok(())
        })
        .unwrap();
    create_goal(&h, "unload while queued", 8);
    tokio::time::timeout(Duration::from_secs(5), receive)
        .await
        .unwrap()
        .unwrap();
    let goal = h.goals().get(&h.agent).unwrap().unwrap();
    assert_eq!(goal.goal.phase, GoalPhase::Active);
    assert_eq!(goal.activation, GoalActivation::Disarmed);
    assert_eq!(goal.rounds_started, 0);
    assert_eq!(h.adapter.request_count(), 0);
    h.close().await;
}

#[tokio::test]
async fn awaits_a_claimed_reservation_stuck_in_pre_step_during_teardown() {
    let h = Harness::new(vec![]).await;
    let release = Arc::new(Notify::new());
    let entered = Arc::new(Notify::new());
    let once = Arc::new(Mutex::new(false));
    h.ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>({
            let release = release.clone();
            let entered = entered.clone();
            let once = once.clone();
            move |event, next| {
                let release = release.clone();
                let entered = entered.clone();
                let once = once.clone();
                async move {
                    if event
                        .messages
                        .first()
                        .is_some_and(|message| message.source.kind == "goal")
                        && !*lock(&once)
                    {
                        *lock(&once) = true;
                        entered.notify_one();
                        release.notified().await;
                    }
                    next.run().await
                }
            }
        })
        .unwrap();
    create_goal(&h, "unload during pre-step", 8);
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    let driver = h.driver.clone();
    let disposal = tokio::spawn(async move { driver.dispose().await });
    wait_for_goal(&h, |goal| goal.activation == GoalActivation::Disarmed).await;
    release.notify_one();
    disposal.await.unwrap();
    let goal = h.goals().get(&h.agent).unwrap().unwrap();
    assert_eq!(goal.goal.phase, GoalPhase::Active);
    assert_eq!(goal.rounds_started, 0);
    assert_eq!(h.adapter.request_count(), 0);
    assert!(
        h.agent
            .session()
            .snapshot_events(None, None)
            .iter()
            .any(|event| event.event_type() == "turn/start")
    );
    h.close().await;
}

#[tokio::test]
async fn restores_non_goal_step_context_when_a_claimed_reservation_becomes_stale() {
    let h = Harness::new(vec![
        Entry::Chunks(text_response("side contexts")),
        Entry::Chunks(text_response("revised goal")),
    ])
    .await;
    let claimed_context = create_user_message(NewUserMessage {
        content: vec![ContentBlock::text("claimed context to restore")],
        source: MessageSource::new("test"),
    });
    let round_zero_context = create_user_message(NewUserMessage {
        content: vec![ContentBlock::text("obsolete goal context")],
        source: MessageSource {
            kind: "goal".into(),
            data: Map::from_iter([
                ("goalId".into(), json!("old-goal")),
                ("revision".into(), json!(1)),
                ("round".into(), json!(0.0)),
            ]),
        },
    });
    let queued_step_context = create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(
            "context already queued for the next step",
        )],
        source: MessageSource::new("test"),
    });
    let queued_turn_context = create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(
            "context already queued for the next turn",
        )],
        source: MessageSource::new("test"),
    });
    let staged = Arc::new(Mutex::new(false));
    h.ctx
        .on_contained::<AgentInboxInserted>({
            let staged = staged.clone();
            let claimed_context = claimed_context.clone();
            let round_zero_context = round_zero_context.clone();
            move |event| {
                if event.message.source.kind == "goal"
                    && event.message.source.data["round"].as_u64().unwrap_or(0) > 0
                    && !*lock(&staged)
                {
                    *lock(&staged) = true;
                    event
                        .agent
                        .inbox()
                        .prepend(InboxTarget::NextStep, claimed_context.clone())?;
                    event
                        .agent
                        .inbox()
                        .prepend(InboxTarget::NextStep, round_zero_context.clone())?;
                }
                Ok(())
            }
        })
        .unwrap();
    let edited = Arc::new(Mutex::new(false));
    let goals = h.goals();
    h.ctx
        .on_waterfall::<AgentPreStep, PreStepDecision, _>({
            let edited = edited.clone();
            let queued_step_context = queued_step_context.clone();
            let queued_turn_context = queued_turn_context.clone();
            move |event, next| {
                let edited = edited.clone();
                let goals = goals.clone();
                let queued_step_context = queued_step_context.clone();
                let queued_turn_context = queued_turn_context.clone();
                async move {
                    let decision = next.run().await?;
                    if !event.messages.iter().any(|message| {
                        message.source.kind == "goal"
                            && message.source.data["round"].as_u64().unwrap_or(0) > 0
                    }) || *lock(&edited)
                    {
                        return Ok(decision);
                    }
                    *lock(&edited) = true;
                    event
                        .agent
                        .inbox()
                        .prepend(InboxTarget::NextStep, queued_step_context.clone())?;
                    event
                        .agent
                        .inbox()
                        .append(InboxTarget::NextTurn, queued_turn_context.clone())?;
                    let goal = goals.get(&event.agent)?.expect("claimed goal");
                    goals.edit(
                        &event.agent,
                        &goal.as_ref(),
                        EditGoalRequest {
                            objective: Some("revised after claim".into()),
                            max_goal_rounds: None,
                        },
                    )?;
                    Ok(match decision {
                        PreStepDecision::Reject => PreStepDecision::Reject,
                        PreStepDecision::Enter {
                            mut messages,
                            starts_request_series,
                        } => {
                            messages.push(queued_step_context);
                            messages.push(queued_turn_context);
                            PreStepDecision::Enter {
                                messages,
                                starts_request_series,
                            }
                        }
                    })
                }
            }
        })
        .unwrap();
    create_goal(&h, "stale before admission", 1);
    let goal = wait_for_goal(&h, |goal| goal.goal.phase == GoalPhase::Blocked).await;
    assert_eq!(goal.goal.objective, "revised after claim");
    assert_eq!(goal.rounds_started, 1);
    assert_eq!(h.adapter.request_count(), 2);
    let side_contexts = h.adapter.request_text(0);
    assert!(side_contexts.contains("claimed context to restore"));
    assert!(side_contexts.contains("context already queued for the next step"));
    assert!(side_contexts.contains("context already queued for the next turn"));
    assert!(!side_contexts.contains("obsolete goal context"));
    assert!(!side_contexts.contains("<goal_round>"));
    let revised = h.adapter.request_text(1);
    assert!(revised.contains("revised after claim"));
    assert!(!revised.contains("stale before admission"));
    h.close().await;
}

#[tokio::test]
async fn keeps_terminal_agent_failure_disarmed_and_defers_queued_human_work_until_wakeup() {
    let h = Harness::new(vec![
        Entry::Fail("round one broke"),
        Entry::Chunks(text_response("human answer")),
    ])
    .await;
    let queued = Arc::new(Mutex::new(false));
    let once = queued.clone();
    let subject = h.agent.clone();
    h.ctx
        .on_contained::<SessionEventEmitted>(move |event| {
            if event.session == *subject.session()
                && event.event.event_type() == "user/message"
                && event.event.data()["source"]["kind"].as_str() == Some("goal")
                && !*lock(&once)
            {
                *lock(&once) = true;
                let agent = subject.clone();
                drop(tokio::spawn(async move {
                    tokio::task::yield_now().await;
                    let _ = agent
                        .inbox()
                        .append(InboxTarget::NextTurn, user_message("human interleaved"));
                }));
            }
            Ok(())
        })
        .unwrap();
    create_goal(&h, "survive a stale failure", 1);
    wait_for_goal(&h, |goal| {
        goal.goal.phase == GoalPhase::Active && goal.activation == GoalActivation::Disarmed
    })
    .await;
    tokio::task::yield_now().await;
    assert_eq!(h.adapter.request_count(), 1);
    assert_eq!(h.agent.inbox().next_turn().len(), 1);
    h.agent.steer(user_message("resume after failure")).unwrap();
    h.agent.when_idle().await;
    assert_eq!(h.adapter.request_count(), 2);
    assert!(h.adapter.request_text(1).contains("human interleaved"));
    assert!(h.adapter.request_text(1).contains("resume after failure"));
    h.close().await;
}

#[tokio::test]
async fn keeps_a_running_goal_active_when_only_its_agent_owner_unloads() {
    let h = Harness::new(vec![Entry::Hang]).await;
    let owner = h.ctx.plugin::<AgentOwner>(json!({})).unwrap();
    owner.settled().await.unwrap();
    let agent = lock(&OWNED_AGENT).take().expect("owner created agent");
    h.goals()
        .create(
            &agent,
            CreateGoalRequest {
                objective: "survive owner reload".into(),
                max_goal_rounds: Some(8),
            },
        )
        .unwrap();
    wait_for_requests(&h.adapter, 1).await;
    owner.dispose().await;
    let events = agent.session().snapshot_events(None, None);
    let operations = events
        .iter()
        .filter(|event| event.event_type() == "goal/change")
        .filter_map(|event| event.data()["operation"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(operations, ["create"]);
    assert_eq!(h.adapter.request_count(), 1);
    h.close().await;
}
