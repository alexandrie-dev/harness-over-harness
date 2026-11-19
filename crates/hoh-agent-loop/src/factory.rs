//! Concrete agent factory: creates scoped `ReactLoopAgent`s, publishes them through the agent and
//! session registries and owns their ordered teardown.

use crate::agent::{CheckModel, LoopProjections, MaxParallel, ReactLoopAgent, ReadModel};
use crate::inbox::ReadInbox;
use futures_util::future::{BoxFuture, FutureExt, Shared};
use hoh_agent::{
    Agent, AgentDetach, AgentFactory, AgentHandle, AgentOptions, AgentSessionMeta, AgentSetup,
    Agents, CancelOptions, CreateAgentOptions, InboxState, ModelSelectionProjectionState,
    ResumeAgentOptions, SessionStartSource, run_maintenance,
};
use hoh_error::{BoxError, SharedError};
use hoh_kernel::{Context, Disposer, FiberState, Service, Undo};
use hoh_session::{
    AgentCancelCause, CreateSessionOptions, PrepareSessionOptions, RestoredSessionOptions, Session,
    SessionDetach, SessionHeader, SessionId, SessionLogOffset, Sessions, interrupted_turn_closers,
};
use hoh_session_persistence::{
    SessionAccess, SessionHandle, SessionHandleReadOptions, SessionPersistenceCreateOptions,
    SessionPersistenceOpenOptions, SessionPersistenceService,
};
use hoh_session_projection::ProjectionHandle;
use hoh_timeout::{AbortController, AbortSignal};
use serde_json::{Map, Value};
use std::{
    collections::HashMap,
    future::Future,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
};
use tokio::sync::{Notify, watch};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Re-throw one collected failure as itself, or several as one `AggregateError`.
fn collected(failures: Vec<BoxError>, message: &str) -> Result<(), BoxError> {
    let mut failures = failures;
    match failures.len() {
        0 => Ok(()),
        1 => Err(failures.remove(0)),
        _ => Err(Box::new(hoh_llm::AggregateError {
            message: message.to_owned(),
            errors: failures.into_iter().map(SharedError::new).collect(),
        })),
    }
}

type Teardown = Shared<BoxFuture<'static, Result<(), SharedError>>>;
type DisposeFn = Arc<dyn Fn() -> BoxFuture<'static, Result<(), SharedError>> + Send + Sync>;

/// Counted join point for work whose settlement factory teardown awaits.
#[derive(Default)]
struct Tasks {
    count: AtomicUsize,
    idle: Notify,
}

struct TaskGuard(Arc<Tasks>);
impl Drop for TaskGuard {
    fn drop(&mut self) {
        if self.0.count.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.idle.notify_waiters();
        }
    }
}

impl Tasks {
    fn begin(self: &Arc<Self>) -> TaskGuard {
        self.count.fetch_add(1, Ordering::AcqRel);
        TaskGuard(self.clone())
    }
    async fn settled(&self) {
        loop {
            let waiting = self.idle.notified();
            tokio::pin!(waiting);
            waiting.as_mut().enable();
            if self.count.load(Ordering::Acquire) == 0 {
                return;
            }
            waiting.await;
        }
    }
}

/// Factory-level ownership: live agent teardowns plus config startup work.
pub struct FactoryOwnership {
    accepting: AtomicBool,
    teardown: AbortController,
    live_agents: Mutex<HashMap<u64, DisposeFn>>,
    next_key: AtomicU64,
    tasks: Arc<Tasks>,
    ctx: Context,
}

impl FactoryOwnership {
    pub fn new(ctx: Context) -> Arc<Self> {
        Arc::new(Self {
            accepting: AtomicBool::new(true),
            teardown: AbortController::new(),
            live_agents: Mutex::new(HashMap::new()),
            next_key: AtomicU64::new(0),
            tasks: Arc::new(Tasks::default()),
            ctx,
        })
    }

