//! Port of @deepseek-ai/dsh-agent (packages/core/agent).
//!
//! Agent service: live registry, factory delegation, and process-local initiator scope.
//! Concrete creation and driving belong to the loop (`hoh-agent-loop`).

pub mod consumed_work;
pub mod dispatch;
pub mod model_selection;
pub mod projection;
pub mod runtime_types;
pub mod types;

pub use consumed_work::{ConsumedWork, fold_consumed_work};
pub use dispatch::{
    AgentEvents, AgentSubjectEvent, AssembleAgent, AssembleModelSelection, agent_carrier,
    agent_events, agent_events_with, assemble_context_for, emit_agent_event,
};
pub use model_selection::{
    AgentModel, ModelSelectionProjectionState, install_model_selection,
    model_selection_projection_definition, model_selection_view,
};
pub use projection::{fold_definition, turn_boundary_projection_definition};
pub use runtime_types::*;
pub use types::*;

use futures_util::future::BoxFuture;
use hoh_error::BoxError;
use hoh_kernel::{
    Cfg, Context, Disposer, FiberState, Inject, InternalStatus, NoConfig, Plugin, Service, Undo,
};
use hoh_scope::Scoped;
use hoh_session::{Session, SessionEvent, SessionId, SessionLogOffset};
use hoh_session_persistence::{SessionAccess, SessionPersistenceService};
use hoh_timeout::AbortSignal;
use std::{
    future::Future,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tokio::sync::Notify;

/// Synchronous finalizer returned by unpublished Agent setup when its contributions need
/// validation at the exact publication commit point.
pub type AgentSetupCommit = Box<dyn FnOnce() -> Result<(), BoxError> + Send>;

/// Creation-time composition of the agent's scoped world (`AgentSetup`): trusted same-process
/// code that composes and never drives.
pub type AgentSetup = Arc<
    dyn Fn(
            Context,
            Arc<dyn Agent>,
        ) -> BoxFuture<'static, Result<Option<AgentSetupCommit>, BoxError>>
        + Send
        + Sync,
>;

/// Session creation metadata, mirroring `CreateSessionOptions.meta` of hoh-session without the
/// internal-only `createdAt`.
#[derive(Clone, Debug, Default)]
pub struct AgentSessionMeta {
    pub cwd: Option<String>,
    pub parent_session: Option<SessionId>,
    pub is_seeded: Option<bool>,
    /// The coarse `origin` classification (`subagent`).
    pub origin: Option<String>,
    pub delegation_depth: Option<u64>,
    pub agent_preset: Option<String>,
}

/// Options for programmatically creating an agent through the registry factory.
#[derive(Clone, Default)]
pub struct CreateAgentOptions {
    /// The live agent/session identity. Required: [`CreateAgentOptions::new`] sets it.
    pub session_id: Option<SessionId>,
    pub meta: Option<AgentSessionMeta>,
    /// Exact fork-inherited prefix length when the session metadata sets `is_seeded`.
    pub inherited_event_count: Option<SessionLogOffset>,
    /// Initial replay/fork history, contiguous from seq 0 with lossless-JSON data.
    pub seed: Option<Vec<Arc<SessionEvent>>>,
    /// Route fallback and output controls.
    pub agent_options: Option<AgentOptions>,
    /// Optional creation-only cancellation signal.
    pub signal: Option<AbortSignal>,
    pub setup: Option<AgentSetup>,
}

impl CreateAgentOptions {
    pub fn new(session_id: SessionId) -> Self {
        Self {
            session_id: Some(session_id),
            ..Self::default()
        }
    }
}

/// Options for resuming an agent on a persisted session.
#[derive(Clone)]
pub struct ResumeAgentOptions {
    /// The persisted session id to load and use as the live agent/session identity.
    pub resume_session_id: SessionId,
    pub agent_options: Option<AgentOptions>,
    pub signal: Option<AbortSignal>,
    pub setup: Option<AgentSetup>,
}

impl ResumeAgentOptions {
    pub fn new(resume_session_id: SessionId) -> Self {
        Self {
            resume_session_id,
            agent_options: None,
            signal: None,
            setup: None,
        }
    }
}

type DisposeFn = Arc<dyn Fn() -> BoxFuture<'static, Result<(), BoxError>> + Send + Sync>;

/// An owned agent plus its disposer: a capability — among consumers, only the holder can tear
/// this agent down.
#[derive(Clone)]
pub struct AgentHandle {
    pub agent: Arc<dyn Agent>,
    dispose: DisposeFn,
}

impl AgentHandle {
    pub fn new(
        agent: Arc<dyn Agent>,
        dispose: impl Fn() -> BoxFuture<'static, Result<(), BoxError>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            agent,
            dispose: Arc::new(dispose),
        }
    }
    /// Stop the loop, await its exit, unregister the agent, remove its session, unwind its scope.
    pub fn dispose(&self) -> BoxFuture<'static, Result<(), BoxError>> {
        (self.dispose)()
    }
}

