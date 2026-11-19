//! Compatibility layer for `hoh-sdk-jsonrpc-server`, until it migrates to the real Agent.
//!
//! DEPRECATED (comment, not the attribute: the consumer builds with `-D warnings`): for
//! hoh-sdk-jsonrpc-server until its migration. Nothing in this module belongs to the source package.
//! The names are those the crate had before it adopted `hoh_agent::Agent`; every [`Agent`] is
//! adapted to a real `hoh_agent::Agent` on entry to `ensure`/`set` (see [`AsLiveAgent`]).

use std::sync::Arc;

use futures_util::future::BoxFuture;
use hoh_agent::{
    AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget, MaintenanceFuture,
    MaintenanceTask, Model, ModelSelection,
};
use hoh_error::BoxError;
use hoh_kernel::Context;
use hoh_llm::{MessageId, UserMessage};
use hoh_scope::ScopeKey;
use hoh_session::{AgentCancelCause, Session, SessionId};

/// What the working directory asked of an Agent before it took the whole `hoh_agent::Agent`: the
/// Session the Agent owns, its plugin context and `inject`.
pub trait AgentApi: Send + Sync + 'static {
    /// `agent.session`.
    fn session(&self) -> Session;
    /// `agent.ctx`.
    fn ctx(&self) -> Context;
    /// `agent.inject(message)`: queue a notice for the next step.
    fn inject(&self, message: UserMessage) -> Result<(), BoxError>;
}

/// Shared handle on an [`AgentApi`]. Cloning shares the Agent; equality is identity.
#[derive(Clone)]
pub struct Agent(pub Arc<dyn AgentApi>);

impl Agent {
    /// Wrap an Agent implementation.
    pub fn new(agent: impl AgentApi) -> Self {
        Self(Arc::new(agent))
    }
    /// `agent.session`.
    pub fn session(&self) -> Session {
        self.0.session()
    }
    /// `agent.ctx`.
    pub fn ctx(&self) -> Context {
        self.0.ctx()
    }
    /// `agent.inject(message)`.
    pub fn inject(&self, message: UserMessage) -> Result<(), BoxError> {
        self.0.inject(message)
    }
}

impl PartialEq for Agent {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// An Agent value the working directory can run for: the real `Arc<dyn hoh_agent::Agent>` or the
/// compatibility [`Agent`].
pub trait AsLiveAgent {
    /// The real Agent behind this value.
    fn live(&self) -> Arc<dyn hoh_agent::Agent>;
}

impl AsLiveAgent for Arc<dyn hoh_agent::Agent> {
    fn live(&self) -> Arc<dyn hoh_agent::Agent> {
        self.clone()
    }
}

impl AsLiveAgent for Agent {
    fn live(&self) -> Arc<dyn hoh_agent::Agent> {
        let session = self.session();
        Arc::new(Adapter {
            id: session.id().clone(),
            session,
            ctx: self.ctx(),
            scope: ScopeKey::new(),
            options: AgentOptions::default(),
            api: self.0.clone(),
        })
    }
}

/// A `hoh_agent::Agent` over an [`AgentApi`]: Session, context and `inject` are real; the driver,
/// model and inbox are absent (the working directory touches none of them).
struct Adapter {
    id: SessionId,
    session: Session,
    ctx: Context,
    scope: ScopeKey,
    options: AgentOptions,
    api: Arc<dyn AgentApi>,
}

struct NoModel;
impl Model for NoModel {
    fn current(&self) -> Option<ModelSelection> {
        None
    }
    fn select(&self, _: &ModelSelection) -> Result<(), BoxError> {
        Err("the compatibility agent has no model".into())
    }
}

struct NoInbox;
impl NoInbox {
    fn refuse<T>() -> Result<T, BoxError> {
        Err("the compatibility agent has no inbox".into())
    }
}
impl Inbox for NoInbox {
    fn next_turn(&self) -> Vec<UserMessage> {
        Vec::new()
    }
    fn next_step(&self) -> Vec<UserMessage> {
        Vec::new()
    }
    fn clear(&self) -> Result<(), BoxError> {
        Self::refuse()
    }
    fn append(&self, _: InboxTarget, _: UserMessage) -> Result<(), BoxError> {
        Self::refuse()
    }
    fn prepend(&self, _: InboxTarget, _: UserMessage) -> Result<(), BoxError> {
        Self::refuse()
    }
    fn replace(&self, _: &MessageId, _: UserMessage) -> Result<bool, BoxError> {
        Self::refuse()
    }
    fn remove(&self, _: &MessageId) -> Result<bool, BoxError> {
        Self::refuse()
    }
    fn splice(
        &self,
        _: InboxTarget,
        _: f64,
        _: f64,
        _: Vec<UserMessage>,
    ) -> Result<Vec<UserMessage>, BoxError> {
        Self::refuse()
    }
}

impl hoh_agent::Agent for Adapter {
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
        &NoModel
    }
    fn inbox(&self) -> &dyn Inbox {
        &NoInbox
    }
    fn status(&self) -> AgentStatus {
        AgentStatus::Idle
    }
    fn ctx(&self) -> &Context {
        &self.ctx
    }
    fn cancel(&self, _: AgentCancelCause, _: CancelOptions) -> Result<(), BoxError> {
        NoInbox::refuse()
    }
    fn when_idle(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance_erased(&self, _: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        NoInbox::refuse()
    }
    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
        NoInbox::refuse()
    }
    fn followup(&self, _: UserMessage) -> Result<(), BoxError> {
        NoInbox::refuse()
    }
    fn steer(&self, _: UserMessage) -> Result<(), BoxError> {
        NoInbox::refuse()
    }
    fn inject(&self, message: UserMessage) -> Result<(), BoxError> {
        self.api.inject(message)
    }
}

/// The `sessionProjections` stand-in of the former stand-in layer. The working directory no longer
/// uses it (it registers with the real `hoh-session-projection`); kept so a composition that still
/// provides it keeps compiling. Its key has its own name: under `"sessionProjections"` it would
/// replace the real registry and the working directory could not find the real service.
#[derive(Default)]
pub struct SessionProjections;

hoh_kernel::service!(pub SessionProjectionsKey: SessionProjections = "sessionProjectionsCompat");