    /// Aborts (reason: `agent loop is not active` error) when factory teardown begins.
    pub fn signal(&self) -> AbortSignal {
        self.teardown.signal()
    }

    pub fn is_active(&self) -> bool {
        self.accepting.load(Ordering::Acquire) && fiber_active(&self.ctx)
    }

    /// Track one live agent's shared teardown until it has run.
    fn track(self: &Arc<Self>, dispose: DisposeFn) -> impl FnOnce() + Send + 'static {
        let key = self.next_key.fetch_add(1, Ordering::Relaxed);
        lock(&self.live_agents).insert(key, dispose);
        let this = self.clone();
        move || {
            lock(&this.live_agents).remove(&key);
        }
    }

    /// Join config startup work or one public create/resume continuation.
    pub fn track_task(self: &Arc<Self>) -> impl Send + 'static {
        self.tasks.begin()
    }

    /// Resolve `job`, or stop waiting when factory teardown begins.
    pub async fn wait_while_active(&self, job: impl Future<Output = ()>) {
        let inactive = self.signal();
        tokio::select! {
            () = job => {}
            _ = inactive.cancelled() => {}
        }
    }

    pub async fn dispose(&self) -> Result<(), BoxError> {
        self.accepting.store(false, Ordering::Release);
        self.teardown.abort_with("agent loop is not active");
        let disposals: Vec<DisposeFn> = lock(&self.live_agents).values().cloned().collect();
        let results =
            futures_util::future::join_all(disposals.iter().map(|dispose| dispose())).await;
        self.tasks.settled().await;
        let failures: Vec<BoxError> = results
            .into_iter()
            .filter_map(Result::err)
            .map(|e| Box::new(e) as BoxError)
            .collect();
        collected(failures, "agent loop disposal failed")
    }
}

/// Fiber states that cannot own or serve a new lifecycle are the inactive ones.
pub fn fiber_active(ctx: &Context) -> bool {
    !matches!(
        ctx.fiber().state(),
        FiberState::Unloading | FiberState::Disposed | FiberState::Failed
    )
}

/// Await `operation`, or fail with the signal's reason as soon as it aborts.
pub async fn race_abort<T>(
    operation: impl Future<Output = T>,
    signal: &AbortSignal,
) -> Result<T, BoxError> {
    if let Some(reason) = signal.reason() {
        return Err(Box::new(reason));
    }
    tokio::select! {
        value = operation => Ok(value),
        reason = signal.cancelled() => Err(Box::new(reason)),
    }
}

/// Start an abortable operation and release a value that arrives after cancellation.
pub async fn race_abort_call<T: Send + 'static>(
    operation: impl Future<Output = Result<T, BoxError>> + Send + 'static,
    signal: &AbortSignal,
    release_abandoned: impl FnOnce(T) + Send + 'static,
) -> Result<T, BoxError> {
    if let Some(reason) = signal.reason() {
        return Err(Box::new(reason));
    }
    let mut task = tokio::spawn(operation);
    tokio::select! {
        joined = &mut task => joined.map_err(|e| BoxError::from(e.to_string()))?,
        reason = signal.cancelled() => {
            tokio::spawn(async move {
                if let Ok(Ok(value)) = task.await {
                    release_abandoned(value);
                }
            });
            Err(Box::new(reason))
        }
    }
}

/// Reject an output-token cap that cannot be represented exactly on the request wire.
fn assert_agent_options(options: &AgentOptions) -> Result<(), BoxError> {
    if let Some(max_tokens) = options.max_tokens()
        && (max_tokens == 0 || max_tokens > 9_007_199_254_740_991)
    {
        return Err("agent maxTokens must be a positive safe integer".into());
    }
    Ok(())
}

/// One session's owned write handle plus the count of events already stored through it.
pub struct StoredSession {
    pub handle: Arc<dyn SessionHandle>,
    pub stored_count: AtomicUsize,
}

