#![allow(dead_code)]

use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentOptions, AgentRegistryPlugin, AgentStatus, CancelOptions, Inbox, InboxTarget,
    MaintenanceFuture, MaintenanceTask, Model, ModelSelection,
};
use hoh_error::BoxError;
use hoh_kernel::{Context, Kernel};
use hoh_llm::{MessageId, UserMessage};
use hoh_scope::ScopeKey;
use hoh_session::{AgentCancelCause, Session, SessionId};
use hoh_session_projection::SessionProjectionRegistry;
use serde_json::json;
use std::sync::Arc;

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

pub fn stub_agent(ctx: &Context, id: &str) -> Arc<dyn Agent> {
    stub_agent_with_session(ctx, id, id)
}

pub fn stub_agent_with_session(ctx: &Context, id: &str, session_id: &str) -> Arc<dyn Agent> {
    let session = Session::create(SessionId::from(session_id), None, None, None, None).unwrap();
    stub_agent_on(ctx, id, session)
}

/// A stub agent that wraps an existing session.
pub fn stub_agent_on(ctx: &Context, id: &str, session: Session) -> Arc<dyn Agent> {
    Arc::new(StubAgent {
        id: SessionId::from(id),
        session,
        ctx: ctx.clone(),
        options: AgentOptions::default(),
        scope: ScopeKey::new(),
        model: UnusedModel,
        inbox: UnusedInbox,
    })
}

pub struct Harness {
    pub kernel: Kernel,
    pub ctx: Context,
}

/// A kernel with the projection registry and the agent registry mounted.
pub async fn mount() -> Harness {
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
    Harness { kernel, ctx }
}