/// The agent-creation factory the loop implementation provides to the registry via
/// [`AgentRegistry::set_factory`].
pub trait AgentFactory: Send + Sync + 'static {
    /// Create a new agent on a caller-supplied session id. `owner_ctx` is the `create()` caller's
    /// context: the transaction and resulting lifecycle attach to that owner.
    fn create_agent(
        &self,
        owner_ctx: Context,
        options: CreateAgentOptions,
    ) -> BoxFuture<'static, Result<AgentHandle, BoxError>>;
    /// Resume an agent on a persisted session.
    fn resume(
        &self,
        owner_ctx: Context,
        options: ResumeAgentOptions,
    ) -> BoxFuture<'static, Result<AgentHandle, BoxError>>;
}

/// Not in the source (HOH-18): an executor that drives its sessions instead of the agent loop — an
/// external agent harness with its own loop, tools and continuation (Claude Code, Codex). It is
/// registered under a name with [`AgentRegistry::set_executor`]; the loop's factory stays the
/// default for every other session.
///
/// Routing: [`AgentRegistry::create`] hands a session to the executor whose name equals the
/// session's provider route (`AgentOptions::provider`); [`AgentRegistry::resume`] hands a
/// persisted session to the executor that [`claims`](SessionExecutor::claims) it, from its durable
/// binding. With no executor registered both behave exactly as before.
pub trait SessionExecutor: AgentFactory {
    /// Whether the persisted session `id` is bound to this executor. A session that is not
    /// stored, or stored by the loop, is not claimed.
    fn claims(
        &self,
        owner_ctx: &Context,
        id: &SessionId,
    ) -> BoxFuture<'static, Result<bool, BoxError>>;
}

/// Not in the source (HOH-18): the plugin record that binds a session to the external harness
/// driving it (`{harness, nativeSessionId[, replaces]}`). The first one is written when the
/// session is created; a session that has one is never continued by the agent loop.
pub const EXECUTOR_BINDING_RECORD: &str = "plugin:external-harness/session";

/// How many events [`stored_executor_binding`] reads at a time.
pub const EXECUTOR_BINDING_SCAN: u64 = 16;

/// Events only a turn writes. The binding record is written before a session takes any input, so
/// it always comes before the first of them; records that listeners of `session/created` append
/// may precede it in any number (review 221315, item 5).
const TURN_EVENTS: [&str; 7] = [
    "turn/start",
    "turn/end",
    "user/message",
    "assistant/message",
    "tool/result",
    "system/message",
    "developer/message",
];

/// Where the binding scan of `events` (the next ones of the log) stopped.
enum BindingScan<'a> {
    Bound(&'a str),
    /// A turn began without a binding: the session is not bound.
    Unbound,
    /// Neither yet: the binding may follow.
    Open,
}

fn scan_binding(events: &[Arc<SessionEvent>]) -> BindingScan<'_> {
    for event in events {
        if event.event_type() == EXECUTOR_BINDING_RECORD {
            return event
                .data()
                .get("harness")
                .and_then(|name| name.as_str())
                .map_or(BindingScan::Unbound, BindingScan::Bound);
        }
        if TURN_EVENTS.contains(&event.event_type()) {
            return BindingScan::Unbound;
        }
    }
    BindingScan::Open
}

/// The harness named by the binding record among the events before the first turn of `events`
/// (a log from its start).
pub fn bound_executor(events: &[Arc<SessionEvent>]) -> Option<&str> {
    match scan_binding(events) {
        BindingScan::Bound(harness) => Some(harness),
        BindingScan::Unbound | BindingScan::Open => None,
    }
}

/// The harness the stored session `id` is bound to: the log is read from its start, a page at a
/// time, up to the binding or the first turn. `Ok(None)` when there is no store, the session is not
/// stored, or it is not bound. A store that fails to answer is an error, never "not bound": a log
/// that could not be checked is not handed to the loop (fail closed; review 232500, item 3). The
/// store's error is returned as it is, the one the loop's own resume would report.
pub async fn stored_executor_binding(
    ctx: &Context,
    id: &SessionId,
) -> Result<Option<String>, BoxError> {
    let Some(persistence) = ctx.get::<SessionPersistenceService>() else {
        return Ok(None);
    };
    if persistence.stat(id, None).await?.is_none() {
        return Ok(None);
    }
    let handle = persistence.open(id, SessionAccess::Read, None).await?;
    let mut from = 0;
    let bound = loop {
        let page = match handle
            .read(Some(from), Some(EXECUTOR_BINDING_SCAN), None)
            .await
        {
            Ok(page) => page,
            Err(error) => break Err(error),
        };
        if page.events.is_empty() {
            break Ok(None);
        }
        match scan_binding(&page.events) {
            BindingScan::Bound(harness) => break Ok(Some(harness.to_owned())),
            BindingScan::Unbound => break Ok(None),
            BindingScan::Open => from += page.events.len() as u64,
        }
    };
    let _ = handle.close().await;
    bound
}