/// Acquired Session resources and composition inputs for one Agent initialization.
struct AgentInitialization {
    session: Session,
    agent_options: AgentOptions,
    source: SessionStartSource,
    stored: Option<Arc<StoredSession>>,
    setup: Option<AgentSetup>,
    signal: Option<AbortSignal>,
}

/// State shared by every racing owner of one agent's lifecycle.
struct Lifecycle {
    id: SessionId,
    abort: Arc<AbortController>,
    ownership: Arc<FactoryOwnership>,
    machine: Mutex<Option<Arc<ReactLoopAgent>>>,
    stored: Option<Arc<StoredSession>>,
    detach_session: Mutex<Option<SessionDetach>>,
    detach_agent: Mutex<Option<AgentDetach>>,
    ready: AtomicBool,
    publication: Mutex<Option<watch::Receiver<bool>>>,
    disposing: Mutex<Option<Teardown>>,
    untrack: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    unfollow_owner: Mutex<Option<Disposer>>,
    unfollow_loop: Mutex<Option<Disposer>>,
}

impl Lifecycle {
    /// Reverse teardown, memoized so every racing owner awaits one quiescence: stop the machine,
    /// unwind the scope, drain and close the session's write path, leave the registries, release
    /// bookkeeping.
    fn teardown(self: &Arc<Self>) -> Teardown {
        let mut slot = lock(&self.disposing);
        if let Some(existing) = slot.as_ref() {
            return existing.clone();
        }
        let this = self.clone();
        let shared = async move { this.run_teardown().await.map_err(SharedError::new) }
            .boxed()
            .shared();
        *slot = Some(shared.clone());
        shared
    }

    async fn run_teardown(self: Arc<Self>) -> Result<(), BoxError> {
        self.abort.abort_with(if self.ownership.is_active() {
            format!("agent \"{}\" lifecycle disposed", self.id)
        } else {
            "agent loop is not active".to_owned()
        });
        // Teardown failures are collected, never swallowed: registry, scope and ownership cleanup
        // always run to quiescence, then the memoized disposal rejects with what failed.
        let mut failures: Vec<BoxError> = Vec::new();
        // Creation listeners retain the session and scope through their awaits.
        let publication = lock(&self.publication).clone();
        if let Some(mut publication) = publication {
            let _ = publication.wait_for(|done| *done).await;
        }
        let machine = lock(&self.machine).clone();
        if let Some(machine) = &machine {
            // Disposal IS a disposed-cause cancel followed by quiescence.
            let keep_inbox = !self.ready.load(Ordering::Acquire);
            if let Err(error) =
                machine.cancel(AgentCancelCause::Disposed, CancelOptions { keep_inbox })
            {
                failures.push(error);
            }
            // A failed queue write must not release resources underneath work.
            machine.when_idle().await;
            machine.scope.dispose().await;
        }
        // Close drains events already routed into the handle.
        if let Some(stored) = &self.stored
            && let Err(error) = stored.handle.close().await
        {
            failures.push(error);
        }
        // Take each handle out first: no lock is held while its callback runs.
        let detach_agent = lock(&self.detach_agent).take();
        if let Some(detach) = detach_agent {
            detach.detach();
        }
        let detach_session = lock(&self.detach_session).take();
        if let Some(detach) = detach_session {
            detach.detach();
        }
        let untrack = lock(&self.untrack).take();
        if let Some(untrack) = untrack {
            untrack();
        }
        collected(failures, &format!("agent \"{}\" disposal failed", self.id))
    }

    /// The shared disposal handed to owners: teardown, then leave the owner's effect.
    async fn dispose(self: Arc<Self>) -> Result<(), SharedError> {
        let result = self.teardown().await;
        let unfollow = lock(&self.unfollow_owner).take();
        if let Some(unfollow) = unfollow {
            unfollow.dispose().await;
        }
        let unfollow = lock(&self.unfollow_loop).take();
        if let Some(unfollow) = unfollow {
            unfollow.dispose().await;
        }
        result
    }
}

hoh_kernel::service!(pub AgentLoopKey: AgentLoop = "agentLoop");

