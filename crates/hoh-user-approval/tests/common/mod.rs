#![allow(dead_code)]
use crate::*;
use hoh_agent::{
    AgentOptions, AgentStatus, CancelOptions, Inbox, InboxTarget, MaintenanceFuture,
    MaintenanceTask, Model,
};
use hoh_kernel::{Fiber, Kernel};
use hoh_llm::UserMessage;
use hoh_scope::ScopeKey;
use hoh_session::{AgentCancelCause, SessionEvent, SessionPlugin, Sessions, SurfaceIntent};
use hoh_session_projection::{SessionProjectionRegistry, SessionProjections};
use hoh_system_prompt::AssembleContext;
use serde_json::json;
use std::sync::{Mutex, MutexGuard};

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

pub struct FakeAgent {
    session: Session,
    ctx: Context,
    key: ScopeKey,
    options: AgentOptions,
    pub injected: Mutex<Vec<UserMessage>>,
}
impl Agent for FakeAgent {
    fn id(&self) -> &hoh_session::SessionId {
        self.session.id()
    }
    fn session(&self) -> &Session {
        &self.session
    }
    fn scope_key(&self) -> ScopeKey {
        self.key.clone()
    }
    fn options(&self) -> &AgentOptions {
        &self.options
    }
    fn ctx(&self) -> &Context {
        &self.ctx
    }
    fn status(&self) -> AgentStatus {
        AgentStatus::Idle
    }
    fn model(&self) -> &dyn Model {
        panic!("structural source fixture has no model")
    }
    fn inbox(&self) -> &dyn Inbox {
        panic!("structural source fixture has no inbox")
    }
    fn cancel(&self, _: AgentCancelCause, _: CancelOptions) -> Result<(), BoxError> {
        panic!("not used by approval")
    }
    fn when_idle(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance_erased(&self, _: MaintenanceTask) -> Result<MaintenanceFuture, BoxError> {
        panic!("not used by approval")
    }
    fn send(&self, _: UserMessage, _: InboxTarget, _: bool) -> Result<(), BoxError> {
        panic!("not used by approval")
    }
    fn followup(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("not used by approval")
    }
    fn steer(&self, _: UserMessage) -> Result<(), BoxError> {
        panic!("not used by approval")
    }
    fn inject(&self, message: UserMessage) -> Result<(), BoxError> {
        lock(&self.injected).push(message);
        Ok(())
    }
}
pub struct Harness {
    pub kernel: Kernel,
    pub ctx: Context,
    pub registry_owner: Option<Fiber>,
}
impl Harness {
    pub async fn base() -> Self {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<SessionPlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        Self {
            kernel,
            ctx,
            registry_owner: None,
        }
    }
    pub async fn context() -> Self {
        let mut h = Self::base().await;
        h.registry_owner = Some(h.load::<SessionProjectionRegistry>(json!({})).await);
        h
    }
    pub async fn mounted() -> Self {
        let h = Self::context().await;
        h.load::<ApprovalService>(json!({})).await;
        h
    }
    pub async fn load<P: Plugin>(&self, config: Value) -> Fiber {
        let f = self.ctx.plugin::<P>(config).unwrap();
        f.settled().await.unwrap();
        f
    }
    pub fn service(&self) -> Service<Approval> {
        self.ctx.get::<Approval>().unwrap()
    }
    pub fn registry(&self) -> Service<SessionProjections> {
        self.ctx.get::<SessionProjections>().unwrap()
    }
    pub fn agent(&self, seed: &[&str]) -> Arc<FakeAgent> {
        let s = self
            .ctx
            .get::<Sessions>()
            .unwrap()
            .create(&self.ctx, None, Default::default())
            .unwrap();
        for event in seed {
            match *event {
                "turn/start" => {
                    s.append(event, json!({"turn":1}), None).unwrap();
                }
                "turn/end" => {
                    s.append(event, json!({"turn":1,"reason":{"kind":"completed"}}), None)
                        .unwrap();
                }
                "user/message" => {
                    let m = create_user_message(NewUserMessage {
                        content: vec![ContentBlock::text("seed")],
                        source: MessageSource::new("user"),
                    });
                    s.append(
                        event,
                        serde_json::to_value(m).unwrap(),
                        Some(SurfaceIntent::append()),
                    )
                    .unwrap();
                }
                _ => panic!("unknown test seed"),
            }
        }
        self.agent_for(s)
    }
    pub fn open_agent(&self) -> Arc<FakeAgent> {
        self.agent(&["turn/start", "user/message"])
    }
    pub fn agent_for(&self, session: Session) -> Arc<FakeAgent> {
        Arc::new(FakeAgent {
            session,
            ctx: self.ctx.clone(),
            key: ScopeKey::new(),
            options: Default::default(),
            injected: Mutex::new(vec![]),
        })
    }
    pub async fn close(self) {
        self.kernel.shutdown().await;
    }
}
pub fn req(agent: Arc<dyn Agent>) -> ApprovalRequest {
    ApprovalRequest::new(agent, "echo")
}
pub fn events(s: &Session) -> Vec<Arc<SessionEvent>> {
    s.snapshot_events(None, None).to_vec()
}
pub fn audit(s: &Session) -> Vec<Arc<SessionEvent>> {
    events(s)
        .into_iter()
        .filter(|e| matches!(e.event_type(), "approval/asked" | "approval/decided"))
        .collect()
}
pub fn assert_pair(s: &Session, outcome: ApprovalOutcome) {
    let a = audit(s);
    assert_eq!(a.len(), 2);
    assert_eq!(a[0].event_type(), "approval/asked");
    assert_eq!(a[1].event_type(), "approval/decided");
    assert_eq!(a[0].data()["id"], a[1].data()["id"]);
    assert_eq!(a[1].data()["outcome"], json!(outcome));
}
pub fn answer(ctx: &Context, outcome: ApprovalOutcome) {
    ctx.on_waterfall::<ApprovalRequestEvent, Value, _>(
        move |_, _| async move { Ok(outcome.into()) },
    )
    .unwrap();
}
pub async fn context_text(h: &Harness, agent: Option<Arc<dyn Agent>>) -> Option<String> {
    let context = agent.map_or_else(AssembleContext::default, |a| {
        AssembleContext::default().with_extension(AssembleAgent(a))
    });
    h.ctx
        .get::<SystemPromptKey>()
        .unwrap()
        .assemble(context)
        .await
        .unwrap()
        .contexts
        .into_iter()
        .find(|e| e.name == "approval:policy")
        .map(|e| e.text)
}
pub async fn pump() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}
