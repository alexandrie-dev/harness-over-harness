#![allow(dead_code)]

use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentOptions, AgentRegistryPlugin, AgentStatus, Agents, CancelOptions, Inbox,
    InboxTarget, MaintenanceFuture, MaintenanceTask, Model, ModelSelection,
};
use hoh_agent_preset_registry::{
    AgentPresetRegistry, Config, PresetDefinition, PresetRuntime,
    agent_preset_projection_definition,
};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Kernel, NoConfig, Plugin, Service};
use hoh_llm::{MessageId, UserMessage};
use hoh_loader::{EvalEnv, PluginRegistry};
use hoh_scope::{CreateScopeOptions, Scope, ScopeKey, create_scope};
use hoh_session::{AgentCancelCause, Session, SessionId};
use hoh_session_projection::{SessionProjectionRegistry, SessionProjections};
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Notify;

pub struct ActivePlugin;
impl Plugin for ActivePlugin {
    const NAME: &'static str = "ActivePlugin";
    type Config = NoConfig;
    type Deps = ();
    async fn apply(_: Context, _: (), _: Cfg<NoConfig>) -> Result<(), BoxError> {
        Ok(())
    }
}

pub struct FailingPlugin;
impl Plugin for FailingPlugin {
    const NAME: &'static str = "FailingPlugin";
    type Config = NoConfig;
    type Deps = ();
    async fn apply(_: Context, _: (), _: Cfg<NoConfig>) -> Result<(), BoxError> {
        Err("refused".into())
    }
}

hoh_kernel::service!(pub MissingService: () = "serviceThatDoesNotExist");
#[derive(Inject)]
pub struct MissingDeps {
    _missing: Service<MissingService>,
}
pub struct NeedsMissingPlugin;
impl Plugin for NeedsMissingPlugin {
    const NAME: &'static str = "NeedsMissingPlugin";
    type Config = NoConfig;
    type Deps = MissingDeps;
    async fn apply(_: Context, _: MissingDeps, _: Cfg<NoConfig>) -> Result<(), BoxError> {
        Ok(())
    }
}

hoh_kernel::service!(pub FixtureLeak: () = "fixtureService");
pub struct LeakPlugin;
impl Plugin for LeakPlugin {
    const NAME: &'static str = "LeakPlugin";
    type Config = NoConfig;
    type Deps = ();
    async fn apply(ctx: Context, _: (), _: Cfg<NoConfig>) -> Result<(), BoxError> {
        ctx.provide::<FixtureLeak>(Arc::new(())).await?;
        Ok(())
    }
}

hoh_kernel::service!(pub LateFixtureLeak: () = "lateFixtureService");
pub struct LateLeakPlugin;
impl Plugin for LateLeakPlugin {
    const NAME: &'static str = "LateLeakPlugin";
    type Config = NoConfig;
    type Deps = MissingDeps;
    async fn apply(ctx: Context, _: MissingDeps, _: Cfg<NoConfig>) -> Result<(), BoxError> {
        ctx.provide::<LateFixtureLeak>(Arc::new(())).await?;
        Ok(())
    }
}

hoh_kernel::service!(pub PresetAlpha: () = "presetAlpha");
hoh_kernel::service!(pub PresetZulu: () = "presetZulu");
pub struct MultiLeakPlugin;
impl Plugin for MultiLeakPlugin {
    const NAME: &'static str = "MultiLeakPlugin";
    type Config = NoConfig;
    type Deps = ();
    async fn apply(ctx: Context, _: (), _: Cfg<NoConfig>) -> Result<(), BoxError> {
        ctx.provide::<PresetZulu>(Arc::new(())).await?;
        ctx.provide::<PresetAlpha>(Arc::new(())).await?;
        Ok(())
    }
}

pub struct DelayGate {
    pub started: Notify,
    pub release: Notify,
}
hoh_kernel::service!(pub DelayGateService: DelayGate = "presetTestDelayGate");
#[derive(Inject)]
pub struct DelayedDeps {
    gate: Service<DelayGateService>,
}
pub struct DelayedPlugin;
impl Plugin for DelayedPlugin {
    const NAME: &'static str = "DelayedPlugin";
    type Config = NoConfig;
    type Deps = DelayedDeps;
    async fn apply(_: Context, deps: DelayedDeps, _: Cfg<NoConfig>) -> Result<(), BoxError> {
        deps.gate.started.notify_one();
        deps.gate.release.notified().await;
        Ok(())
    }
}