/// Concrete agent factory and driver service.
#[derive(Clone)]
pub struct AgentLoop {
    pub(crate) inner: Arc<Inner>,
}

pub(crate) struct Inner {
    pub ctx: Context,
    pub ownership: Arc<FactoryOwnership>,
    pub model_projection: Arc<ProjectionHandle<ModelSelectionProjectionState>>,
    pub inbox_projection: Arc<ProjectionHandle<InboxState>>,
    pub max_parallel: MaxParallel,
}

fn session_meta(meta: &AgentSessionMeta) -> Value {
    let mut map = Map::new();
    if let Some(cwd) = &meta.cwd {
        map.insert("cwd".into(), cwd.clone().into());
    }
    if let Some(parent) = &meta.parent_session {
        map.insert("parentSession".into(), parent.to_string().into());
    }
    if let Some(is_seeded) = meta.is_seeded {
        map.insert("isSeeded".into(), is_seeded.into());
    }
    if let Some(origin) = &meta.origin {
        map.insert("origin".into(), origin.clone().into());
    }
    if let Some(depth) = meta.delegation_depth {
        map.insert("delegationDepth".into(), depth.into());
    }
    if let Some(preset) = &meta.agent_preset {
        map.insert("agentPreset".into(), preset.clone().into());
    }
    Value::Object(map)
}

impl AgentLoop {
    pub(crate) fn new(inner: Inner) -> Self {
        Self {
            inner: Arc::new(inner),
        }
    }

    pub fn ownership(&self) -> &Arc<FactoryOwnership> {
        &self.inner.ownership
    }

    fn sessions(&self) -> Result<Service<Sessions>, BoxError> {
        self.inner
            .ctx
            .get::<Sessions>()
            .ok_or_else(|| "sessions service is unavailable".into())
    }

