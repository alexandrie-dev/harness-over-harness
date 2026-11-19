#![allow(dead_code)]

use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget, MaintenanceFuture,
    MaintenanceTask, Model, ModelSelection,
};
use hoh_commands::{CommandDefinition, CommandResult, CommandRuntime};
use hoh_error::BoxError;
use hoh_kernel::{Context, Kernel};
use hoh_llm::{MessageId, UserMessage};
use hoh_scope::ScopeKey;
use hoh_session::{AgentCancelCause, Session, SessionId};
use hoh_timeout::AbortController;
use std::sync::Arc;

struct UnusedModel;

impl Model for UnusedModel {
    fn current(&self) -> Option<ModelSelection> {
        None
    }

    fn select(&self, _: &ModelSelection) -> Result<(), BoxError> {
        Err("model fixture is not driven".into())
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
        Ok(task(AbortController::new().signal()))
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

pub struct Harness {
    pub _kernel: Kernel,
    pub ctx: Context,
    pub commands: CommandRuntime,
    pub agent: Arc<dyn Agent>,
}

pub fn harness() -> Harness {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let commands = CommandRuntime::new(ctx.clone()).unwrap();
    let agent = agent(&ctx, ScopeKey::new(), "main");
    Harness {
        _kernel: kernel,
        ctx,
        commands,
        agent,
    }
}

pub fn agent(ctx: &Context, scope: ScopeKey, id: &str) -> Arc<dyn Agent> {
    agent_with_session(
        ctx,
        scope,
        Session::create(SessionId::from(id), None, None, None, None).unwrap(),
    )
}

pub fn agent_with_session(ctx: &Context, scope: ScopeKey, session: Session) -> Arc<dyn Agent> {
    Arc::new(StubAgent {
        id: session.id().clone(),
        session,
        ctx: ctx.clone(),
        options: AgentOptions::default(),
        scope,
        model: UnusedModel,
        inbox: UnusedInbox,
    })
}

pub fn definition(name: &str) -> CommandDefinition {
    CommandDefinition::new(name, format!("{name} description"), |_| async {
        Ok(CommandResult::success(Some("ok")))
    })
}

pub fn event_data(agent: &Arc<dyn Agent>) -> Vec<(String, serde_json::Value)> {
    agent
        .session()
        .snapshot_events(None, None)
        .iter()
        .map(|event| (event.event_type().to_owned(), event.data().clone()))
        .collect()
}