pub fn runtime() -> PresetRuntime {
    let mut registry = PluginRegistry::default();
    registry
        .add::<ActivePlugin>("fixture:active")
        .add::<FailingPlugin>("fixture:failing")
        .add::<NeedsMissingPlugin>("fixture:needs-missing")
        .add::<LeakPlugin>("fixture:leak")
        .add::<LateLeakPlugin>("fixture:late-leak")
        .add::<MultiLeakPlugin>("fixture:multi-leak")
        .add::<DelayedPlugin>("fixture:delayed");
    PresetRuntime::new(
        Arc::new(registry),
        EvalEnv {
            home: PathBuf::from("/fixture/home"),
            cwd: PathBuf::from("/fixture/work"),
            exe: PathBuf::from("/fixture/hoh"),
            platform: "linux",
            var: Arc::new(|_| None),
        },
    )
}

pub fn fresh() -> (Kernel, Context, AgentPresetRegistry) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let registry = AgentPresetRegistry::new(
        &ctx,
        Config {
            default: "standard".into(),
            selected_default: Default::default(),
        },
        runtime(),
    )
    .unwrap();
    (kernel, ctx, registry)
}

pub fn definition(id: &str, plugins: Value) -> PresetDefinition {
    PresetDefinition {
        id: id.into(),
        name: None,
        description: None,
        order: None,
        plugins,
    }
}

struct UnusedModel;
impl Model for UnusedModel {
    fn current(&self) -> Option<ModelSelection> {
        None
    }
    fn select(&self, _: &ModelSelection) -> Result<(), BoxError> {
        Ok(())
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

pub struct StubAgent {
    id: SessionId,
    session: Session,
    ctx: Context,
    scope: ScopeKey,
    options: AgentOptions,
    model: UnusedModel,
    inbox: UnusedInbox,
    panic_session: AtomicBool,
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
        if self.panic_session.swap(false, Ordering::SeqCst) {
            panic!("fixture session panic");
        }
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

pub struct SelectionHarness {
    pub kernel: Kernel,
    pub ctx: Context,
    pub registry: AgentPresetRegistry,
}

pub async fn selection_fresh() -> SelectionHarness {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionProjectionRegistry>(serde_json::json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.plugin::<AgentRegistryPlugin>(serde_json::json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let projections = ctx.get::<SessionProjections>().unwrap();
    let projection = projections
        .register(&ctx, agent_preset_projection_definition())
        .await
        .unwrap();
    let agents = ctx.get::<Agents>().unwrap();
    let registry = AgentPresetRegistry::new_with_services(
        &ctx,
        Config {
            default: "standard".into(),
            selected_default: Default::default(),
        },
        runtime(),
        agents,
        projection,
    )
    .unwrap();
    SelectionHarness {
        kernel,
        ctx,
        registry,
    }
}

pub fn agent(ctx: &Context, id: &str) -> (Scope, Arc<dyn Agent>) {
    let key = ScopeKey::new();
    let scope = create_scope(ctx, key.clone(), CreateScopeOptions::default()).unwrap();
    let session = Session::create(SessionId::from(id), None, None, None, None).unwrap();
    let agent: Arc<dyn Agent> = Arc::new(StubAgent {
        id: SessionId::from(id),
        session,
        ctx: scope.ctx.clone(),
        scope: key,
        options: AgentOptions::default(),
        model: UnusedModel,
        inbox: UnusedInbox,
        panic_session: AtomicBool::new(false),
    });
    (scope, agent)
}

pub fn panicking_agent(ctx: &Context, id: &str) -> (Scope, Arc<dyn Agent>) {
    let key = ScopeKey::new();
    let scope = create_scope(ctx, key.clone(), CreateScopeOptions::default()).unwrap();
    let session = Session::create(SessionId::from(id), None, None, None, None).unwrap();
    let agent: Arc<dyn Agent> = Arc::new(StubAgent {
        id: SessionId::from(id),
        session,
        ctx: scope.ctx.clone(),
        scope: key,
        options: AgentOptions::default(),
        model: UnusedModel,
        inbox: UnusedInbox,
        panic_session: AtomicBool::new(true),
    });
    (scope, agent)
}