    /// Own initialization and teardown from an acquired Session to a published Agent. Failed
    /// initialization drains cleanup while retaining stored events and inbox input.
    async fn initialize_agent(
        &self,
        owner_ctx: Context,
        init: AgentInitialization,
    ) -> Result<AgentHandle, BoxError> {
        let AgentInitialization {
            session,
            agent_options,
            source,
            stored,
            setup,
            signal: caller_signal,
        } = init;
        let id = session.id().clone();
        let loop_ctx = self.inner.ctx.clone();
        let ownership = self.inner.ownership.clone();

        // The caller's signal governs creation. Owner disposal and factory shutdown end the
        // Agent's lifetime both during and after creation.
        let abort = Arc::new(AbortController::new());
        let lifetime = AbortSignal::any([abort.signal(), ownership.signal()]);
        let creation = match &caller_signal {
            Some(caller) => AbortSignal::any([abort.signal(), ownership.signal(), caller.clone()]),
            None => lifetime.clone(),
        };

        let prelude = (|| -> Result<(Arc<ReactLoopAgent>, u64), BoxError> {
            assert_agent_options(&agent_options)?;
            if !fiber_active(&owner_ctx) {
                return Err("owner fiber is not active".into());
            }
            if !ownership.is_active() {
                return Err("agent loop is not active".into());
            }
            if let Some(reason) = caller_signal.as_ref().and_then(AbortSignal::reason) {
                return Err(Box::new(reason));
            }
            let agents = loop_ctx
                .get::<Agents>()
                .ok_or("agents service is unavailable")?;
            let last_turn = agents.turn_boundary_of(&session)?.last_turn;
            let model_check: CheckModel = {
                let handle = self.inner.model_projection.clone();
                let session = session.clone();
                Arc::new(move || handle.state_of(&session))
            };
            // `Model::current` cannot return an error: a broken projection throws, as in the
            // source (the pre-step reads `model_check` first, so the driver sees an `Err`).
            let reader_model: ReadModel = {
                let check = model_check.clone();
                Arc::new(move || check().unwrap_or_else(|error| panic!("{error}")))
            };
            let reader_inbox: ReadInbox = {
                let handle = self.inner.inbox_projection.clone();
                let session = session.clone();
                Arc::new(move || handle.state_of(&session))
            };
            let machine = ReactLoopAgent::new(
                loop_ctx.clone(),
                id.clone(),
                agent_options.clone(),
                session.clone(),
                LoopProjections {
                    model_selection: reader_model,
                    model_check,
                    inbox: reader_inbox,
                },
                last_turn,
                lifetime.clone(),
                self.inner.max_parallel.clone(),
            )?;
            Ok((machine, last_turn))
        })();
        let machine = match prelude {
            Ok((machine, _)) => machine,
            Err(error) => {
                if let Some(stored) = &stored {
                    let _ = stored.handle.close().await;
                }
                return Err(error);
            }
        };

        let lc = Arc::new(Lifecycle {
            id: id.clone(),
            abort: abort.clone(),
            ownership: ownership.clone(),
            machine: Mutex::new(Some(machine.clone())),
            stored: stored.clone(),
            detach_session: Mutex::new(None),
            detach_agent: Mutex::new(None),
            ready: AtomicBool::new(false),
            publication: Mutex::new(None),
            disposing: Mutex::new(None),
            untrack: Mutex::new(None),
            unfollow_owner: Mutex::new(None),
            unfollow_loop: Mutex::new(None),
        });
        let dispose_fn: DisposeFn = {
            let lc = lc.clone();
            Arc::new(move || lc.clone().dispose().boxed())
        };
        *lock(&lc.untrack) = Some(Box::new(ownership.track(dispose_fn.clone())));

        // Follow the owner: its disposal tears this lifecycle down.
        let follow = {
            let lc = lc.clone();
            owner_ctx.try_effect(
                format!("agentLoop.lifecycle({id})"),
                move |undos| -> Result<(), BoxError> {
                    undos.push(Undo::future(move || async move {
                        lc.abort.abort_with(format!(
                            "agent \"{}\" setup aborted: owner disposed during setup",
                            lc.id
                        ));
                        lc.teardown().await.map_err(|e| Box::new(e) as BoxError)
                    }))?;
                    Ok(())
                },
            )
        };
        match follow {
            Ok(disposer) => *lock(&lc.unfollow_owner) = Some(disposer),
            Err(error) => {
                lc.clone().dispose().await.ok();
                return Err(error);
            }
        }

        // The loop's own unload tears this agent down too, in the source's order: the scope's raw
        // disposal is adopted into this ordered effect, so the reverse-order unload runs the
        // teardown (cancel, quiesce, drain) first and disposes the scope only afterwards, never
        // underneath the agent's work.
        let loop_follow = {
            let (lc, scope) = (lc.clone(), machine.scope.raw_dispose());
            loop_ctx.try_effect(
                format!("agentLoop.scope({id})"),
                move |undos| -> Result<(), BoxError> {
                    undos.push(scope)?;
                    undos.push(Undo::future(move || async move {
                        lc.teardown().await.map_err(|e| Box::new(e) as BoxError)
                    }))?;
                    Ok(())
                },
            )
        };
        match loop_follow {
            Ok(disposer) => *lock(&lc.unfollow_loop) = Some(disposer),
            Err(error) => {
                lc.clone().dispose().await.ok();
                return Err(error);
            }
        }

        let agent: Arc<dyn Agent> = machine.clone();
        let body = {
            let (lc, agent, creation, loop_ctx) = (
                lc.clone(),
                agent.clone(),
                creation.clone(),
                loop_ctx.clone(),
            );
            let sessions = self.sessions()?;
            let stored = stored.clone();
            let session = session.clone();
            move |_signal: AbortSignal| async move {
                let (publication_done, publication) = watch::channel(false);
                *lock(&lc.publication) = Some(publication);
                let outcome: Result<(), BoxError> = async {
                    creation.throw_if_aborted()?;
                    append_unstored_suffix(stored.as_deref(), &session).await?;
                    creation.throw_if_aborted()?;
                    let detach = sessions.enter(agent.ctx(), session.clone())?;
                    *lock(&lc.detach_session) = Some(detach);
                    sessions.announce(&session)?;
                    creation.throw_if_aborted()?;
                    if let Some(setup) = &setup {
                        let commit =
                            race_abort(setup(agent.ctx().clone(), agent.clone()), &creation)
                                .await??;
                        creation.throw_if_aborted()?;
                        if let Some(commit) = commit {
                            commit()?;
                        }
                        creation.throw_if_aborted()?;
                    }
                    let agents = loop_ctx
                        .get::<Agents>()
                        .ok_or("agents service is unavailable")?;
                    let detach = agents.enter(&loop_ctx, agent.clone())?;
                    *lock(&lc.detach_agent) = Some(detach);
                    agents
                        .announce(&loop_ctx, &agent, source, Some(creation.clone()))
                        .await?;
                    creation.throw_if_aborted()?;
                    lc.ready.store(true, Ordering::Release);
                    Ok(())
                }
                .await;
                let outcome = outcome.inspect_err(|_| {
                    // Failed initialization must not release queued input into a new turn.
                    let _ = agent.cancel(
                        AgentCancelCause::Disposed,
                        CancelOptions { keep_inbox: true },
                    );
                });
                publication_done.send_replace(true);
                *lock(&lc.publication) = None;
                outcome
            }
        };
        let published = match run_maintenance(&*machine, body) {
            Ok(future) => future.await,
            Err(error) => Err(error),
        };
        match published {
            Ok(()) => Ok(AgentHandle::new(agent, move || {
                let dispose = dispose_fn.clone();
                async move { dispose().await.map_err(|e| Box::new(e) as BoxError) }.boxed()
            })),
            Err(error) => {
                // Initialization failure remains primary after cleanup reaches quiescence.
                let _ = lc.clone().dispose().await;
                Err(error)
            }
        }
    }