/// Not in the source (HOH-18): the executor new chats go to, and the model it runs, as the user
/// chose it in the window (`hoh/executor/select`, a privileged command).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutorSelection {
    pub executor: String,
    pub model: String,
}

/// Thrown when create/resume is called before an agent factory is registered.
pub const NO_FACTORY_MESSAGE: &str = "no agent factory registered (load an agent-loop plugin)";
const NO_INITIATOR_MESSAGE: &str = "no initiating agent is active";
const DISPOSED_INITIATOR_MESSAGE: &str = "agent initiator scope is disposed";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[derive(Default)]
struct EntryState {
    announced: bool,
    announcing: bool,
    detach_requested: bool,
}

/// All mutable lifecycle state for one exact registry entry.
struct Entry {
    id: SessionId,
    agent: Arc<dyn Agent>,
    carrier: Scoped<dyn Agent>,
    ctx: Context,
    state: Mutex<EntryState>,
}

/// One tracked boundary plus its inherited nesting chain.
struct InitiatorRun {
    active: AtomicBool,
    parent: Option<Arc<InitiatorRun>>,
}

/// One frame of the task-local initiator chain.
struct Frame {
    registry: u64,
    agent: Option<Arc<dyn Agent>>,
    run: Arc<InitiatorRun>,
    parent: Option<Arc<Frame>>,
}

tokio::task_local! {
    static INITIATOR: Arc<Frame>;
}

