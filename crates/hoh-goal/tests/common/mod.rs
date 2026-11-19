#![allow(dead_code)]

use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentOptions, AgentRegistryPlugin, AgentStatus, Agents, CancelOptions, Inbox,
    InboxTarget, MaintenanceFuture, MaintenanceTask, Model,
};
use hoh_agent_loop_testkit::{unsupported_inbox, unsupported_model};
use hoh_error::{BoxError, HarnessError};
use hoh_goal::{GoalServicePlugin, Goals};
use hoh_kernel::{Context, Kernel, Service};
use hoh_llm::UserMessage;
use hoh_scope::ScopeKey;
use hoh_session::{
    AgentCancelCause, CreateSessionOptions, Session, SessionId, SessionPlugin, Sessions,
};
use hoh_session_projection::{SessionProjectionRegistry, SessionProjections};
use serde_json::json;
use std::sync::Arc;

struct StubAgent {
    id: SessionId,
    session: Session,
    ctx: Context,
    options: AgentOptions,
    scope: ScopeKey,
    model: Arc<dyn Model>,
    inbox: Arc<dyn Inbox>,
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

pub fn agent(ctx: &Context, session: Session) -> Arc<dyn Agent> {
    Arc::new(StubAgent {
        id: session.id().clone(),
        session,
        ctx: ctx.clone(),
        options: AgentOptions::default(),
        scope: ScopeKey::new(),
        model: unsupported_model(),
        inbox: unsupported_inbox(),
    })
}

pub struct Harness {
    pub _kernel: Kernel,
    pub ctx: Context,
    pub goals: Service<Goals>,
    pub projections: Service<SessionProjections>,
    pub sessions: Service<Sessions>,
    pub agents: Service<Agents>,
    pub agent: Arc<dyn Agent>,
    pub session: Session,
}

pub async fn harness(config: serde_json::Value) -> Harness {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SessionPlugin>(json!(null))
        .unwrap()
        .settled()
        .await
        .unwrap();
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
    ctx.plugin::<GoalServicePlugin>(config)
        .unwrap()
        .settled()
        .await
        .unwrap();
    let projections = ctx.get::<SessionProjections>().unwrap();
    let agents = ctx.get::<Agents>().unwrap();
    let goals = ctx.get::<Goals>().unwrap();
    let sessions = ctx.get::<Sessions>().unwrap();
    let session = sessions
        .create(
            &ctx,
            Some(SessionId::from("goal-test")),
            CreateSessionOptions::default(),
        )
        .unwrap();
    let agent = agent(&ctx, session.clone());
    agents.register(&ctx, agent.clone()).await.unwrap();
    Harness {
        _kernel: kernel,
        ctx,
        goals,
        projections,
        sessions,
        agents,
        agent,
        session,
    }
}

pub fn code(error: &BoxError) -> &str {
    HarnessError::of(error.as_ref()).unwrap().code()
}
