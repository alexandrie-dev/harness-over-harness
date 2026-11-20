//! Shared test stand: a kernel with the agent registry and the local job registry mounted, stub
//! agents that are only registered and observed, and a controllable producer.
//! Port of the helpers at the head of `tests/jobs.spec.ts`.
#![allow(dead_code)]

use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentOptions, AgentRegistryPlugin, AgentStatus, Agents, CancelOptions, Inbox,
    InboxTarget, MaintenanceFuture, MaintenanceTask, Model, ModelSelection,
};
use hoh_error::BoxError;
use hoh_jobs::{
    JobEvent, JobEventFilter, JobEventOwners, JobHandle, JobHooks, JobId, JobOutcome,
    JobOutcomeStatus, JobOutputSource, JobSpec, Jobs,
};
use hoh_jobs_local::LocalJobRegistryPlugin;
use hoh_kernel::{Context, Fiber, Kernel, Level, Levels, Service};
use hoh_llm::{MessageId, UserMessage};
use hoh_scope::{CreateScopeOptions, ScopeKey, create_scope};
use hoh_session::{AgentCancelCause, Session, SessionId};
use hoh_session_projection::SessionProjectionRegistry;
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};
use tokio::sync::oneshot;

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct UnusedModel;
impl Model for UnusedModel {
    fn current(&self) -> Option<ModelSelection> {
        None
    }
    fn select(&self, _: &ModelSelection) -> Result<(), BoxError> {
        Err("model fixture does not drive the Agent".into())
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

/// An `Agent` that is only registered and observed, never driven.
pub struct StubAgent {
    id: SessionId,
    session: Session,
    ctx: Context,
    options: AgentOptions,
    scope: ScopeKey,
    model: UnusedModel,
    inbox: UnusedInbox,
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
    fn when_idle(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance_erased(&self, task: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        Ok(task(hoh_timeout::AbortController::new().signal()))
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

/// A stub agent plus the fiber that carries its context (`agentScopeDisposers`).
#[derive(Clone)]
pub struct TestAgent {
    pub agent: Arc<dyn Agent>,
    pub fiber: Fiber,
}

impl TestAgent {
    pub fn id(&self) -> SessionId {
        self.agent.id().clone()
    }
    pub fn session(&self) -> Session {
        self.agent.session().clone()
    }
    /// `disposeAgentScope`: dispose the context fiber the agent's jobs are tied to.
    pub async fn dispose_scope(&self) {
        self.fiber.dispose().await;
    }
}

/// `stubAgent`: a fresh child context; `preset_scope` reproduces what `agentPresets.compose`
/// does — the agent gets its own key parented to the standing mount's.
pub fn stub_agent(ctx: &Context, raw_id: &str, preset_scope: Option<ScopeKey>) -> TestAgent {
    let session = Session::create(SessionId::from(raw_id), None, None, None, None).unwrap();
    stub_agent_on(ctx, raw_id, preset_scope, session)
}

/// `stub_agent` over a caller-supplied session.
pub fn stub_agent_on(
    ctx: &Context,
    raw_id: &str,
    preset_scope: Option<ScopeKey>,
    session: Session,
) -> TestAgent {
    let id = SessionId::from(raw_id);
    let scope_ctx = ctx.child("agent").unwrap();
    let fiber = scope_ctx.fiber();
    let mut agent_ctx = scope_ctx;
    let mut scope = ScopeKey::new();
    if let Some(preset) = preset_scope {
        let key = ScopeKey::new();
        let created = create_scope(
            &agent_ctx,
            key.clone(),
            CreateScopeOptions {
                parent: Some(preset),
            },
        )
        .unwrap();
        agent_ctx = created.ctx.clone();
        scope = key;
    }
    TestAgent {
        agent: Arc::new(StubAgent {
            id,
            session,
            ctx: agent_ctx,
            options: AgentOptions::default(),
            scope,
            model: UnusedModel,
            inbox: UnusedInbox,
        }),
        fiber,
    }
}

/// `liveAgent`: register a stub agent so the registry can resolve it as a live owner.
pub async fn live_agent(ctx: &Context, raw_id: &str, preset_scope: Option<ScopeKey>) -> TestAgent {
    let agent = stub_agent(ctx, raw_id, preset_scope);
    let agents = ctx.get::<Agents>().expect("agent registry");
    agents.register(ctx, agent.agent.clone()).await.unwrap();
    agent
}

/// `agents.register(agent)`: the disposer detaches the agent again.
pub async fn register_agent(ctx: &Context, agent: &TestAgent) -> hoh_kernel::Disposer {
    let agents = ctx.get::<Agents>().expect("agent registry");
    agents.register(ctx, agent.agent.clone()).await.unwrap()
}

/// The kernel, its root context, and the warnings the logger exported.
pub struct World {
    pub kernel: Kernel,
    pub ctx: Context,
    pub warnings: Arc<Mutex<Vec<String>>>,
    /// The `LocalJobRegistry` plugin's fiber, when the plugin is mounted.
    pub jobs_fiber: Option<Fiber>,
}

impl World {
    pub fn jobs(&self) -> Service<Jobs> {
        self.ctx.get::<Jobs>().expect("jobs service")
    }
    pub fn warnings(&self) -> Vec<String> {
        lock(&self.warnings).clone()
    }
}

fn warn_levels() -> Levels {
    Levels {
        default: Some(Level::Warn),
        ..Levels::default()
    }
}

/// A kernel that only captures warnings (no providers mounted).
pub fn bare() -> World {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let sink = warnings.clone();
    ctx.logger()
        .exporter(warn_levels(), move |record| {
            if record.level == Level::Warn {
                lock(&sink).push(record.message.clone());
            }
        })
        .unwrap();
    World {
        kernel,
        ctx,
        warnings,
        jobs_fiber: None,
    }
}

/// `harness`: projection registry, agent registry and the local job registry, with a
/// `test-controller` attached.
pub async fn harness(config: Value) -> World {
    let world = harness_without_controller(config).await;
    let ctx = world.ctx.clone();
    world
        .jobs()
        .attach_controller(&ctx, "test-controller")
        .unwrap();
    world
}

/// Only the local job registry: no agent registry, no controller.
pub async fn jobs_only() -> World {
    let mut world = bare();
    let fiber = world
        .ctx
        .plugin::<LocalJobRegistryPlugin>(json!({}))
        .unwrap();
    fiber.settled().await.unwrap();
    world.jobs_fiber = Some(fiber);
    world
}

pub async fn harness_without_controller(config: Value) -> World {
    let mut world = bare();
    let ctx = world.ctx.clone();
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
    let fiber = ctx.plugin::<LocalJobRegistryPlugin>(config).unwrap();
    fiber.settled().await.unwrap();
    world.jobs_fiber = Some(fiber);
    world
}

type Settle = Arc<Mutex<Option<oneshot::Sender<Result<JobOutcome, BoxError>>>>>;

/// A controllable producer spec: settle its `done` on demand, record cancels, expose the handle.
pub struct Producer {
    pub spec: Option<JobSpec>,
    settle: Settle,
    pub cancels: Arc<Mutex<Vec<Option<String>>>>,
    started: Arc<Mutex<Option<JobHandle>>>,
}

type CancelHook = Box<dyn Fn(Option<&str>) -> Result<(), BoxError> + Send + Sync>;

#[derive(Default)]
pub struct ProducerOptions {
    pub kind: Option<&'static str>,
    pub label: Option<&'static str>,
    pub owner: Option<SessionId>,
    pub output_limit_bytes: Option<u64>,
    pub output: Option<Vec<JobOutputSource>>,
    /// Replaces the recording cancel hook.
    pub cancel: Option<CancelHook>,
    /// The recording cancel hook also settles the job with this outcome (a child that stops on
    /// request).
    pub settle_on_cancel: Option<JobOutcome>,
    /// After settling, the cancelling thread blocks this long, so another worker can run the
    /// settlement before `kill` returns (a producer that stops at once).
    pub cancel_stall: Option<std::time::Duration>,
}

pub fn producer() -> Producer {
    producer_with(ProducerOptions::default())
}

pub fn owned_by(owner: &TestAgent) -> Producer {
    producer_with(ProducerOptions {
        owner: Some(owner.id()),
        ..ProducerOptions::default()
    })
}

pub fn producer_with(options: ProducerOptions) -> Producer {
    let (tx, rx) = oneshot::channel::<Result<JobOutcome, BoxError>>();
    let settle = Arc::new(Mutex::new(Some(tx)));
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
        label: options.label.unwrap_or("sleep 60").to_owned(),
        owner: options.owner,
        tool_call: None,
        output_limit_bytes: options.output_limit_bytes,
        output: options.output,
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
        self.finish(Ok(outcome));
    }
    pub fn reject(&self, error: impl Into<BoxError>) {
        self.finish(Err(error.into()));
    }
    fn finish(&self, result: Result<JobOutcome, BoxError>) {
        if let Some(tx) = lock(&self.settle).take() {
            let _ = tx.send(result);
        }
    }
    pub fn job(&self) -> JobHandle {
        lock(&self.started).clone().expect("producer not started")
    }
    pub fn cancels(&self) -> Vec<Option<String>> {
        lock(&self.cancels).clone()
    }
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

pub fn killed() -> JobOutcome {
    outcome(JobOutcomeStatus::Killed)
}

pub fn failed(detail: &str) -> JobOutcome {
    JobOutcome {
        detail: Some(detail.to_owned()),
        ..outcome(JobOutcomeStatus::Failed)
    }
}

pub fn all_owners() -> JobEventFilter {
    JobEventFilter::Owners {
        owners: JobEventOwners::All,
    }
}

pub fn scope_owners() -> JobEventFilter {
    JobEventFilter::Owners {
        owners: JobEventOwners::Scope,
    }
}

pub fn owner_filter(owner: &SessionId) -> JobEventFilter {
    JobEventFilter::Owner {
        owner: owner.clone(),
    }
}

pub type Events = Arc<Mutex<Vec<JobEvent>>>;

/// `collect`: events matching `filter`; `types` narrows what is recorded.
pub fn collect(
    world: &World,
    filter: JobEventFilter,
    types: Option<&'static [&'static str]>,
) -> Events {
    let seen: Events = Arc::default();
    let sink = seen.clone();
    world
        .jobs()
        .events()
        .subscribe(
            &world.ctx,
            filter,
            Arc::new(move |event| {
                if types.is_none_or(|types| types.contains(&event_type(event))) {
                    lock(&sink).push(event.clone());
                }
                Ok(())
            }),
        )
        .unwrap();
    seen
}

pub fn event_type(event: &JobEvent) -> &'static str {
    match event {
        JobEvent::Registered { .. } => "registered",
        JobEvent::Progress { .. } => "progress",
        JobEvent::Stopping { .. } => "stopping",
        JobEvent::Removed { .. } => "removed",
        JobEvent::Settled { .. } => "settled",
        JobEvent::Output { .. } => "output",
    }
}

/// `attachControllerIn`: attach a job controller from a plugin-like context so it files into that
/// context's scope layer.
pub fn attach_controller_in(world: &World, ctx: &Context) {
    world.jobs().attach_controller(ctx, "tool-jobs").unwrap();
}

/// `tick`: let the settlement continuation (a `done.then`) run.
pub async fn tick() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_millis(2)).await;
}

pub fn job_id(raw: &str) -> JobId {
    JobId::from(raw)
}

/// The kinds of the events recorded so far.
pub fn types(events: &Events) -> Vec<&'static str> {
    lock(events).iter().map(event_type).collect()
}

