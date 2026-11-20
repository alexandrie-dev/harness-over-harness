//! Shared test stand: the services `tool-jobs` runs on, a fake owner agent with a delivery
//! recorder, and a controllable producer. Port of the helpers at the head of
//! `tests/tool-jobs.spec.ts`.
#![allow(dead_code)]

use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentInboxInserted, AgentOptions, AgentRegistryPlugin, AgentStatus, Agents,
    CancelOptions, Inbox, InboxTarget, MaintenanceFuture, MaintenanceTask, Model, emit_agent_event,
};
use hoh_agent_loop_testkit::unsupported_model;
use hoh_error::BoxError;
use hoh_jobs::{
    JobEvent, JobEventFilter, JobEventOwners, JobHandle, JobHooks, JobId, JobOutcome,
    JobOutcomeStatus, JobSpec, Jobs,
};
use hoh_jobs_local::LocalJobRegistryPlugin;
use hoh_kernel::{Context, Disposer, Fiber, Kernel, Level, Levels};
use hoh_llm::{
    ContentBlock, LlmPlugin, MessageId, MessageSource, NewUserMessage, ToolCallId, UserMessage,
    create_user_message,
};
use hoh_scope::ScopeKey;
use hoh_session::{
    AgentCancelCause, CreateSessionOptions, Session, SessionId, SessionPlugin, Sessions,
};
use hoh_session_projection::SessionProjectionRegistry;
use hoh_system_prompt::SystemPrompt;
use hoh_timeout::{AbortController, AbortSignal};
use hoh_tool_executor_native::NativeToolExecutorPlugin;
use hoh_tool_jobs::ToolJobs;
use hoh_tool_testkit::execute_tool_fixture;
use hoh_tools::{ToolExecutionInput, ToolExecutionResult, ToolRuntime};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::oneshot;

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub async fn mount<P: hoh_kernel::Plugin>(ctx: &Context, config: Value) -> Fiber {
    let fiber = ctx.plugin::<P>(config).expect("plugin mounts");
    fiber.settled().await.expect("plugin settles");
    fiber
}

/// `ctx.logger.warn` captured as text (the source spies on it).
pub fn capture_warnings(ctx: &Context) -> Arc<Mutex<Vec<String>>> {
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let sink = warnings.clone();
    ctx.logger()
        .exporter(
            Levels {
                default: Some(Level::Warn),
                ..Levels::default()
            },
            move |record| {
                if record.level == Level::Warn {
                    lock(&sink).push(record.message.clone());
                }
            },
        )
        .expect("exporter");
    warnings
}

/// A mutable fake next-step inbox per agent, so a test can queue input.
pub struct FakeInbox {
    pub next_step: Mutex<Vec<UserMessage>>,
    removed: Mutex<Vec<MessageId>>,
    remove_result: bool,
}