/// `tokio::spawn` for a task that continues the current initiator chain, as every continuation
/// does under `AsyncLocalStorage` in the source. Outside any boundary it is a plain `tokio::spawn`.
/// The task shares the boundary's attribution only: it is not drained with the boundary, and after
/// the boundary ends it still reads the same initiator (as a detached continuation does).
pub fn spawn_inherited<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    match INITIATOR.try_with(Arc::clone) {
        Ok(frame) => tokio::spawn(INITIATOR.scope(frame, future)),
        Err(_) => tokio::spawn(future),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InitiatorState {
    Active,
    Closing,
    Disposed,
}

struct Initiators {
    state: InitiatorState,
    active_runs: usize,
}

struct Inner {
    id: u64,
    turn_boundary: hoh_session_projection::ProjectionHandle<TurnBoundaryProjection>,
    store: Mutex<Vec<Arc<Entry>>>,
    factory: Mutex<Option<Arc<dyn AgentFactory>>>,
    /// Named session executors (HOH-18), in registration order.
    executors: Mutex<Vec<(String, Arc<dyn SessionExecutor>)>>,
    /// The executor the user chose for new chats (HOH-18).
    selected: Mutex<Option<ExecutorSelection>>,
    initiators: Mutex<Initiators>,
    drained: Notify,
    initiator_disposal: Mutex<Option<()>>,
    /// Test seam: called by `announce` between finding its entry and marking it, with the
    /// registry lock held.
    #[cfg(test)]
    window_probe: Mutex<Option<WindowProbe>>,
}

#[cfg(test)]
type WindowProbe = Arc<dyn Fn(&Mutex<Vec<Arc<Entry>>>) + Send + Sync>;

/// Agent service (`ctx.agents`): tracks live agents and carries the initiating Agent through one
/// process-local asynchronous driver chain.
///
/// Initiator methods provide same-process causal attribution only. Ambient presence is neither
/// liveness proof nor authorization. The chain follows the awaited future given to
/// [`with_initiator`](Self::with_initiator); a task started with plain `tokio::spawn` does not
/// inherit it — start it with [`spawn_inherited`], which carries the chain over.
#[derive(Clone)]
pub struct AgentRegistry {
    inner: Arc<Inner>,
}

hoh_kernel::service!(pub Agents: AgentRegistry = "agents");

/// Detach capability returned by [`AgentRegistry::enter`]: idempotent.
#[derive(Clone)]
pub struct AgentDetach {
    registry: AgentRegistry,
    entry: Arc<Entry>,
    entered: Arc<AtomicBool>,
}

impl AgentDetach {
    /// Remove this exact entry and emit `agent/disposed` with listener failures contained. When
    /// called from an `agent/created` listener, removal waits until the serial creation dispatch
    /// settles.
    pub fn detach(&self) {
        if !self.entered.swap(false, Ordering::SeqCst) {
            return;
        }
        self.registry.detach_entered(&self.entry, true);
    }
    /// The closure as an ordered composite-effect step.
    pub fn into_undo(self) -> Undo {
        Undo::sync(move || self.detach())
    }
}

impl AgentRegistry {
    fn new(
        turn_boundary: hoh_session_projection::ProjectionHandle<TurnBoundaryProjection>,
    ) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            inner: Arc::new(Inner {
                id: NEXT.fetch_add(1, Ordering::Relaxed),
                turn_boundary,
                store: Mutex::new(Vec::new()),
                factory: Mutex::new(None),
                executors: Mutex::new(Vec::new()),
                selected: Mutex::new(None),
                initiators: Mutex::new(Initiators {
                    state: InitiatorState::Active,
                    active_runs: 0,
                }),
                drained: Notify::new(),
                initiator_disposal: Mutex::new(None),
                #[cfg(test)]
                window_probe: Mutex::new(None),
            }),
        }
    }

    /// Read the turn and step positions recorded in a Session.
    pub fn turn_boundary_of(
        &self,
        session: &Session,
    ) -> Result<Arc<TurnBoundaryProjection>, BoxError> {
        self.inner.turn_boundary.state_of(session)
    }

    // ---- initiator scope ------------------------------------------------------------------

    fn assert_initiators_readable(&self) -> Result<(), BoxError> {
        if lock(&self.inner.initiators).state == InitiatorState::Disposed {
            return Err(DISPOSED_INITIATOR_MESSAGE.into());
        }
        Ok(())
    }

    /// Read the Agent that initiated the inherited asynchronous driver chain; `None` outside an
    /// initiator boundary and inside an explicit clearing boundary.
    pub fn current_initiator(&self) -> Result<Option<Arc<dyn Agent>>, BoxError> {
        self.assert_initiators_readable()?;
        Ok(INITIATOR
            .try_with(|frame| {
                let mut cursor = Some(frame.clone());
                while let Some(frame) = cursor {
                    if frame.registry == self.inner.id {
                        return frame.agent.clone();
                    }
                    cursor = frame.parent.clone();
                }
                None
            })
            .unwrap_or(None))
    }

    /// Read the initiating Agent and fail when no initiator boundary is active.
    pub fn require_initiator(&self) -> Result<Arc<dyn Agent>, BoxError> {
        self.current_initiator()?
            .ok_or_else(|| NO_INITIATOR_MESSAGE.into())
    }

    /// The nearest frame of this registry in the chain that ends at `frame`.
    fn own_frame(&self, frame: &Arc<Frame>) -> Option<Arc<Frame>> {
        let mut cursor = Some(frame.clone());
        while let Some(frame) = cursor {
            if frame.registry == self.inner.id {
                return Some(frame);
            }
            cursor = frame.parent.clone();
        }
        None
    }

    fn current_run(&self) -> Option<Arc<Frame>> {
        INITIATOR
            .try_with(|frame| self.own_frame(frame))
            .unwrap_or(None)
    }

    fn open_run(&self, agent: Option<Arc<dyn Agent>>) -> Result<(Arc<Frame>, RunGuard), BoxError> {
        {
            let mut state = lock(&self.inner.initiators);
            if state.state != InitiatorState::Active {
                return Err(DISPOSED_INITIATOR_MESSAGE.into());
            }
            state.active_runs += 1;
        }
        let parent = self.current_run();
        let run = Arc::new(InitiatorRun {
            active: AtomicBool::new(true),
            parent: parent.as_ref().map(|frame| frame.run.clone()),
        });
        let outer = INITIATOR.try_with(Arc::clone).ok();
        let frame = Arc::new(Frame {
            registry: self.inner.id,
            agent,
            run: run.clone(),
            parent: outer,
        });
        Ok((
            frame,
            RunGuard {
                registry: self.clone(),
                run,
            },
        ))
    }

    /// Run an asynchronous operation with one exact Agent as its process-local initiator. Fails
    /// synchronously when the initiator scope is closing or disposed. The boundary lasts until the
    /// returned future completes or is dropped.
    pub fn with_initiator<F>(
        &self,
        agent: Arc<dyn Agent>,
        operation: F,
    ) -> Result<impl Future<Output = F::Output> + Send + 'static, BoxError>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.scoped_future(Some(agent), operation)
    }

    /// Run an asynchronous operation inside a boundary that hides any inherited initiating Agent
    /// (lazy shared timers, queue pumps, watchers): it clears only attribution.
    pub fn without_initiator<F>(
        &self,
        operation: F,
    ) -> Result<impl Future<Output = F::Output> + Send + 'static, BoxError>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.scoped_future(None, operation)
    }

    fn scoped_future<F>(
        &self,
        agent: Option<Arc<dyn Agent>>,
        operation: F,
    ) -> Result<impl Future<Output = F::Output> + Send + 'static, BoxError>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let (frame, guard) = self.open_run(agent)?;
        Ok(async move {
            let _guard = guard;
            INITIATOR.scope(frame, operation).await
        })
    }

    /// Synchronous form of [`with_initiator`](Self::with_initiator): the boundary covers only `operation`.
    pub fn with_initiator_sync<T>(
        &self,
        agent: Arc<dyn Agent>,
        operation: impl FnOnce() -> T,
    ) -> Result<T, BoxError> {
        let (frame, _guard) = self.open_run(Some(agent))?;
        Ok(INITIATOR.sync_scope(frame, operation))
    }

    /// Synchronous form of [`without_initiator`](Self::without_initiator).
    pub fn without_initiator_sync<T>(&self, operation: impl FnOnce() -> T) -> Result<T, BoxError> {
        let (frame, _guard) = self.open_run(None)?;
        Ok(INITIATOR.sync_scope(frame, operation))
    }

    fn close_initiators(&self) {
        let mut state = lock(&self.inner.initiators);
        if state.state == InitiatorState::Active {
            state.state = InitiatorState::Closing;
        }
    }

    /// Exclude the boundary chain of the calling task from the teardown drain (a nested lineage
    /// that starts an owning-fiber unload is excluded from its own drain).
    ///
    /// Teardown does this itself for the callers of `Fiber::dispose()` (their chains reach it
    /// through the kernel's dispose captures); the call is for a boundary that starts the
    /// teardown some other way.
    pub fn release_reentrant_initiator_runs(&self) {
        self.release_chain(self.current_run());
    }

    /// Release the chains of the disposal callers not yet released (`seen` of them were).
    fn release_captured(&self, captured: Vec<Arc<Frame>>, seen: &mut usize) {
        for frame in captured.iter().skip(*seen) {
            self.release_chain(self.own_frame(frame));
        }
        *seen = captured.len();
    }

    fn release_chain(&self, frame: Option<Arc<Frame>>) {
        let mut run = frame.map(|f| f.run.clone());
        while let Some(current) = run {
            self.release_run(&current);
            run = current.parent.clone();
        }
    }

    fn release_run(&self, run: &Arc<InitiatorRun>) {
        if !run.active.swap(false, Ordering::SeqCst) {
            return;
        }
        let mut state = lock(&self.inner.initiators);
        state.active_runs -= 1;
        if state.active_runs == 0 {
            drop(state);
            self.inner.drained.notify_waiters();
        }
    }

    /// Wait for returned-future boundaries, then invalidate retained references.
    async fn dispose_initiators(&self, ctx: &Context) {
        if lock(&self.inner.initiator_disposal).is_some() {
            // The first disposal owns the drain; a repeat waits for the same end state.
            while self.assert_initiators_readable().is_ok() {
                tokio::task::yield_now().await;
            }
            return;
        }
        *lock(&self.inner.initiator_disposal) = Some(());
        self.close_initiators();
        // The boundaries whose code called `Fiber::dispose()` wait for this teardown: exclude
        // them from its drain, as the source does for the chain that started the unload.
        let mut seen = 0;
        self.release_captured(ctx.dispose_captures::<Frame>(), &mut seen);
        self.release_reentrant_initiator_runs();
        loop {
            let waiting = self.inner.drained.notified();
            tokio::pin!(waiting);
            // Register interest before re-reading the counter so a release cannot be missed.
            waiting.as_mut().enable();
            if lock(&self.inner.initiators).active_runs == 0 {
                break;
            }
            // A second disposal that arrives while this one drains brings its own chain.
            tokio::select! {
                () = &mut waiting => {}
                captured = ctx.next_dispose_captures::<Frame>(seen) => {
                    self.release_captured(captured, &mut seen);
                }
            }
        }
        lock(&self.inner.initiators).state = InitiatorState::Disposed;
    }

    // ---- factory --------------------------------------------------------------------------

    /// Register the agent-creation factory (the loop calls this on construction, effect-scoped).
    /// Fails if a factory is already registered. The returned disposer clears the factory slot.
    pub fn set_factory(
        &self,
        ctx: &Context,
        factory: Arc<dyn AgentFactory>,
    ) -> Result<Disposer, BoxError> {
        let registry = self.clone();
        ctx.try_effect(
            "agents.setFactory()",
            move |undos| -> Result<(), BoxError> {
                {
                    let mut slot = lock(&registry.inner.factory);
                    if slot.is_some() {
                        return Err("an agent factory is already registered".into());
                    }
                    *slot = Some(factory);
                }
                undos.push(Undo::sync(move || {
                    *lock(&registry.inner.factory) = None;
                }))?;
                Ok(())
            },
        )
    }

    fn require_factory(&self) -> Result<Arc<dyn AgentFactory>, BoxError> {
        lock(&self.inner.factory)
            .clone()
            .ok_or_else(|| NO_FACTORY_MESSAGE.into())
    }

    /// Create and publish a new agent through the registered factory. `ctx` is the caller's
    /// context: it owns the transaction and the live handle.
    ///
    /// A provider route that names a registered [`SessionExecutor`] creates the session on that
    /// executor instead (HOH-18). While the user has an executor selected for new chats
    /// ([`select_executor`](Self::select_executor)), a new top-level session (no parent, no
    /// origin, no seed) goes to it with the selected model, whatever route it was given.
    pub fn create(
        &self,
        ctx: &Context,
        options: CreateAgentOptions,
    ) -> BoxFuture<'static, Result<AgentHandle, BoxError>> {
        let top_level = options.seed.is_none()
            && options
                .meta
                .as_ref()
                .is_none_or(|meta| meta.parent_session.is_none() && meta.origin.is_none());
        if top_level && let Some(selection) = self.selected_executor() {
            let Some(executor) = self.executor(&selection.executor) else {
                let error = format!(
                    "the executor \"{}\" chosen for new chats is not loaded",
                    selection.executor
                );
                return Box::pin(std::future::ready(Err(error.into())));
            };
            let mut options = options;
            let route = options
                .agent_options
                .get_or_insert_with(AgentOptions::default);
            route.provider = Some(Live::Value(selection.executor));
            route.model = Some(Live::Value(selection.model));
            return executor.create_agent(ctx.clone(), options);
        }
        let routed = options
            .agent_options
            .as_ref()
            .and_then(AgentOptions::provider)
            .and_then(|provider| self.executor(&provider));
        if let Some(executor) = routed {
            return executor.create_agent(ctx.clone(), options);
        }
        match self.require_factory() {
            Ok(factory) => factory.create_agent(ctx.clone(), options),
            Err(error) => Box::pin(std::future::ready(Err(error))),
        }
    }

    /// Load a persisted session and resume an agent on it through the registered factory.
    ///
    /// When session executors are registered, a session one of them claims is resumed on that
    /// executor instead (HOH-18); the claims are asked in registration order. A stored session
    /// bound to an external harness that no executor claims (its package is not loaded) is
    /// refused: the loop must not continue a log another harness wrote.
    pub fn resume(
        &self,
        ctx: &Context,
        options: ResumeAgentOptions,
    ) -> BoxFuture<'static, Result<AgentHandle, BoxError>> {
        let executors = lock(&self.inner.executors).clone();
        let (registry, ctx) = (self.clone(), ctx.clone());
        Box::pin(async move {
            for (_, executor) in executors {
                if executor.claims(&ctx, &options.resume_session_id).await? {
                    return executor.resume(ctx, options).await;
                }
            }
            if let Some(harness) = stored_executor_binding(&ctx, &options.resume_session_id).await?
            {
                return Err(format!("session belongs to external harness {harness}").into());
            }
            registry.require_factory()?.resume(ctx, options).await
        })
    }

    // ---- session executors (HOH-18) -------------------------------------------------------

    /// Register a session executor under `name`, as an effect of `ctx`. Fails when the name is
    /// taken. The returned disposer removes it; sessions it already drives are not affected.
    pub fn set_executor(
        &self,
        ctx: &Context,
        name: &str,
        executor: Arc<dyn SessionExecutor>,
    ) -> Result<Disposer, BoxError> {
        let registry = self.clone();
        let name = name.to_owned();
        ctx.try_effect(
            format!("agents.setExecutor({name})"),
            move |undos| -> Result<(), BoxError> {
                {
                    let mut executors = lock(&registry.inner.executors);
                    if executors.iter().any(|(taken, _)| *taken == name) {
                        return Err(format!(
                            "an agent executor named \"{name}\" is already registered"
                        )
                        .into());
                    }
                    executors.push((name.clone(), executor.clone()));
                }
                undos.push(Undo::sync(move || {
                    lock(&registry.inner.executors).retain(|(taken, registered)| {
                        !(*taken == name && Arc::ptr_eq(registered, &executor))
                    });
                }))?;
                Ok(())
            },
        )
    }

    /// Choose the executor new chats go to (`None`: the agent loop, as without executors). Only
    /// the window's privileged command calls this: nothing a session says reaches it.
    pub fn select_executor(&self, selection: Option<ExecutorSelection>) {
        *lock(&self.inner.selected) = selection;
    }

    /// The executor chosen for new chats.
    pub fn selected_executor(&self) -> Option<ExecutorSelection> {
        lock(&self.inner.selected).clone()
    }

    /// The executor registered under `name`.
    pub fn executor(&self, name: &str) -> Option<Arc<dyn SessionExecutor>> {
        lock(&self.inner.executors)
            .iter()
            .find(|(taken, _)| taken == name)
            .map(|(_, executor)| executor.clone())
    }

    /// Names of the registered session executors, in registration order.
    pub fn executor_names(&self) -> Vec<String> {
        lock(&self.inner.executors)
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }

    // ---- registry -------------------------------------------------------------------------

    #[cfg(test)]
    pub(crate) fn set_window_probe(&self, probe: WindowProbe) {
        *lock(&self.inner.window_probe) = Some(probe);
    }

    /// Register a live agent with source `startup`. Rejects if the id is already registered or a
    /// serial `agent/created` listener fails. Emits `agent/disposed` when the calling fiber is
    /// disposed. Await registration before using the agent.
    pub async fn register(
        &self,
        ctx: &Context,
        agent: Arc<dyn Agent>,
    ) -> Result<Disposer, BoxError> {
        let registry = self.clone();
        let owner = ctx.clone();
        ctx.effect_async::<BoxError, _>("agents.register()", move |undos| async move {
            let detach = registry.enter(&owner, agent.clone())?;
            undos.push(detach.into_undo())?;
            registry
                .announce(&owner, &agent, SessionStartSource::Startup, None)
                .await
        })
        .await
    }

    /// Insert an already-constructed agent without announcing it: the advanced ordered-lifecycle
    /// primitive used by the async agent factory. Ordinary callers use [`register`](Self::register).
    pub fn enter(&self, ctx: &Context, agent: Arc<dyn Agent>) -> Result<AgentDetach, BoxError> {
        let id = agent.id().clone();
        if &id != agent.session().id() {
            return Err(format!(
                "agent id \"{id}\" does not match session id \"{}\"",
                agent.session().id()
            )
            .into());
        }
        let carrier = agent_carrier(&agent);
        let entry = Arc::new(Entry {
            id: id.clone(),
            agent,
            carrier,
            ctx: ctx.clone(),
            state: Mutex::new(EntryState::default()),
        });
        // The authoritative collision boundary.
        {
            let mut store = lock(&self.inner.store);
            if store.iter().any(|e| e.id == id) {
                return Err(format!("agent \"{id}\" is already registered").into());
            }
            store.push(entry.clone());
        }
        Ok(AgentDetach {
            registry: self.clone(),
            entry,
            entered: Arc::new(AtomicBool::new(true)),
        })
    }

    /// Remove one exact entered agent and emit its paired disposal when announced.
    ///
    /// The registry lock and the entry lock are held together (always in that order, as in
    /// [`announce`](Self::announce)), so the decision whether an announcement is under way, the
    /// removal and the read of `announced` are one step: a concurrent `announce` either wins
    /// (the removal is deferred until its dispatch settles, then `created` precedes `disposed`)
    /// or finds the entry gone (`not live`). With `defer`, a detach requested while the creation
    /// dispatch is running only records the request.
    fn detach_entered(&self, entry: &Arc<Entry>, defer: bool) {
        let announced = {
            let mut store = lock(&self.inner.store);
            let mut state = lock(&entry.state);
            if defer && state.announcing {
                state.detach_requested = true;
                return;
            }
            state.detach_requested = false;
            let Some(position) = store.iter().position(|e| Arc::ptr_eq(e, entry)) else {
                return;
            };
            store.remove(position);
            state.announced
        };
        // An insertion rolled back before announce was never externally created.
        if !announced {
            return;
        }
        self.emit_disposed(entry);
    }

    fn emit_disposed(&self, entry: &Arc<Entry>) {
        let log = entry.ctx.logger();
        let id = entry.id.clone();
        entry
            .carrier
            .context(&entry.ctx)
            .dispatch(AgentDisposed {
                agent: entry.agent.clone(),
            })
            .invoke(move |failure| {
                let (kind, error) = match failure {
                    hoh_kernel::ListenerFailure::Threw(error) => ("threw", error),
                    hoh_kernel::ListenerFailure::Rejected(error) => ("rejected", error),
                };
                log.warn(format_args!(
                    "agent \"{id}\": agent/disposed listener {kind}: {}",
                    dispatch::js_error_text(&error)
                ));
            });
    }

    /// Announce an agent previously inserted with [`enter`](Self::enter).
    pub async fn announce(
        &self,
        ctx: &Context,
        agent: &Arc<dyn Agent>,
        source: SessionStartSource,
        signal: Option<AbortSignal>,
    ) -> Result<(), BoxError> {
        // Lookup and marking are one step under the registry lock: a concurrent `detach` removes
        // the entry before this (`not live`) or after it (deferred behind the dispatch).
        let entry = {
            let store = lock(&self.inner.store);
            let entry = match store.iter().find(|e| &e.id == agent.id()) {
                Some(entry) if Arc::ptr_eq(&entry.agent, agent) => entry.clone(),
                _ => {
                    return Err(
                        format!("agent \"{}\" is not live in this registry", agent.id()).into(),
                    );
                }
            };
            #[cfg(test)]
            if let Some(probe) = lock(&self.inner.window_probe).clone() {
                probe(&self.inner.store);
            }
            let mut state = lock(&entry.state);
            if state.announced || state.announcing {
                return Err(format!("agent \"{}\" was already announced", entry.id).into());
            }
            // Mark before dispatch so a listener cannot recursively create a second lifecycle edge.
            state.announcing = true;
            state.announced = true;
            drop(state);
            entry
        };
        let result = entry
            .carrier
            .context(ctx)
            .serial::<AgentCreated, ()>(AgentCreated {
                agent: entry.agent.clone(),
                source,
                signal,
            })
            .await
            .map(|_| ());
        let detach_requested = {
            let mut state = lock(&entry.state);
            state.announcing = false;
            state.detach_requested
        };
        if detach_requested {
            self.detach_entered(&entry, false);
        }
        result
    }

    /// Look up a live agent.
    pub fn get(&self, id: &SessionId) -> Option<Arc<dyn Agent>> {
        lock(&self.inner.store)
            .iter()
            .find(|e| &e.id == id)
            .map(|e| e.agent.clone())
    }

    /// All live agents, in registration order.
    pub fn list(&self) -> Vec<Arc<dyn Agent>> {
        lock(&self.inner.store)
            .iter()
            .map(|e| e.agent.clone())
            .collect()
    }
}