    /// Create an agent and session under one caller-supplied identity, owned by the loop's own
    /// context. When a persistence backend is mounted, the session's durable identity is stored
    /// before publication.
    pub async fn create(
        &self,
        id: SessionId,
        options: AgentOptions,
        meta: AgentSessionMeta,
    ) -> Result<Arc<dyn Agent>, BoxError> {
        let sessions = self.sessions()?;
        let session = sessions.prepare(
            Some(id),
            PrepareSessionOptions::Create(CreateSessionOptions {
                seed: None,
                inherited_event_count: None,
                meta: Some(session_meta(&meta)),
            }),
        )?;
        let stored = self.create_stored_session(&session, None).await?;
        let handle = self
            .initialize_agent(
                self.inner.ctx.clone(),
                AgentInitialization {
                    session,
                    stored,
                    agent_options: options,
                    source: SessionStartSource::Startup,
                    setup: None,
                    signal: None,
                },
            )
            .await?;
        Ok(handle.agent)
    }

    /// Take a fresh session's write ownership when persistence is mounted. Creation failure
    /// retains any events already written to the handle.
    async fn create_stored_session(
        &self,
        session: &Session,
        signal: Option<AbortSignal>,
    ) -> Result<Option<Arc<StoredSession>>, BoxError> {
        let Some(persistence) = self.inner.ctx.get::<SessionPersistenceService>() else {
            return Ok(None);
        };
        let header: SessionHeader = serde_json::from_value((*session.header()).clone())?;
        let options = SessionPersistenceCreateOptions {
            signal,
            inherited_event_count: Some(session.inherited_event_count()),
        };
        let handle = persistence.create(&header, Some(&options)).await?;
        Ok(Some(Arc::new(StoredSession {
            handle,
            stored_count: AtomicUsize::new(0),
        })))
    }

