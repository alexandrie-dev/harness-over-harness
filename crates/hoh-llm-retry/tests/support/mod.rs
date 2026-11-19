use hoh_agent::{
    Agent, AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget, MaintenanceFuture,
    MaintenanceTask, Model, ModelSelection,
};
use hoh_error::BoxError;
use hoh_kernel::{Context, Kernel};
use hoh_llm::{MessageId, UserMessage};
use hoh_scope::ScopeKey;
use hoh_session::{
    AgentCancelCause, CreateSessionOptions, Session, SessionId, SessionStore, Sessions,
};
use hoh_session_projection::{SessionProjectionRegistry, SessionProjections};
use std::{future::Future, pin::Pin, sync::Arc};

struct TestModel;

impl Model for TestModel {
    fn current(&self) -> Option<ModelSelection> {
        None
    }

    fn select(&self, _selection: &ModelSelection) -> Result<(), BoxError> {
        Ok(())
    }
}

struct TestInbox;

impl Inbox for TestInbox {
    fn next_turn(&self) -> Vec<UserMessage> {
        Vec::new()
    }

    fn next_step(&self) -> Vec<UserMessage> {
        Vec::new()
    }

    fn clear(&self) -> Result<(), BoxError> {
        Ok(())
    }

    fn append(&self, _target: InboxTarget, _message: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }

    fn prepend(&self, _target: InboxTarget, _message: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }

    fn replace(
        &self,
        _message_id: &MessageId,
        _new_message: UserMessage,
    ) -> Result<bool, BoxError> {
        Ok(false)
    }

    fn remove(&self, _message_id: &MessageId) -> Result<bool, BoxError> {
        Ok(false)
    }

    fn splice(
        &self,
        _target: InboxTarget,
        _start: f64,
        _delete_count: f64,
        _inserted: Vec<UserMessage>,
    ) -> Result<Vec<UserMessage>, BoxError> {
        Ok(Vec::new())
    }
}

pub struct TestAgent {
    ctx: Context,
    session: Session,
    scope: ScopeKey,
    options: AgentOptions,
    model: TestModel,
    inbox: TestInbox,
}

impl TestAgent {
    fn new(ctx: Context, session: Session) -> Self {
        Self {
            ctx,
            session,
            scope: ScopeKey::new(),
            options: AgentOptions::default(),
            model: TestModel,
            inbox: TestInbox,
        }
    }
}

impl Agent for TestAgent {
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

    fn cancel(&self, _cause: AgentCancelCause, _options: CancelOptions) -> Result<(), BoxError> {
        Ok(())
    }

    fn when_idle(&self) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        Box::pin(async {})
    }

    fn run_maintenance_erased(
        &self,
        _task: MaintenanceTask,
    ) -> Result<MaintenanceFuture, BoxError> {
        Err("test agent does not run maintenance".into())
    }

    fn send(
        &self,
        _message: UserMessage,
        _target: InboxTarget,
        _wakeup: bool,
    ) -> Result<(), BoxError> {
        Ok(())
    }

    fn followup(&self, _message: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }

    fn steer(&self, _message: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }

    fn inject(&self, _message: UserMessage) -> Result<(), BoxError> {
        Ok(())
    }
}

pub struct Env {
    pub kernel: Kernel,
    pub ctx: Context,
    #[allow(dead_code)]
    pub registry: SessionProjectionRegistry,
    pub agent: Arc<dyn Agent>,
}

impl Env {
    pub async fn new(id: &str) -> Self {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        let sessions = Arc::new(SessionStore::new());
        ctx.provide::<Sessions>(sessions.clone()).await.unwrap();
        let registry = SessionProjectionRegistry::new(&ctx).unwrap();
        ctx.provide::<SessionProjections>(Arc::new(registry.clone()))
            .await
            .unwrap();
        let session = sessions
            .create(&ctx, Some(id.into()), CreateSessionOptions::default())
            .unwrap();
        let agent = Arc::new(TestAgent::new(ctx.clone(), session));
        Self {
            kernel,
            ctx,
            registry,
            agent,
        }
    }

    #[allow(dead_code)]
    pub fn event_types(&self) -> Vec<String> {
        self.agent
            .session()
            .snapshot_events(None, None)
            .iter()
            .map(|event| event.event_type().to_owned())
            .collect()
    }

    pub async fn close(self) {
        self.kernel.shutdown().await;
    }
}
