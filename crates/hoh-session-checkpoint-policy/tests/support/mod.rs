#![allow(dead_code)]

use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget, MaintenanceFuture,
    MaintenanceTask, Model, ModelSelection,
};
use hoh_error::BoxError;
use hoh_kernel::{Context, Kernel};
use hoh_llm::{MessageId, UserMessage};
use hoh_scope::ScopeKey;
use hoh_session::{AgentCancelCause, CreateSessionOptions, Session, SessionId, Sessions};
use hoh_session_persistence::{
    SessionAccess, SessionHandle, SessionPersistence, SessionPersistenceCreateOptions,
    SessionPersistenceIdentity, SessionPersistenceListOptions, SessionPersistenceOpenOptions,
    SessionPersistenceService, SessionPersistenceSnapshot, SessionPersistenceStatOptions,
};
use hoh_timeout::{AbortController, AbortSignal};
use std::sync::Arc;

pub struct NullPersistence;

impl SessionPersistence for NullPersistence {
    fn identity(&self) -> SessionPersistenceIdentity {
        SessionPersistenceIdentity::new()
    }

    fn create<'a>(
        &'a self,
        _: &'a hoh_session::SessionHeader,
        _: Option<&'a SessionPersistenceCreateOptions>,
    ) -> BoxFuture<'a, Result<Arc<dyn SessionHandle>, BoxError>> {
        Box::pin(async { Err("not used".into()) })
    }

    fn open<'a>(
        &'a self,
        _: &'a SessionId,
        _: SessionAccess,
        _: Option<&'a SessionPersistenceOpenOptions>,
    ) -> BoxFuture<'a, Result<Arc<dyn SessionHandle>, BoxError>> {
        Box::pin(async { Err("not used".into()) })
    }

    fn flush(&self) -> BoxFuture<'_, Result<(), BoxError>> {
        Box::pin(async { Ok(()) })
    }

    fn stat<'a>(
        &'a self,
        _: &'a SessionId,
        _: Option<&'a SessionPersistenceStatOptions>,
    ) -> BoxFuture<'a, Result<Option<SessionPersistenceSnapshot>, BoxError>> {
        Box::pin(async { Ok(None) })
    }

    fn list<'a>(
        &'a self,
        _: Option<&'a SessionPersistenceListOptions>,
    ) -> BoxFuture<'a, Result<Vec<SessionPersistenceSnapshot>, BoxError>> {
        Box::pin(async { Ok(vec![]) })
    }
}

pub struct Harness {
    pub kernel: Kernel,
    pub ctx: Context,
    pub sessions: hoh_kernel::Service<Sessions>,
    pub llm: hoh_kernel::Service<hoh_llm::Llm>,
    pub tools: hoh_kernel::Service<hoh_tools::Tools>,
    pub policy: hoh_kernel::Fiber,
}

pub async fn setup() -> Harness {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    for fiber in [
        ctx.plugin::<hoh_session::SessionPlugin>(serde_json::json!({}))
            .unwrap(),
        ctx.plugin::<hoh_llm::LlmPlugin>(serde_json::json!({}))
            .unwrap(),
        ctx.plugin::<hoh_system_prompt::SystemPrompt>(serde_json::json!({}))
            .unwrap(),
    ] {
        fiber.settled().await.unwrap();
    }
    ctx.plugin::<hoh_tools::ToolRuntime>(serde_json::json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.provide::<SessionPersistenceService>(
        Arc::new(NullPersistence) as Arc<dyn SessionPersistence>
    )
    .await
    .unwrap();
    let policy = ctx
        .plugin::<hoh_session_checkpoint_policy::SessionCheckpointPolicy>(serde_json::json!({}))
        .unwrap();
    policy.settled().await.unwrap();
    Harness {
        sessions: ctx.get().unwrap(),
        llm: ctx.get().unwrap(),
        tools: ctx.get().unwrap(),
        kernel,
        ctx,
        policy,
    }
}

impl Harness {
    pub fn session(&self, id: &str) -> Session {
        self.sessions
            .create(
                &self.ctx,
                Some(SessionId::from(id)),
                CreateSessionOptions::default(),
            )
            .unwrap()
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
}

impl StubAgent {
    pub fn new(ctx: &Context, session: Session) -> Arc<Self> {
        Arc::new(Self {
            id: session.id().clone(),
            session,
            ctx: ctx.clone(),
            scope: ScopeKey::new(),
            options: AgentOptions::default(),
            model: UnusedModel,
            inbox: UnusedInbox,
        })
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

impl hoh_tools::Agent for StubAgent {
    fn scope_key(&self) -> ScopeKey {
        self.scope.clone()
    }
    fn session(&self) -> Session {
        self.session.clone()
    }
}

pub fn live_signal() -> AbortSignal {
    AbortController::new().signal()
}

pub async fn execute_tool(
    ctx: &Context,
    input: hoh_tools::ToolExecutionInput,
) -> hoh_tools::ToolExecutionResult {
    let pipeline = hoh_tools::ToolExecutionPipeline::new(ctx.clone()).unwrap();
    let prepared = pipeline.prepare(input).await;
    let exec = prepared.exec().clone();
    let outcome = match prepared {
        hoh_tools::ScheduledToolPreparation::Dispatch(exec) => pipeline.dispatch(exec).await,
        hoh_tools::ScheduledToolPreparation::PostResult { result, .. } => {
            hoh_tools::ScheduledToolDispatch::PostResult(result)
        }
        hoh_tools::ScheduledToolPreparation::FinalResult { result, .. } => {
            hoh_tools::ScheduledToolDispatch::FinalResult(result)
        }
    };
    pipeline.complete(&exec, outcome).await
}