    async fn create_agent_inner(
        &self,
        owner_ctx: Context,
        options: CreateAgentOptions,
    ) -> Result<AgentHandle, BoxError> {
        let sessions = self.sessions()?;
        let session = sessions.prepare(
            options.session_id.clone(),
            PrepareSessionOptions::Create(CreateSessionOptions {
                seed: options.seed.clone(),
                inherited_event_count: options.inherited_event_count,
                meta: options.meta.as_ref().map(session_meta),
            }),
        )?;
        // `race_abort_call` closes a handle that finishes creating after cancellation.
        let stored = match &options.signal {
            None => self.create_stored_session(&session, None).await?,
            Some(signal) => {
                let this = self.clone();
                let task_session = session.clone();
                let task_signal = signal.clone();
                race_abort_call(
                    async move {
                        this.create_stored_session(&task_session, Some(task_signal))
                            .await
                    },
                    signal,
                    |abandoned: Option<Arc<StoredSession>>| {
                        if let Some(abandoned) = abandoned {
                            tokio::spawn(async move {
                                let _ = abandoned.handle.close().await;
                            });
                        }
                    },
                )
                .await?
            }
        };
        self.initialize_agent(
            owner_ctx,
            AgentInitialization {
                session,
                stored,
                agent_options: options.agent_options.unwrap_or_default(),
                setup: options.setup,
                signal: options.signal,
                source: SessionStartSource::Startup,
            },
        )
        .await
    }

    /// Resume an owned agent from the configured persistence service.
    pub async fn resume_agent(
        &self,
        owner_ctx: Context,
        options: ResumeAgentOptions,
    ) -> Result<AgentHandle, BoxError> {
        let Some(persistence) = self.inner.ctx.get::<SessionPersistenceService>() else {
            return Err("cannot resume: session persistence is not configured (load a hoh-session-persistence backend)".into());
        };
        self.resume_with(owner_ctx, persistence, options).await
    }

    /// Resume through an explicit persistence handle used by the deferred config path.
    pub async fn resume_with(
        &self,
        owner_ctx: Context,
        persistence: Service<SessionPersistenceService>,
        options: ResumeAgentOptions,
    ) -> Result<AgentHandle, BoxError> {
        let _tracked = self.inner.ownership.track_task();
        let id = options.resume_session_id.clone();
        // The open and read may outlive their owner: race them against caller cancellation,
        // owner-fiber unload and factory teardown so a never-settling backend cannot pin the
        // identity.
        let owner_abort = Arc::new(AbortController::new());
        let unfollow_owner = {
            let owner_abort = owner_abort.clone();
            let id = id.clone();
            owner_ctx.try_effect(
                format!("agentLoop.resume-load({id})"),
                move |undos| -> Result<(), BoxError> {
                    undos.push(Undo::sync(move || {
                        owner_abort.abort_with(format!(
                            "agent \"{id}\" setup aborted: owner disposed during setup"
                        ));
                    }))?;
                    Ok(())
                },
            )?
        };
        let mut signals = vec![owner_abort.signal(), self.inner.ownership.signal()];
        if let Some(caller) = &options.signal {
            signals.push(caller.clone());
        }
        let fused = AbortSignal::any(signals);

        let loaded = self.load_for_resume(&persistence, &id, &fused).await;
        unfollow_owner.dispose().await;
        let (handle, stored, session) = loaded?;
        let refusal: Option<&str> = if !fiber_active(&owner_ctx) {
            Some("owner fiber is not active")
        } else if !self.inner.ownership.is_active() {
            Some("agent loop is not active")
        } else {
            None
        };
        if let Some(reason) = refusal {
            // The write handle is released before the error surfaces, so a retry can reopen it.
            let _ = handle.close().await;
            return Err(reason.into());
        }
        // `initialize_agent` owns the write handle through rollback and teardown.
        self.initialize_agent(
            owner_ctx,
            AgentInitialization {
                session,
                stored: Some(stored),
                agent_options: options.agent_options.unwrap_or_default(),
                setup: options.setup,
                signal: options.signal,
                source: SessionStartSource::Resume,
            },
        )
        .await
    }