/// The structured identity a registry refusal carries: `(name, code)`.
pub fn error_identity(error: &BoxError) -> (String, String) {
    let harness = error
        .downcast_ref::<hoh_error::HarnessError>()
        .unwrap_or_else(|| panic!("not a HarnessError: {error}"));
    (harness.name().to_owned(), harness.code().to_owned())
}

pub fn assert_job_error(error: &BoxError, code: &str) {
    assert_eq!(
        error_identity(error),
        ("JobError".to_owned(), code.to_owned())
    );
}

/// `start` that must fail; returns the error.
pub fn start_err(world: &World, spec: JobSpec) -> BoxError {
    world.jobs().start(spec).expect_err("start must refuse")
}

pub fn start_ok(world: &World, spec: JobSpec) -> JobId {
    world.jobs().start(spec).expect("start must succeed")
}

/// Count how often the producer's `run` is invoked (`vi.fn(run)`).
pub fn counted(spec: JobSpec) -> (JobSpec, Arc<std::sync::atomic::AtomicUsize>) {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = calls.clone();
    let run = spec.run;
    (
        JobSpec {
            run: Box::new(move |job| {
                seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                run(job)
            }),
            ..spec
        },
        calls,
    )
}

/// `^<kind>-[0-9a-f]{16}$`
pub fn is_job_id(kind: &str, id: &JobId) -> bool {
    let text = id.to_string();
    text.strip_prefix(&format!("{kind}-")).is_some_and(|tail| {
        tail.len() == 16 && tail.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
    })
}

