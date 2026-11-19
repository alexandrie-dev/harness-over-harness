#![allow(dead_code)]

use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentOptions, AgentRegistryPlugin, AgentStatus, Agents, CancelOptions, Inbox,
    InboxTarget, MaintenanceFuture, MaintenanceTask, Model, ModelSelection,
};
use hoh_error::BoxError;
use hoh_kernel::{Context, Fiber, Kernel, Service};
use hoh_llm::{MessageId, UserMessage};
use hoh_scope::ScopeKey;
use hoh_session::{AgentCancelCause, Session, SessionId};
use hoh_session_projection::SessionProjectionRegistry;
use hoh_terminal::*;
use serde_json::json;
use std::sync::{
    Arc, Barrier, Mutex, MutexGuard,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{Notify, oneshot};

pub fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct UnusedModel;

impl Model for UnusedModel {
    fn current(&self) -> Option<ModelSelection> {
        None
    }

    fn select(&self, _: &ModelSelection) -> Result<(), BoxError> {
        Err("terminal fixture does not drive the model".into())
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

struct StubAgent {
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

pub struct Harness {
    pub kernel: Kernel,
    pub ctx: Context,
    pub terminals: Service<Terminals>,
}

pub async fn harness() -> Harness {
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
    ctx.plugin::<TerminalSessionService>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let terminals = ctx.get::<Terminals>().unwrap();
    Harness {
        kernel,
        ctx,
        terminals,
    }
}

pub async fn owner(harness: &Harness, id: &str) -> (Fiber, Arc<dyn Agent>) {
    let slot = Arc::new(Mutex::new(None));
    let capture = slot.clone();
    let fiber = harness
        .ctx
        .inject::<(), _>(move |ctx, ()| {
            *lock(&capture) = Some(ctx);
            async { Ok(()) }
        })
        .unwrap();
    fiber.settled().await.unwrap();
    let owner_ctx = lock(&slot).clone().unwrap();
    let agent = stub_agent(&owner_ctx, id);
    harness
        .ctx
        .get::<Agents>()
        .unwrap()
        .register(&owner_ctx, agent.clone())
        .await
        .unwrap();
    (fiber, agent)
}

pub fn stub_agent(ctx: &Context, id: &str) -> Arc<dyn Agent> {
    let session = Session::create(SessionId::from(id), None, None, None, None).unwrap();
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

pub async fn dispose(fiber: &Fiber) {
    if let Some(disposer) = fiber.disposer() {
        disposer.dispose().await;
    }
}

#[derive(Default)]
pub struct Gate {
    open: AtomicBool,
    notify: Notify,
}

impl Gate {
    pub fn release(&self) {
        self.open.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    pub async fn wait(&self) {
        while !self.open.load(Ordering::Acquire) {
            self.notify.notified().await;
        }
    }
}

pub struct StubSession {
    pub motd: String,
    pub pid: Option<u32>,
    pub closed: Arc<Mutex<Vec<String>>>,
    pub status: Arc<Mutex<TerminalSessionStatus>>,
    pub reject_close: Arc<AtomicBool>,
    pub reject_send: Arc<AtomicBool>,
    pub close_gate: Arc<Mutex<Option<Arc<Gate>>>>,
    pub start_entered: Arc<Mutex<Option<Arc<Barrier>>>>,
    pub start_release: Arc<Mutex<Option<Arc<Barrier>>>>,
    pub operation: Arc<Mutex<Option<TerminalSendOperation>>>,
}

impl Default for StubSession {
    fn default() -> Self {
        Self {
            motd: "stub ready".into(),
            pid: Some(123),
            closed: Arc::new(Mutex::new(vec![])),
            status: Arc::new(Mutex::new(TerminalSessionStatus::Running)),
            reject_close: Arc::new(AtomicBool::new(false)),
            reject_send: Arc::new(AtomicBool::new(false)),
            close_gate: Arc::new(Mutex::new(None)),
            start_entered: Arc::new(Mutex::new(None)),
            start_release: Arc::new(Mutex::new(None)),
            operation: Arc::new(Mutex::new(None)),
        }
    }
}

impl TerminalBackendSession for StubSession {
    fn motd(&self) -> &str {
        &self.motd
    }

    fn pid(&self) -> Option<u32> {
        self.pid
    }

    fn start_send(&self, _: TerminalSendRequest) -> Result<TerminalSendOperation, BoxError> {
        if let Some(barrier) = lock(&self.start_entered).clone() {
            barrier.wait();
        }
        if let Some(barrier) = lock(&self.start_release).clone() {
            barrier.wait();
        }
        if self.reject_send.load(Ordering::Acquire) {
            return Ok(TerminalSendOperation::new(
                async { Err("send failed".into()) },
                || TerminalSendRead {
                    delta: String::new(),
                    truncated: false,
                },
                || false,
            ));
        }
        let (settle, settled) = oneshot::channel::<()>();
        let settle = Arc::new(Mutex::new(Some(settle)));
        let cancelled = settle.clone();
        let status = self.status.clone();
        let operation = TerminalSendOperation::new(
            async move {
                let _ = settled.await;
                Ok(TerminalSendResult {
                    viewport: "done".into(),
                    wait_reason: TerminalWaitReason::StdinRead,
                    session_status: lock(&status).clone(),
                    truncated: false,
                })
            },
            || TerminalSendRead {
                delta: "delta".into(),
                truncated: false,
            },
            move || {
                lock(&cancelled)
                    .take()
                    .is_some_and(|settle| settle.send(()).is_ok())
            },
        );
        *lock(&self.operation) = Some(operation.clone());
        Ok(operation)
    }

    fn read(&self, request: TerminalReadRequest) -> Result<TerminalReadResult, BoxError> {
        Ok(TerminalReadResult {
            text: format!(
                "{}:{}",
                request.offset.unwrap_or(0),
                request.count.unwrap_or(0)
            ),
            total_lines: 1,
            line_begin: 0,
            line_end: 1,
            truncated: false,
        })
    }

    fn signal(&self, signal: TerminalSignal) -> TerminalFuture<TerminalSignalResult> {
        Box::pin(async move {
            Ok(TerminalSignalResult {
                delivered: true,
                target_pgid: if signal == TerminalSignal::Sigint {
                    12
                } else {
                    13
                },
            })
        })
    }

    fn status(&self) -> TerminalSessionStatus {
        lock(&self.status).clone()
    }

    fn close(&self, reason: String) -> TerminalFuture<()> {
        lock(&self.closed).push(reason);
        let reject = self.reject_close.clone();
        let gate = lock(&self.close_gate).clone();
        let status = self.status.clone();
        let operation = lock(&self.operation).clone();
        Box::pin(async move {
            if let Some(gate) = gate {
                gate.wait().await;
            }
            if reject.load(Ordering::Acquire) {
                return Err("close failed".into());
            }
            *lock(&status) = TerminalSessionStatus::Exited {
                exit_code: Some(0),
                signal: None,
            };
            if let Some(operation) = operation {
                operation.cancel();
            }
            Ok(())
        })
    }
}

type SpawnFn = Arc<
    dyn Fn(TerminalBackendSpawnSpec) -> TerminalFuture<Arc<dyn TerminalBackendSession>>
        + Send
        + Sync,
>;

pub struct StubBackend {
    kind: String,
    spawn: SpawnFn,
}

pub type StubSessions = Arc<Mutex<Vec<Arc<StubSession>>>>;

impl TerminalBackend for StubBackend {
    fn kind(&self) -> &str {
        &self.kind
    }

    fn spawn(
        &self,
        spec: TerminalBackendSpawnSpec,
    ) -> TerminalFuture<Arc<dyn TerminalBackendSession>> {
        (self.spawn)(spec)
    }
}

pub fn custom_backend(
    kind: &str,
    spawn: impl Fn(TerminalBackendSpawnSpec) -> TerminalFuture<Arc<dyn TerminalBackendSession>>
    + Send
    + Sync
    + 'static,
) -> Arc<dyn TerminalBackend> {
    Arc::new(StubBackend {
        kind: kind.into(),
        spawn: Arc::new(spawn),
    })
}

pub fn backend(kind: &str) -> (Arc<dyn TerminalBackend>, StubSessions) {
    let sessions = Arc::new(Mutex::new(vec![]));
    let output = sessions.clone();
    let provider = custom_backend(kind, move |_| {
        let session = Arc::new(StubSession::default());
        lock(&output).push(session.clone());
        Box::pin(async move { Ok(session as Arc<dyn TerminalBackendSession>) })
    });
    (provider, sessions)
}

pub fn spawn_request(kind: &str) -> TerminalSpawnRequest {
    TerminalSpawnRequest {
        kind: kind.into(),
        name: None,
        cwd: None,
    }
}

pub fn send_request() -> TerminalSendRequest {
    TerminalSendRequest {
        text: "echo hi".into(),
        submit: true,
        signal: None,
    }
}

pub fn code<'a>(error: &'a (dyn std::error::Error + 'static)) -> Option<&'a str> {
    hoh_error::HarnessError::of(error).map(hoh_error::HarnessError::code)
}