struct RunGuard {
    registry: AgentRegistry,
    run: Arc<InitiatorRun>,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.registry.release_run(&self.run);
    }
}

/// Dependencies of the agent registry: `static inject = ['sessionProjections']`.
#[derive(Inject)]
pub struct AgentRegistryDeps {
    projections: Service<hoh_session_projection::SessionProjections>,
}

/// Uids of this fiber and all its ancestors, read once from the kernel snapshot. A kernel that is
/// already closed answers with an empty snapshot: the chain then holds only the fiber itself.
async fn lifecycle_lineage(ctx: &Context) -> Vec<u32> {
    let own = ctx.fiber().uid();
    let fibers = ctx.reflect().store().await.fibers;
    let mut lineage = vec![own];
    let mut uid = own;
    while let Some(info) = fibers.iter().find(|info| info.uid == uid) {
        if info.parent == uid || lineage.contains(&info.parent) {
            break;
        }
        lineage.push(info.parent);
        uid = info.parent;
    }
    lineage
}

/// The `AgentRegistry` plugin of the source (a class plugin named after its class).
pub struct AgentRegistryPlugin;

impl Plugin for AgentRegistryPlugin {
    const NAME: &'static str = "AgentRegistry";
    type Config = NoConfig;
    type Deps = AgentRegistryDeps;
    async fn apply(
        ctx: Context,
        deps: AgentRegistryDeps,
        _config: Cfg<NoConfig>,
    ) -> Result<(), BoxError> {
        // `[Service.init]`: register the turn-boundary projection before the service is visible.
        let handle = deps
            .projections
            .register(&ctx, turn_boundary_projection_definition())
            .await?;
        let registry = AgentRegistry::new(handle);
        // Close the initiator scope when this fiber or any ancestor starts unloading
        // (`hasLifecycleAncestor` of the source): the chain is fixed at creation.
        let lineage = lifecycle_lineage(&ctx).await;
        // `Fiber::dispose()` hands the caller's initiator chain to the teardown (see
        // `dispose_initiators`): the kernel's teardown tasks do not carry `tokio::task_local!`.
        // Adopted into the lifecycle effect below, so it is undone after the drain: a disposal
        // that arrives while the drain runs is still captured.
        let capture = ctx.capture_on_dispose(|| INITIATOR.try_with(Arc::clone).ok())?;
        {
            let registry = registry.clone();
            ctx.on::<InternalStatus>(move |event| {
                if event.fiber.state() == FiberState::Unloading
                    && lineage.contains(&event.fiber.uid())
                {
                    registry.close_initiators();
                }
            })?;
        }
        {
            let registry = registry.clone();
            let owner = ctx.clone();
            ctx.try_effect(
                "agents.initiatorLifecycle()",
                move |undos| -> Result<(), BoxError> {
                    // Undone in reverse: close first, then drain, then stop capturing.
                    undos.push(Undo::from(capture))?;
                    let drain = registry.clone();
                    undos.push(Undo::future(move || async move {
                        drain.dispose_initiators(&owner).await;
                        Ok(())
                    }))?;
                    let close = registry;
                    undos.push(Undo::sync(move || close.close_initiators()))?;
                    Ok(())
                },
            )?;
        }
        ctx.provide::<Agents>(Arc::new(registry)).await?;
        Ok(())
    }
}

#[cfg(test)]
extern crate self as hoh_agent;
#[cfg(test)]
#[path = "announce_window_tests.rs"]
mod announce_window_tests;