/// A registry built directly (not through the plugin) so a test can reach the hidden accessors;
/// it has a controller attached.
pub fn direct_registry(world: &World) -> hoh_jobs_local::LocalJobRegistry {
    use hoh_jobs::JobRegistryApi;
    let registry =
        hoh_jobs_local::LocalJobRegistry::new(world.ctx.clone(), hoh_jobs_local::Config::default())
            .unwrap();
    registry
        .attach_controller(&world.ctx, "test-controller")
        .unwrap();
    registry
}

/// A pull source scripted from per-read texts (each read shifts the next one).
pub fn scripted_source(
    reads: Vec<&'static str>,
    channel: Option<hoh_jobs::JobChannel>,
) -> JobOutputSource {
    let queue = Mutex::new(std::collections::VecDeque::from(reads));
    let offset = Mutex::new(0u64);
    JobOutputSource {
        channel,
        read: Box::new(move |_| {
            let text = lock(&queue).pop_front().unwrap_or("");
            let mut offset = lock(&offset);
            *offset += text.len() as u64;
            Ok(hoh_jobs::JobSourceRead {
                text: text.to_owned(),
                next_offset: *offset,
                lossy: false,
                spill_path: None,
            })
        }),
    }
}

/// Sleep on the real clock long enough for the pump's 1-5 ms cadence to run.
pub async fn real_sleep(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

/// The first session event of `kind`.
pub fn session_event(session: &Session, kind: &str) -> Option<Arc<hoh_session::SessionEvent>> {
    session
        .snapshot_events(None, None)
        .iter()
        .find(|event| event.event_type() == kind)
        .cloned()
}

pub fn session_event_types(session: &Session) -> Vec<String> {
    session
        .snapshot_events(None, None)
        .iter()
        .map(|event| event.event_type().to_owned())
        .collect()
}