impl Inbox for FakeInbox {
    fn next_turn(&self) -> Vec<UserMessage> {
        vec![]
    }
    fn next_step(&self) -> Vec<UserMessage> {
        lock(&self.next_step).clone()
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
    fn remove(&self, message_id: &MessageId) -> Result<bool, BoxError> {
        lock(&self.removed).push(message_id.clone());
        Ok(self.remove_result)
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

/// The delivery surface a completion notice may reach on a fake owner.
#[derive(Clone)]
pub struct FakeDelivery {
    /// Defaults to `running`, the lane that never wakes, so notice-content tests pin one lane.
    pub status: AgentStatus,
    pub remove_result: bool,
    /// `inject` fails with this text.
    pub inject_error: Option<&'static str>,
}

impl Default for FakeDelivery {
    fn default() -> Self {
        Self {
            status: AgentStatus::Running,
            remove_result: true,
            inject_error: None,
        }
    }
}

impl FakeDelivery {
    pub fn idle() -> Self {
        Self {
            status: AgentStatus::Idle,
            ..Self::default()
        }
    }
}

/// A fake agent with a real session, registered in the agent registry with a dedicated lifecycle
/// scope.
pub struct FakeAgent {
    id: SessionId,
    session: Session,
    ctx: Context,
    options: AgentOptions,
    scope: ScopeKey,
    model: Arc<dyn Model>,
    pub inbox: Arc<FakeInbox>,
    status: AgentStatus,
    inject_error: Option<&'static str>,
    pub injected: Mutex<Vec<UserMessage>>,
    pub followed: Mutex<Vec<UserMessage>>,
}

impl Agent for FakeAgent {
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
        self.status
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
    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
        Ok(())
    }
    fn followup(&self, message: UserMessage) -> Result<(), BoxError> {
        lock(&self.followed).push(message);
        Ok(())
    }
    fn steer(&self, _: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }
    fn inject(&self, message: UserMessage) -> Result<(), BoxError> {
        if let Some(error) = self.inject_error {
            return Err(error.into());
        }
        lock(&self.injected).push(message);
        Ok(())
    }
}

/// A fake owner plus what a test reaches behind it.
#[derive(Clone)]
pub struct Owner {
    pub fake: Arc<FakeAgent>,
    pub agent: Arc<dyn Agent>,
    scope_fiber: Fiber,
    registration: Arc<Mutex<Option<Disposer>>>,
}

impl Owner {
    pub fn id(&self) -> SessionId {
        self.fake.id.clone()
    }
    pub fn session(&self) -> Session {
        self.fake.session.clone()
    }
    pub fn injected(&self) -> Vec<UserMessage> {
        lock(&self.fake.injected).clone()
    }
    pub fn followed(&self) -> Vec<UserMessage> {
        lock(&self.fake.followed).clone()
    }
    /// Forget delivered notices and queued input between rounds of a stress test.
    pub fn clear_injected(&self) {
        lock(&self.fake.injected).clear();
        lock(&self.fake.inbox.next_step).clear();
        lock(&self.fake.inbox.removed).clear();
    }
    pub fn removed(&self) -> Vec<MessageId> {
        lock(&self.fake.inbox.removed).clone()
    }
    /// `detachAgent`.
    pub async fn detach(&self) {
        let disposer = lock(&self.registration).take().expect("registered");
        disposer.dispose().await;
    }
    /// Dispose the agent's own lifecycle scope, which is what drains its owned jobs.
    pub async fn dispose_scope(&self) {
        self.scope_fiber.dispose().await;
    }
    /// `emitAgentEvent(ctx, owner, 'agent/inbox/claimed', ...)`.
    pub fn claim(&self, ctx: &Context, message: UserMessage) {
        emit_agent_event(ctx, &self.agent, |agent| hoh_agent::AgentInboxClaimed {
            agent,
            message,
            turn: 1,
        });
    }
    /// Report the notice as logged into the owner's session, the point it reaches the model.
    pub fn log_user_message(&self, message: &UserMessage) {
        self.fake
            .session()
            .append(
                "user/message",
                serde_json::to_value(message).unwrap(),
                Some(hoh_session::SurfaceIntent::append()),
            )
            .expect("log user message");
    }
    /// Durably queue one next-step message and publish its live insertion, as `ReactLoopInbox` does.
    pub fn insert_next_step(&self, ctx: &Context, text: &str) {
        self.insert(ctx, MessageSource::new("user"), text);
    }
    /// Queue one plugin notice, the other source a waiting step can receive.
    pub fn insert_notice(&self, ctx: &Context, text: &str) {
        let mut source = MessageSource::new("tool-jobs");
        source.data.insert("form".into(), json!("notice"));
        source.data.insert("summary".into(), json!(text));
        self.insert(ctx, source, text);
    }
    fn insert(&self, ctx: &Context, source: MessageSource, text: &str) {
        let message = create_user_message(NewUserMessage {
            content: vec![ContentBlock::text(text)],
            source,
        });
        lock(&self.fake.inbox.next_step).push(message.clone());
        emit_agent_event(ctx, &self.agent, |agent| AgentInboxInserted {
            agent,
            message,
        });
    }
}

/// The kernel and the mounted services.
pub struct Env {
    pub kernel: Kernel,
    pub ctx: Context,
    pub agents_fiber: Option<Fiber>,
    pub tools_fiber: Option<Fiber>,
    pub warnings: Arc<Mutex<Vec<String>>>,
    /// First-seen ids are test-local ordinals; the runtime id is opaque.
    issued: Arc<Mutex<HashMap<String, Vec<JobId>>>>,
}

impl Env {
    pub fn jobs(&self) -> hoh_kernel::Service<Jobs> {
        self.ctx.get::<Jobs>().expect("jobs service")
    }
    pub fn warnings(&self) -> Vec<String> {
        lock(&self.warnings).clone()
    }
    /// `startedId(ctx, 'bash-1')`.
    pub fn id(&self, legacy: &str) -> JobId {
        let (kind, ordinal) = legacy.rsplit_once('-').expect("ordinal");
        let ordinal: usize = ordinal.parse().expect("ordinal number");
        lock(&self.issued)
            .get(kind)
            .and_then(|ids| ids.get(ordinal - 1))
            .unwrap_or_else(|| panic!("job {legacy} was not registered"))
            .clone()
    }
    /// A fake agent registered in `ctx.agents`, with a real session.
    pub async fn owner(&self, session_id: &str, delivery: FakeDelivery) -> Owner {
        self.owner_in(session_id, delivery, None).await
    }
    pub async fn owner_in(
        &self,
        session_id: &str,
        delivery: FakeDelivery,
        scoped: Option<Context>,
    ) -> Owner {
        let scope_ctx = self.ctx.child("agent").expect("agent scope");
        let scope_fiber = scope_ctx.fiber();
        let id = SessionId::from(session_id);
        // A same-session replacement cannot attach a second session to the store: it gets a free
        // one, which emits nothing.
        let sessions = self.ctx.get::<Sessions>().expect("sessions");
        let session = sessions
            .create(&self.ctx, Some(id.clone()), CreateSessionOptions::default())
            .unwrap_or_else(|_| {
                Session::create(id.clone(), None, None, None, None).expect("session")
            });
        let fake = Arc::new(FakeAgent {
            id,
            session,
            ctx: scoped.unwrap_or(scope_ctx),
            options: AgentOptions::default(),
            scope: ScopeKey::new(),
            model: unsupported_model(),
            inbox: Arc::new(FakeInbox {
                next_step: Mutex::new(vec![]),
                removed: Mutex::new(vec![]),
                remove_result: delivery.remove_result,
            }),
            status: delivery.status,
            inject_error: delivery.inject_error,
            injected: Mutex::new(vec![]),
            followed: Mutex::new(vec![]),
        });
        let agent: Arc<dyn Agent> = fake.clone();
        let disposer = self
            .ctx
            .get::<Agents>()
            .expect("agents")
            .register(&self.ctx, agent.clone())
            .await
            .expect("register");
        Owner {
            fake,
            agent,
            scope_fiber,
            registration: Arc::new(Mutex::new(Some(disposer))),
        }
    }
}

/// A seam in the registry's `read`: while armed, the first read delegates, then parks its thread
/// until released. It stops a notice builder right after its consuming read.
#[derive(Default)]
pub struct ReadGate {
    armed: std::sync::atomic::AtomicBool,
    state: Mutex<(bool, bool)>,
    changed: std::sync::Condvar,
}

impl ReadGate {
    pub fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }
    /// Block until a read is parked at the seam.
    pub fn wait_parked(&self) {
        let mut state = lock(&self.state);
        while !state.0 {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
    pub fn release(&self) {
        lock(&self.state).1 = true;
        self.changed.notify_all();
    }
    fn park(&self) {
        if !self.armed.swap(false, Ordering::SeqCst) {
            return;
        }
        let mut state = lock(&self.state);
        state.0 = true;
        self.changed.notify_all();
        while !state.1 {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// The local registry with a [`ReadGate`] after every `read`.
struct GatedJobs {
    inner: hoh_jobs_local::LocalJobRegistry,
    gate: Arc<ReadGate>,
}

impl hoh_jobs::JobRegistryApi for GatedJobs {
    fn events(&self) -> &dyn hoh_jobs::JobEvents {
        self.inner.events()
    }
    fn start(&self, spec: JobSpec) -> Result<JobId, BoxError> {
        self.inner.start(spec)
    }
    fn list(&self, caller: Option<&SessionId>) -> Vec<hoh_jobs::JobView> {
        self.inner.list(caller)
    }
    fn get(&self, id: &JobId, caller: Option<&SessionId>) -> Result<hoh_jobs::JobView, BoxError> {
        self.inner.get(id, caller)
    }
    fn read(&self, id: &JobId, caller: Option<&SessionId>) -> Result<hoh_jobs::JobRead, BoxError> {
        let read = self.inner.read(id, caller);
        self.gate.park();
        read
    }
    fn read_at(
        &self,
        id: &JobId,
        from: f64,
        caller: Option<&SessionId>,
    ) -> Result<hoh_jobs::JobOutputRead, BoxError> {
        self.inner.read_at(id, from, caller)
    }
    fn kill(
        &self,
        id: &JobId,
        caller: Option<&SessionId>,
        reason: Option<&str>,
    ) -> Result<hoh_jobs::KillOutcome, BoxError> {
        self.inner.kill(id, caller, reason)
    }
    fn wait(
        &self,
        id: &JobId,
        timeout_ms: f64,
        caller: Option<&SessionId>,
        signal: Option<AbortSignal>,
    ) -> hoh_jobs::JobFuture<'static, Result<hoh_jobs::JobView, BoxError>> {
        self.inner.wait(id, timeout_ms, caller, signal)
    }
    fn remove(&self, id: &JobId, caller: Option<&SessionId>) -> Result<(), BoxError> {
        self.inner.remove(id, caller)
    }
    fn attach_controller(
        &self,
        ctx: &Context,
        name: &str,
    ) -> Result<Disposer, hoh_kernel::KernelError> {
        self.inner.attach_controller(ctx, name)
    }
}

/// Every service `tool-jobs` needs, without the plugin itself.
pub async fn base(registry: Value) -> (Kernel, Context, Fiber) {
    base_gated(registry, None).await
}

/// [`base`] with the registry's `read` behind `gate`.
pub async fn base_gated(registry: Value, gate: Option<Arc<ReadGate>>) -> (Kernel, Context, Fiber) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    mount::<LlmPlugin>(&ctx, json!({})).await;
    mount::<SessionPlugin>(&ctx, json!({})).await;
    mount::<SystemPrompt>(&ctx, json!({})).await;
    mount::<ToolRuntime>(&ctx, json!({})).await;
    mount::<SessionProjectionRegistry>(&ctx, json!({})).await;
    mount::<NativeToolExecutorPlugin>(&ctx, json!({})).await;
    let agents_fiber = mount::<AgentRegistryPlugin>(&ctx, json!({})).await;
    match gate {
        None => {
            mount::<LocalJobRegistryPlugin>(&ctx, registry).await;
        }
        Some(gate) => {
            let config = serde_json::from_value(registry).expect("registry config");
            let inner =
                hoh_jobs_local::LocalJobRegistry::new(ctx.clone(), config).expect("registry");
            hoh_jobs::provide_jobs(&ctx, Arc::new(GatedJobs { inner, gate }))
                .await
                .expect("jobs provided");
        }
    }
    (kernel, ctx, agents_fiber)
}

/// `setup(config, registry)`.
pub async fn setup(config: Value, registry: Value) -> Env {
    setup_gated(config, registry, None).await
}

pub async fn setup_gated(config: Value, registry: Value, gate: Option<Arc<ReadGate>>) -> Env {
    let env = setup_unmounted_gated(registry, gate).await;
    let tools_fiber = mount::<ToolJobs>(&env.ctx, config).await;
    Env {
        tools_fiber: Some(tools_fiber),
        ..env
    }
}

/// The services without the plugin; registered job ids are tracked from the start.
pub async fn setup_unmounted(registry: Value) -> Env {
    setup_unmounted_gated(registry, None).await
}

pub async fn setup_unmounted_gated(registry: Value, gate: Option<Arc<ReadGate>>) -> Env {
    let (kernel, ctx, agents_fiber) = base_gated(registry, gate).await;
    let warnings = capture_warnings(&ctx);
    let issued: Arc<Mutex<HashMap<String, Vec<JobId>>>> = Arc::default();
    let sink = issued.clone();
    ctx.get::<Jobs>()
        .expect("jobs")
        .events()
        .subscribe(
            &ctx,
            JobEventFilter::Owners {
                owners: JobEventOwners::All,
            },
            Arc::new(move |event| {
                if let JobEvent::Registered { job } = event {
                    lock(&sink)
                        .entry(job.kind.clone())
                        .or_default()
                        .push(job.id.clone());
                }
                Ok(())
            }),
        )
        .expect("subscribe");
    Env {
        kernel,
        ctx,
        agents_fiber: Some(agents_fiber),
        tools_fiber: None,
        warnings,
        issued,
    }
}

pub async fn setup_default() -> Env {
    setup(json!({}), json!({})).await
}

static NEXT_CALL: AtomicU64 = AtomicU64::new(1);

/// `call(ctx, name, args, agent)`.
pub async fn call(
    ctx: &Context,
    name: &str,
    args: Value,
    agent: Option<&Owner>,
) -> ToolExecutionResult {
    let signal = AbortController::new().signal();
    call_with_signal(ctx, name, args, agent, signal).await
}

pub async fn call_with_signal(
    ctx: &Context,
    name: &str,
    args: Value,
    agent: Option<&Owner>,
    signal: AbortSignal,
) -> ToolExecutionResult {
    let mut input = ToolExecutionInput::new(
        ToolCallId::from(format!(
            "call-{}",
            NEXT_CALL.fetch_add(1, Ordering::Relaxed)
        )),
        name,
        args,
        signal,
    );
    input.agent = agent.map(|owner| owner.agent.clone());
    execute_tool_fixture(ctx, input).await.expect("tool call")
}

/// `text(result)`: every text block, joined with nothing.
pub fn text(result: &ToolExecutionResult) -> String {
    result
        .content()
        .iter()
        .filter(|block| block.r#type == "text")
        .filter_map(ContentBlock::text_value)
        .collect()
}

/// `tick`: let the settlement continuation (a `done.then`) run.
pub async fn tick() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_millis(5)).await;
}

type Settle = Arc<Mutex<Option<oneshot::Sender<Result<JobOutcome, BoxError>>>>>;
type CancelHook = Box<dyn Fn(Option<&str>) -> Result<(), BoxError> + Send + Sync>;

#[derive(Default)]
pub struct ProducerOptions {
    pub kind: Option<&'static str>,
    pub label: Option<String>,
    pub owner: Option<SessionId>,
    pub output_limit_bytes: Option<u64>,
    /// Replaces the recording cancel hook.
    pub cancel: Option<CancelHook>,
    /// The recording cancel hook also settles the job with this outcome.
    pub settle_on_cancel: Option<JobOutcome>,
    /// Block the cancelling thread this long after settling, so another worker can observe the
    /// settlement before `kill` returns.
    pub cancel_stall: Option<std::time::Duration>,
}

/// A controllable producer spec: pushes output and progress through the handle the registry hands
/// its starter, settles `done` on demand, records cancels.
pub struct Producer {
    spec: Option<JobSpec>,
    settle: Settle,
    cancels: Arc<Mutex<Vec<Option<String>>>>,
    started: Arc<Mutex<Option<JobHandle>>>,
}

pub fn producer() -> Producer {
    producer_with(ProducerOptions::default())
}

pub fn owned_by(owner: &Owner) -> Producer {
    producer_with(ProducerOptions {
        owner: Some(owner.id()),
        ..ProducerOptions::default()
    })
}

pub fn producer_with(options: ProducerOptions) -> Producer {
    let (tx, rx) = oneshot::channel::<Result<JobOutcome, BoxError>>();
    let settle: Settle = Arc::new(Mutex::new(Some(tx)));
    let cancels = Arc::new(Mutex::new(Vec::new()));
    let started = Arc::new(Mutex::new(None));
    let recorded = cancels.clone();
    let on_cancel = options.settle_on_cancel;
    let stall = options.cancel_stall;
    let settler = settle.clone();
    let cancel: CancelHook = options.cancel.unwrap_or_else(|| {
        Box::new(move |reason| {
            lock(&recorded).push(reason.map(str::to_owned));
            if let Some(outcome) = &on_cancel
                && let Some(tx) = lock(&settler).take()
            {
                let _ = tx.send(Ok(outcome.clone()));
                if let Some(stall) = stall {
                    std::thread::sleep(stall);
                }
            }
            Ok(())
        })
    });
    let seen = started.clone();
    let spec = JobSpec {
        kind: options.kind.unwrap_or("bash").to_owned(),
        label: options.label.unwrap_or_else(|| "sleep 60".to_owned()),
        owner: options.owner,
        tool_call: None,
        output_limit_bytes: options.output_limit_bytes,
        output: None,
        run: Box::new(move |job| {
            *lock(&seen) = Some(job);
            Ok(JobHooks {
                cancel,
                done: Box::pin(async move {
                    rx.await
                        .unwrap_or_else(|_| Err("producer dropped without settling".into()))
                }),
            })
        }),
    };
    Producer {
        spec: Some(spec),
        settle,
        cancels,
        started,
    }
}

impl Producer {
    pub fn spec(&mut self) -> JobSpec {
        self.spec.take().expect("spec already taken")
    }
    pub fn settle(&self, outcome: JobOutcome) {
        if let Some(tx) = lock(&self.settle).take() {
            let _ = tx.send(Ok(outcome));
        }
    }
    pub fn cancels(&self) -> Vec<Option<String>> {
        lock(&self.cancels).clone()
    }
    fn handle(&self) -> JobHandle {
        lock(&self.started).clone().expect("producer not started")
    }
    pub fn append(&self, text: &str) {
        self.handle().append(text);
    }
    pub fn append_on(&self, text: &str, channel: hoh_jobs::JobChannel) {
        self.handle().append_with_options(
            text,
            hoh_jobs::JobAppendOptions {
                channel: Some(channel),
                gap_before: false,
            },
        );
    }
    pub fn progress(&self, line: &str) {
        self.handle().update_progress(line);
    }
}

/// Start a producer through the registry and return its id.
pub fn start(env: &Env, producer: &mut Producer) -> JobId {
    env.jobs().start(producer.spec()).expect("start")
}

pub fn outcome(status: JobOutcomeStatus) -> JobOutcome {
    JobOutcome {
        status,
        detail: None,
        result: None,
    }
}

pub fn completed() -> JobOutcome {
    outcome(JobOutcomeStatus::Completed)
}

pub fn completed_with(detail: Option<&str>, result: Option<&str>) -> JobOutcome {
    JobOutcome {
        status: JobOutcomeStatus::Completed,
        detail: detail.map(str::to_owned),
        result: result.map(str::to_owned),
    }
}

pub fn killed() -> JobOutcome {
    outcome(JobOutcomeStatus::Killed)
}

pub fn killed_with(detail: &str) -> JobOutcome {
    JobOutcome {
        status: JobOutcomeStatus::Killed,
        detail: Some(detail.to_owned()),
        result: None,
    }
}

/// `settleTasks(ctx, owner, count)`: start and settle `count` owned jobs one at a time, letting
/// each notice land.
pub async fn settle_tasks(env: &Env, owner: &Owner, count: usize) {
    for _ in 0..count {
        let mut p = owned_by(owner);
        start(env, &mut p);
        p.settle(completed());
        tick().await;
    }
}

/// The text of a delivered notice message.
pub fn notice_text(message: &UserMessage) -> String {
    message.content[0]
        .text_value()
        .unwrap_or_default()
        .to_owned()
}

/// `createUserMessage({ content: [{ type: 'text', text }], source: { kind: 'user' } })`.
pub fn user_message(text: &str) -> UserMessage {
    create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source: MessageSource::new("user"),
    })
}

/// A message of the `tool-jobs` notice form.
pub fn notice_message(text: &str, summary: &str) -> UserMessage {
    let mut source = MessageSource::new("tool-jobs");
    source.data.insert("form".into(), json!("notice"));
    source.data.insert("summary".into(), json!(summary));
    create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source,
    })
}