    async fn load_for_resume(
        &self,
        persistence: &Service<SessionPersistenceService>,
        id: &SessionId,
        fused: &AbortSignal,
    ) -> Result<(Arc<dyn SessionHandle>, Arc<StoredSession>, Session), BoxError> {
        // Taking write ownership FIRST excludes a concurrent resume of the same id.
        let open = {
            let persistence = persistence.clone();
            let id = id.clone();
            let signal = fused.clone();
            async move {
                let options = SessionPersistenceOpenOptions {
                    signal: Some(signal),
                };
                persistence
                    .open(&id, SessionAccess::Write, Some(&options))
                    .await
            }
        };
        let handle = race_abort_call(open, fused, |abandoned: Arc<dyn SessionHandle>| {
            tokio::spawn(async move {
                let _ = abandoned.close().await;
            });
        })
        .await?;
        // Everything after the open owns the write handle: on failure it is closed (and awaited)
        // before the error is returned, so the next resume of the same id can reopen it.
        match self.restore_session(&handle, id, fused).await {
            Ok((stored, session)) => Ok((handle, stored, session)),
            Err(error) => {
                let _ = handle.close().await;
                Err(error)
            }
        }
    }

    async fn restore_session(
        &self,
        handle: &Arc<dyn SessionHandle>,
        id: &SessionId,
        fused: &AbortSignal,
    ) -> Result<(Arc<StoredSession>, Session), BoxError> {
        // Semantic crash repair is the agent layer's job: an interrupted final turn receives
        // synthetic closers appended through the same handle as an ordinary batch.
        let read = SessionHandleReadOptions {
            signal: Some(fused.clone()),
        };
        let cold = handle.read(Some(0), None, Some(&read)).await?;
        fused.throw_if_aborted()?;
        let persisted = cold.events;
        let closers = interrupted_turn_closers(&persisted)?;
        if !closers.is_empty() {
            handle.append(&closers, None).await?;
        }
        let mut seed = persisted.clone();
        seed.extend(closers.iter().cloned());
        let session = self.sessions()?.prepare(
            Some(id.clone()),
            PrepareSessionOptions::Restored(RestoredSessionOptions {
                seed,
                meta: serde_json::to_value(handle.header())?,
                inherited_event_count: handle.inherited_event_count(),
                event_state: cold.event_state,
            }),
        )?;
        let stored = Arc::new(StoredSession {
            handle: handle.clone(),
            stored_count: AtomicUsize::new(persisted.len() + closers.len()),
        });
        append_unstored_suffix(Some(&stored), &session).await?;
        Ok((stored, session))
    }
}

/// Durably store the session events appended since the last stored cursor.
async fn append_unstored_suffix(
    stored: Option<&StoredSession>,
    session: &Session,
) -> Result<(), BoxError> {
    let Some(stored) = stored else {
        return Ok(());
    };
    let count = stored.stored_count.load(Ordering::Acquire);
    let suffix = session.snapshot_events(Some(SessionLogOffset::new(count as u64)?), None);
    if !suffix.is_empty() {
        stored.handle.append(&suffix, None).await?;
    }
    stored
        .stored_count
        .store(count + suffix.len(), Ordering::Release);
    Ok(())
}

impl AgentFactory for AgentLoop {
    fn create_agent(
        &self,
        owner_ctx: Context,
        options: CreateAgentOptions,
    ) -> BoxFuture<'static, Result<AgentHandle, BoxError>> {
        let this = self.clone();
        let tracked = self.inner.ownership.track_task();
        Box::pin(async move {
            let _tracked = tracked;
            this.create_agent_inner(owner_ctx, options).await
        })
    }

    fn resume(
        &self,
        owner_ctx: Context,
        options: ResumeAgentOptions,
    ) -> BoxFuture<'static, Result<AgentHandle, BoxError>> {
        let this = self.clone();
        Box::pin(async move { this.resume_agent(owner_ctx, options).await })
    }
}
