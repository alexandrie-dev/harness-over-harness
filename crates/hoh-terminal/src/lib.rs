//! Owner-scoped persistent terminal registry.

pub mod error;
pub mod types;

pub use error::*;
pub use types::*;

use futures_util::{
    FutureExt, StreamExt,
    future::{BoxFuture, Shared, join_all},
    stream::FuturesUnordered,
};
use hoh_agent::{Agent, Agents};
use hoh_error::{BoxError, HarnessError, SharedError};
use hoh_kernel::{Context, Disposer, Inject, NoConfig, Plugin, Service, Undo};
use hoh_timeout::{AbortController, AbortSignal};
use indexmap::IndexMap;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use tokio::sync::{mpsc, oneshot};

pub type TerminalFuture<T> = BoxFuture<'static, Result<T, BoxError>>;

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn boxed(error: impl Into<BoxError>) -> BoxError {
    error.into()
}

fn ready_error<T: Send + 'static>(error: impl Into<BoxError>) -> TerminalFuture<T> {
    Box::pin(std::future::ready(Err(error.into())))
}

fn terminal_error(message: impl Into<String>, code: TerminalErrorCode) -> BoxError {
    Box::new(TerminalError::new(message, code))
}

type SharedClose = Shared<BoxFuture<'static, Result<(), SharedError>>>;
type SharedStart = Shared<BoxFuture<'static, ()>>;

struct Closing {
    token: Arc<()>,
    future: SharedClose,
}

#[derive(Clone)]
struct Disposing {
    future: SharedClose,
}

struct Starting {
    token: Arc<()>,
    settled: SharedStart,
}

#[derive(Default)]
struct RecordState {
    starting: Option<Starting>,
    active: Option<Arc<()>>,
    closing: Option<Closing>,
}

struct SessionRecord {
    id: TerminalSessionId,
    owner: Arc<dyn Agent>,
    name: Option<String>,
    kind: String,
    session: Arc<dyn TerminalBackendSession>,
    state: Mutex<RecordState>,
}

struct PendingSpawn {
    owner: Arc<dyn Agent>,
    controller: AbortController,
    settled: Shared<BoxFuture<'static, ()>>,
    settle: Mutex<Option<oneshot::Sender<()>>>,
    cleanup_failure: Mutex<Option<SharedError>>,
}

struct SpawnDrive {
    owner: Arc<dyn Agent>,
    request: TerminalSpawnRequest,
    id: TerminalSessionId,
    caller_signal: Option<AbortSignal>,
    pending: Arc<PendingSpawn>,
    name_token: Option<Arc<()>>,
    spawn: TerminalFuture<Arc<dyn TerminalBackendSession>>,
}

impl PendingSpawn {
    fn new(owner: Arc<dyn Agent>) -> Arc<Self> {
        let (settle, settled) = oneshot::channel();
        let settled = async move {
            let _ = settled.await;
        }
        .boxed()
        .shared();
        Arc::new(Self {
            owner,
            controller: AbortController::new(),
            settled,
            settle: Mutex::new(Some(settle)),
            cleanup_failure: Mutex::new(None),
        })
    }

    fn finish(&self, cleanup_failure: Option<SharedError>) {
        *lock(&self.cleanup_failure) = cleanup_failure;
        if let Some(settle) = lock(&self.settle).take() {
            let _ = settle.send(());
        }
    }
}

struct ReservedName {
    owner: Arc<dyn Agent>,
    name: String,
    token: Arc<()>,
}

struct OwnerCleanup {
    owner: Arc<dyn Agent>,
    generation: Arc<()>,
    disposer: Option<Disposer>,
}

struct State {
    backends: IndexMap<String, Arc<dyn TerminalBackend>>,
    sessions: IndexMap<TerminalSessionId, Arc<SessionRecord>>,
    reserved_names: Vec<ReservedName>,
    pending_spawns: Vec<Arc<PendingSpawn>>,
    owner_cleanups: Vec<OwnerCleanup>,
    global_owner_cleanups: Vec<Arc<()>>,
    global_owner_failures: Vec<SharedError>,
    disposed_owners: Vec<Weak<dyn Agent>>,
    next_id: u64,
    disposing: bool,
    disposal: Option<Disposing>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            backends: IndexMap::new(),
            sessions: IndexMap::new(),
            reserved_names: vec![],
            pending_spawns: vec![],
            owner_cleanups: vec![],
            global_owner_cleanups: vec![],
            global_owner_failures: vec![],
            disposed_owners: vec![],
            next_id: 0,
            disposing: false,
            disposal: None,
        }
    }
}

struct Inner {
    agents: Service<Agents>,
    state: Mutex<State>,
    commands: mpsc::UnboundedSender<Command>,
}

#[derive(Clone)]
pub struct TerminalSessionService {
    inner: Arc<Inner>,
}

hoh_kernel::service!(pub Terminals: TerminalSessionService = "terminals");

enum Command {
    Drive(BoxFuture<'static, ()>),
    DisposeOwner {
        owner: Arc<dyn Agent>,
        result: oneshot::Sender<Result<(), SharedError>>,
    },
    DisposeAll {
        result: Option<oneshot::Sender<Result<(), SharedError>>>,
    },
}

enum JobOutcome {
    Continue,
    Stop(Result<(), SharedError>),
}

impl TerminalSessionService {
    fn install_disposal_fence(
        state: &mut State,
    ) -> (Disposing, Option<oneshot::Sender<Result<(), SharedError>>>) {
        if let Some(disposal) = &state.disposal {
            return (disposal.clone(), None);
        }
        let (settle, settled) = oneshot::channel();
        let future = async move {
            settled
                .await
                .unwrap_or_else(|_| Err(SharedError::new("PTY service cleanup driver stopped")))
        }
        .boxed()
        .shared();
        let disposal = Disposing { future };
        state.disposing = true;
        state.disposal = Some(disposal.clone());
        (disposal, Some(settle))
    }

    fn await_shared(future: SharedClose) -> TerminalFuture<()> {
        Box::pin(async move { future.await.map_err(|error| Box::new(error) as BoxError) })
    }

    fn drain_shutdown_commands(
        commands: &mut mpsc::UnboundedReceiver<Command>,
        jobs: &mut FuturesUnordered<BoxFuture<'static, JobOutcome>>,
    ) -> Vec<oneshot::Sender<Result<(), SharedError>>> {
        let mut responders = vec![];
        while let Ok(command) = commands.try_recv() {
            match command {
                Command::Drive(job) => jobs.push(
                    async move {
                        job.await;
                        JobOutcome::Continue
                    }
                    .boxed(),
                ),
                // Global teardown covers every owner, so queued narrower teardown
                // requests share its settlement instead of racing a duplicate close.
                Command::DisposeOwner { result, .. } => responders.push(result),
                Command::DisposeAll { result } => responders.extend(result),
            }
        }
        responders
    }

    fn new(agents: Service<Agents>, commands: mpsc::UnboundedSender<Command>) -> Self {
        Self {
            inner: Arc::new(Inner {
                agents,
                state: Mutex::new(State::default()),
                commands,
            }),
        }
    }

    fn assert_active(&self) -> Result<(), BoxError> {
        if lock(&self.inner.state).disposing {
            Err(terminal_error(
                "PTY service is disposing",
                TerminalErrorCode::ServiceDisposing,
            ))
        } else {
            Ok(())
        }
    }

    fn owner_is_disposed(state: &mut State, owner: &Arc<dyn Agent>) -> bool {
        state
            .disposed_owners
            .retain(|candidate| candidate.strong_count() > 0);
        let owner = Arc::downgrade(owner);
        state
            .disposed_owners
            .iter()
            .any(|candidate| Weak::ptr_eq(candidate, &owner))
    }

    fn is_live_owner(&self, owner: &Arc<dyn Agent>) -> bool {
        let registered = self.inner.agents.get(owner.id());
        registered
            .as_ref()
            .is_some_and(|registered| Arc::ptr_eq(registered, owner))
            && !Self::owner_is_disposed(&mut lock(&self.inner.state), owner)
    }

    fn ensure_owner_cleanup(&self, owner: &Arc<dyn Agent>) -> Result<(), BoxError> {
        if !self.is_live_owner(owner) {
            return Err(terminal_error(
                format!("agent \"{}\" is not the registered PTY owner", owner.id()),
                TerminalErrorCode::OwnerNotLive,
            ));
        }
        let generation = Arc::new(());
        {
            let mut state = lock(&self.inner.state);
            if state.disposing {
                return Err(terminal_error(
                    "PTY service is disposing",
                    TerminalErrorCode::ServiceDisposing,
                ));
            }
            if state
                .owner_cleanups
                .iter()
                .any(|entry| Arc::ptr_eq(&entry.owner, owner))
            {
                return Ok(());
            }
            state.owner_cleanups.push(OwnerCleanup {
                owner: owner.clone(),
                generation: generation.clone(),
                disposer: None,
            });
        }

        let weak = Arc::downgrade(&self.inner);
        let cleanup_owner = owner.clone();
        let cleanup_generation = generation.clone();
        let disposer = match owner.ctx().effect("pty.ownerCleanup()", || {
            Undo::future(move || async move {
                if let Some(inner) = weak.upgrade() {
                    TerminalSessionService { inner }
                        .owner_cleanup_started(cleanup_owner, cleanup_generation)
                        .await?;
                }
                Ok(())
            })
        }) {
            Ok(disposer) => disposer,
            Err(error) => {
                lock(&self.inner.state)
                    .owner_cleanups
                    .retain(|entry| !Arc::ptr_eq(&entry.generation, &generation));
                return Err(Box::new(error));
            }
        };

        let mut disposer = Some(disposer);
        let installed = {
            let mut state = lock(&self.inner.state);
            if state.disposing || Self::owner_is_disposed(&mut state, owner) {
                false
            } else if let Some(entry) = state
                .owner_cleanups
                .iter_mut()
                .find(|entry| Arc::ptr_eq(&entry.generation, &generation))
            {
                entry.disposer = disposer.take();
                true
            } else {
                false
            }
        };
        if !installed {
            if let Some(disposer) = disposer {
                drop(disposer.dispose());
            }
            return Err(terminal_error(
                format!("agent \"{}\" is not the registered PTY owner", owner.id()),
                TerminalErrorCode::OwnerNotLive,
            ));
        }
        Ok(())
    }

    async fn owner_cleanup_started(
        &self,
        owner: Arc<dyn Agent>,
        generation: Arc<()>,
    ) -> Result<(), BoxError> {
        let global_cleanup = {
            let mut state = lock(&self.inner.state);
            state.disposed_owners.push(Arc::downgrade(&owner));
            state
                .owner_cleanups
                .retain(|entry| !Arc::ptr_eq(&entry.generation, &generation));
            let global_cleanup = state
                .global_owner_cleanups
                .iter()
                .position(|candidate| Arc::ptr_eq(candidate, &generation));
            if let Some(position) = global_cleanup {
                state.global_owner_cleanups.swap_remove(position);
                true
            } else {
                false
            }
        };
        if !global_cleanup {
            return self.dispose_owned(owner).await;
        }
        let outcome = self.dispose_owned_impl(owner).await;
        if let Err(error) = &outcome {
            lock(&self.inner.state)
                .global_owner_failures
                .push(error.clone());
        }
        outcome.map_err(|error| Box::new(error) as BoxError)
    }

    pub fn register_backend(
        &self,
        ctx: &Context,
        backend: Arc<dyn TerminalBackend>,
    ) -> Result<Disposer, BoxError> {
        self.assert_active()?;
        let kind = backend.kind().to_owned();
        if kind.is_empty() {
            return Err("pty backend type must be non-empty".into());
        }
        let service = self.clone();
        ctx.try_effect::<BoxError>("pty.registerBackend()", move |undos| {
            {
                let mut state = lock(&service.inner.state);
                if state.disposing {
                    return Err(terminal_error(
                        "PTY service is disposing",
                        TerminalErrorCode::ServiceDisposing,
                    ));
                }
                if state.backends.contains_key(&kind) {
                    return Err(terminal_error(
                        format!("a PTY backend named \"{kind}\" is already registered"),
                        TerminalErrorCode::DuplicateBackend,
                    ));
                }
                state.backends.insert(kind.clone(), backend.clone());
            }
            let undo_service = service.clone();
            undos.push(Undo::sync(move || {
                let mut state = lock(&undo_service.inner.state);
                Self::remove_backend_contribution(&mut state, &kind, &backend);
            }))?;
            Ok(())
        })
    }

    fn remove_backend_contribution(
        state: &mut State,
        kind: &str,
        backend: &Arc<dyn TerminalBackend>,
    ) {
        if state
            .backends
            .get(kind)
            .is_some_and(|current| Arc::ptr_eq(current, backend))
        {
            state.backends.shift_remove(kind);
        }
    }

    pub fn list_backends(&self) -> Vec<String> {
        lock(&self.inner.state).backends.keys().cloned().collect()
    }

    fn reserve_name(
        &self,
        owner: &Arc<dyn Agent>,
        name: Option<&str>,
    ) -> Result<Option<Arc<()>>, BoxError> {
        let Some(name) = name else {
            return Ok(None);
        };
        let mut state = lock(&self.inner.state);
        if state
            .sessions
            .values()
            .any(|record| Arc::ptr_eq(&record.owner, owner) && record.name.as_deref() == Some(name))
        {
            return Err(terminal_error(
                format!("PTY session name \"{name}\" already exists for this owner"),
                TerminalErrorCode::DuplicateName,
            ));
        }
        if state
            .reserved_names
            .iter()
            .any(|entry| Arc::ptr_eq(&entry.owner, owner) && entry.name == name)
        {
            return Err(terminal_error(
                format!("PTY session name \"{name}\" is already being created"),
                TerminalErrorCode::DuplicateName,
            ));
        }
        let token = Arc::new(());
        state.reserved_names.push(ReservedName {
            owner: owner.clone(),
            name: name.into(),
            token: token.clone(),
        });
        Ok(Some(token))
    }

    fn release_name(&self, token: Option<&Arc<()>>) {
        let Some(token) = token else { return };
        lock(&self.inner.state)
            .reserved_names
            .retain(|entry| !Arc::ptr_eq(&entry.token, token));
    }

    fn reserve_spawn(&self, owner: Arc<dyn Agent>) -> Arc<PendingSpawn> {
        let pending = PendingSpawn::new(owner);
        lock(&self.inner.state).pending_spawns.push(pending.clone());
        pending
    }

    fn release_spawn(&self, pending: &Arc<PendingSpawn>, cleanup_failure: Option<SharedError>) {
        if cleanup_failure.is_none() {
            lock(&self.inner.state)
                .pending_spawns
                .retain(|candidate| !Arc::ptr_eq(candidate, pending));
        }
        pending.finish(cleanup_failure);
    }

    pub fn spawn(
        &self,
        owner: Arc<dyn Agent>,
        request: TerminalSpawnRequest,
        signal: Option<AbortSignal>,
    ) -> TerminalFuture<TerminalSpawnResult> {
        if let Err(error) = self.assert_active() {
            return ready_error(error);
        }
        if let Some(reason) = signal.as_ref().and_then(AbortSignal::reason) {
            return ready_error(Box::new(reason));
        }
        if let Err(error) = self.ensure_owner_cleanup(&owner) {
            return ready_error(error);
        }
        let backend = {
            let state = lock(&self.inner.state);
            let Some(backend) = state.backends.get(&request.kind).cloned() else {
                return ready_error(terminal_error(
                    format!("no PTY backend registered for \"{}\"", request.kind),
                    TerminalErrorCode::NoBackend,
                ));
            };
            backend
        };
        if request.name.as_deref() == Some("") {
            return ready_error(boxed("PTY session name must be non-empty"));
        }
        let name_token = match self.reserve_name(&owner, request.name.as_deref()) {
            Ok(token) => token,
            Err(error) => return ready_error(error),
        };
        let pending = self.reserve_spawn(owner.clone());
        let id = {
            let mut state = lock(&self.inner.state);
            state.next_id = state.next_id.saturating_add(1);
            terminal_session_id(format!("pty-{}", state.next_id))
        };
        // Reserve an actor-owned driver before invoking backend code.  The state lock
        // orders this enqueue before either teardown path marks the service disposing,
        // so dropping the caller's returned future cannot strand a pending reservation.
        let (drive, receiver) = oneshot::channel::<BoxFuture<'static, ()>>();
        let driver = async move {
            if let Ok(job) = receiver.await {
                job.await;
            }
        }
        .boxed();
        let dispatched = {
            let state = lock(&self.inner.state);
            !state.disposing && self.inner.commands.send(Command::Drive(driver)).is_ok()
        };
        if !dispatched {
            self.release_spawn(&pending, None);
            self.release_name(name_token.as_ref());
            return ready_error(terminal_error(
                "PTY service is disposing",
                TerminalErrorCode::ServiceDisposing,
            ));
        }
        let backend_signal = match &signal {
            Some(signal) => AbortSignal::any([signal.clone(), pending.controller.signal()]),
            None => pending.controller.signal(),
        };
        let spawn = backend.spawn(TerminalBackendSpawnSpec {
            session_id: id.clone(),
            owner: owner.clone(),
            kind: request.kind.clone(),
            name: request.name.clone(),
            cwd: request.cwd.clone(),
            signal: Some(backend_signal),
        });
        let (result, receiver) = oneshot::channel();
        let failed_pending = pending.clone();
        let failed_name_token = name_token.clone();
        let service = self.clone();
        let job = async move {
            let outcome = service
                .drive_spawn(SpawnDrive {
                    owner,
                    request,
                    id,
                    caller_signal: signal,
                    pending,
                    name_token,
                    spawn,
                })
                .await;
            let _ = result.send(outcome);
        }
        .boxed();
        // The receiver is actor-owned; a failed send can only mean the actor has
        // already failed, in which case the caller receives the infrastructure error.
        if drive.send(job).is_err() {
            self.release_spawn(&failed_pending, None);
            self.release_name(failed_name_token.as_ref());
            return ready_error("PTY spawn driver stopped before backend setup could run");
        }
        Box::pin(async move {
            receiver
                .await
                .map_err(|_| boxed("PTY spawn driver stopped before returning a result"))?
                .map_err(|error| Box::new(error) as BoxError)
        })
    }

    async fn drive_spawn(&self, drive: SpawnDrive) -> Result<TerminalSpawnResult, SharedError> {
        let SpawnDrive {
            owner,
            request,
            id,
            caller_signal,
            pending,
            name_token,
            spawn,
        } = drive;
        let mut session: Option<Arc<dyn TerminalBackendSession>> = None;
        let mut cleanup_failure = None;
        let mut failure = match spawn.await {
            Ok(created) => {
                session = Some(created);
                None
            }
            Err(error) => {
                let error = SharedError::new(error);
                if let Some(detail) = TerminalBackendCleanupError::detail(error.get_ref()) {
                    cleanup_failure = Some(detail.cleanup_error.clone());
                }
                Some(error)
            }
        };

        if failure.is_none() {
            failure = caller_signal
                .as_ref()
                .and_then(AbortSignal::reason)
                .or_else(|| pending.controller.signal().reason());
        }
        if failure.is_none() && lock(&self.inner.state).disposing {
            failure = Some(SharedError::new(TerminalError::new(
                "PTY service is disposing",
                TerminalErrorCode::ServiceDisposing,
            )));
        }
        if failure.is_none() && !self.is_live_owner(&owner) {
            failure = Some(SharedError::new(TerminalError::new(
                "PTY owner is no longer live",
                TerminalErrorCode::OwnerNotLive,
            )));
        }

        let outcome = if let Some(mut failure) = failure {
            let mut rollback_failure = None;
            if let Some(session) = session
                && let Err(error) = session.close("PTY spawn rolled back".into()).await
            {
                let error = SharedError::new(error);
                cleanup_failure = Some(error.clone());
                rollback_failure = Some(error);
            }
            if let Some(cancellation) = caller_signal
                .as_ref()
                .and_then(AbortSignal::reason)
                .or_else(|| pending.controller.signal().reason())
            {
                failure = cancellation;
            }
            if let Some(rollback_failure) = rollback_failure
                && !caller_signal.as_ref().is_some_and(AbortSignal::aborted)
            {
                failure = SharedError::new(AggregateError::new(
                    "PTY spawn and rollback both failed",
                    vec![failure, rollback_failure],
                ));
            }
            Err(failure)
        } else {
            let Some(session) = session else {
                return Err(SharedError::new(
                    "PTY backend reported success without a session",
                ));
            };
            let record = Arc::new(SessionRecord {
                id: id.clone(),
                owner: owner.clone(),
                name: request.name.clone(),
                kind: request.kind,
                session: session.clone(),
                state: Mutex::new(RecordState::default()),
            });
            let live = self.inner.agents.get(owner.id());
            let rejection = {
                let mut state = lock(&self.inner.state);
                if state.disposing {
                    Some(TerminalError::new(
                        "PTY service is disposing",
                        TerminalErrorCode::ServiceDisposing,
                    ))
                } else if live.as_ref().is_none_or(|live| !Arc::ptr_eq(live, &owner))
                    || Self::owner_is_disposed(&mut state, &owner)
                {
                    Some(TerminalError::new(
                        "PTY owner is no longer live",
                        TerminalErrorCode::OwnerNotLive,
                    ))
                } else {
                    state.sessions.insert(id, record.clone());
                    None
                }
            };
            if let Some(rejection) = rejection {
                self.rollback_publication(session, rejection, &mut cleanup_failure)
                    .await
            } else {
                Ok(self.spawn_snapshot(&record))
            }
        };
        self.release_spawn(&pending, cleanup_failure);
        self.release_name(name_token.as_ref());
        outcome
    }

    async fn rollback_publication(
        &self,
        session: Arc<dyn TerminalBackendSession>,
        failure: impl Into<BoxError>,
        cleanup_failure: &mut Option<SharedError>,
    ) -> Result<TerminalSpawnResult, SharedError> {
        let failure = SharedError::new(failure);
        match session.close("PTY spawn rolled back".into()).await {
            Ok(()) => Err(failure),
            Err(error) => {
                let error = SharedError::new(error);
                *cleanup_failure = Some(error.clone());
                Err(SharedError::new(AggregateError::new(
                    "PTY spawn and rollback both failed",
                    vec![failure, error],
                )))
            }
        }
    }

    pub fn has_owner_activity(&self, owner: &Arc<dyn Agent>) -> bool {
        let state = lock(&self.inner.state);
        state
            .pending_spawns
            .iter()
            .any(|pending| Arc::ptr_eq(&pending.owner, owner))
            || state
                .sessions
                .values()
                .any(|record| Arc::ptr_eq(&record.owner, owner))
    }

    fn expect_owned(
        &self,
        owner: &Arc<dyn Agent>,
        id: &TerminalSessionId,
    ) -> Result<Arc<SessionRecord>, BoxError> {
        let state = lock(&self.inner.state);
        let Some(record) = state.sessions.get(id).cloned() else {
            return Err(terminal_error(
                format!("unknown PTY session {id}"),
                TerminalErrorCode::NoSession,
            ));
        };
        if !Arc::ptr_eq(&record.owner, owner) {
            return Err(terminal_error(
                format!("PTY session {id} belongs to another agent"),
                TerminalErrorCode::ForeignSession,
            ));
        }
        Ok(record)
    }

    pub fn start_send(
        &self,
        owner: &Arc<dyn Agent>,
        id: &TerminalSessionId,
        request: TerminalSendRequest,
    ) -> Result<TerminalSendOperation, BoxError> {
        let record = self.expect_owned(owner, id)?;
        let (drive_send, drive_receive) = oneshot::channel::<BoxFuture<'static, ()>>();
        let drive_slot = async move {
            if let Ok(driver) = drive_receive.await {
                driver.await;
            }
        }
        .boxed();
        let dispatched = {
            let state = lock(&self.inner.state);
            !state.disposing && self.inner.commands.send(Command::Drive(drive_slot)).is_ok()
        };
        if !dispatched {
            return Err(terminal_error(
                "PTY service is disposing",
                TerminalErrorCode::ServiceDisposing,
            ));
        }
        let token = Arc::new(());
        let (settle_start, start_settled) = oneshot::channel();
        let start_settled = async move {
            let _ = start_settled.await;
        }
        .boxed()
        .shared();
        {
            let mut state = lock(&record.state);
            if state.closing.is_some() {
                return Err(terminal_error(
                    format!("PTY session {id} is closing"),
                    TerminalErrorCode::SessionClosing,
                ));
            }
            if state.active.is_some() || state.starting.is_some() {
                return Err(terminal_error(
                    format!("PTY session {id} already has an active send"),
                    TerminalErrorCode::SendActive,
                ));
            }
            state.starting = Some(Starting {
                token: token.clone(),
                settled: start_settled,
            });
        }
        let backend = match record.session.start_send(request) {
            Ok(operation) => operation,
            Err(error) => {
                self.finish_start(&record, &token, None);
                let _ = settle_start.send(());
                return Err(error);
            }
        };
        self.finish_start(&record, &token, Some(token.clone()));
        let _ = settle_start.send(());
        let backend_done = backend.shared_done();
        let clear_service = self.clone();
        let clear_record = record.clone();
        let clear_token = token.clone();
        let done = async move {
            let result = backend_done.await;
            clear_service.clear_active(&clear_record, &clear_token);
            result
        }
        .boxed()
        .shared();
        let operation = backend.with_done(done.clone());
        let drive = async move {
            let _ = done.await;
        }
        .boxed();
        if drive_send.send(drive).is_err() {
            operation.cancel();
            self.clear_active(&record, &token);
            return Err(terminal_error(
                "PTY service is disposing",
                TerminalErrorCode::ServiceDisposing,
            ));
        }
        Ok(operation)
    }

    fn finish_start(&self, record: &Arc<SessionRecord>, token: &Arc<()>, active: Option<Arc<()>>) {
        let mut state = lock(&record.state);
        if state
            .starting
            .as_ref()
            .is_some_and(|starting| Arc::ptr_eq(&starting.token, token))
        {
            state.starting = None;
            state.active = active;
        }
    }

    fn clear_active(&self, record: &Arc<SessionRecord>, token: &Arc<()>) {
        let mut state = lock(&record.state);
        if state
            .active
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, token))
        {
            state.active = None;
        }
    }

    pub fn read(
        &self,
        owner: &Arc<dyn Agent>,
        id: &TerminalSessionId,
        request: TerminalReadRequest,
    ) -> Result<TerminalReadResult, BoxError> {
        self.expect_owned(owner, id)?.session.read(request)
    }

    pub fn signal(
        &self,
        owner: &Arc<dyn Agent>,
        id: &TerminalSessionId,
        signal: TerminalSignal,
    ) -> TerminalFuture<TerminalSignalResult> {
        match self.expect_owned(owner, id) {
            Ok(record) => record.session.signal(signal),
            Err(error) => ready_error(error),
        }
    }

    fn begin_close(&self, record: &Arc<SessionRecord>, reason: String) -> CloseWait {
        let (settle, settled) = oneshot::channel();
        let future = async move {
            settled
                .await
                .unwrap_or_else(|_| Err(SharedError::new("PTY close driver stopped")))
        }
        .boxed()
        .shared();
        let token = Arc::new(());
        let starting = {
            let mut state = lock(&record.state);
            if let Some(closing) = &state.closing {
                return CloseWait {
                    started: false,
                    future: closing.future.clone(),
                    local_driver: None,
                };
            }
            state.closing = Some(Closing {
                token: token.clone(),
                future: future.clone(),
            });
            state
                .starting
                .as_ref()
                .map(|starting| starting.settled.clone())
        };
        let close = if let Some(starting) = starting {
            let session = record.session.clone();
            async move {
                starting.await;
                session.close(reason).await
            }
            .boxed()
        } else {
            record.session.close(reason)
        };
        let close_service = self.clone();
        let close_record = record.clone();
        let close_token = token.clone();
        let driver = async move {
            let outcome = close.await.map_err(SharedError::new);
            close_service.complete_close(&close_record, &close_token, &outcome);
            let _ = settle.send(outcome);
        }
        .boxed();
        let local_driver = match self.inner.commands.send(Command::Drive(driver)) {
            Ok(()) => None,
            Err(error) => match error.0 {
                Command::Drive(driver) => Some(driver),
                _ => None,
            },
        };
        CloseWait {
            started: true,
            future,
            local_driver,
        }
    }

    fn complete_close(
        &self,
        record: &Arc<SessionRecord>,
        token: &Arc<()>,
        outcome: &Result<(), SharedError>,
    ) {
        if outcome.is_ok() {
            let mut state = lock(&self.inner.state);
            if state
                .sessions
                .get(&record.id)
                .is_some_and(|current| Arc::ptr_eq(current, record))
            {
                state.sessions.shift_remove(&record.id);
            }
            return;
        }
        let mut state = lock(&record.state);
        if state
            .closing
            .as_ref()
            .is_some_and(|closing| Arc::ptr_eq(&closing.token, token))
        {
            state.closing = None;
        }
    }

    async fn finish_close(&self, mut wait: CloseWait) -> Result<bool, SharedError> {
        if let Some(driver) = wait.local_driver.take() {
            driver.await;
        }
        wait.future.await.map(|()| wait.started)
    }

    pub fn kill(
        &self,
        owner: &Arc<dyn Agent>,
        id: &TerminalSessionId,
        reason: Option<&str>,
    ) -> TerminalFuture<bool> {
        let record = match self.expect_owned(owner, id) {
            Ok(record) => record,
            Err(error) => return ready_error(error),
        };
        let wait = self.begin_close(&record, reason.unwrap_or("model request").into());
        let service = self.clone();
        Box::pin(async move {
            service
                .finish_close(wait)
                .await
                .map_err(|error| Box::new(error) as BoxError)
        })
    }

    fn snapshot(&self, record: &SessionRecord) -> TerminalSessionSnapshot {
        TerminalSessionSnapshot {
            session_id: record.id.clone(),
            name: record.name.clone(),
            kind: record.kind.clone(),
            pid: record.session.pid(),
            status: record.session.status(),
        }
    }

    fn spawn_snapshot(&self, record: &SessionRecord) -> TerminalSpawnResult {
        TerminalSpawnResult {
            session_id: record.id.clone(),
            name: record.name.clone(),
            kind: record.kind.clone(),
            pid: record.session.pid(),
            status: record.session.status(),
            motd: record.session.motd().into(),
        }
    }

    pub fn list(&self, owner: &Arc<dyn Agent>) -> Vec<TerminalSessionSnapshot> {
        let records: Vec<_> = lock(&self.inner.state)
            .sessions
            .values()
            .filter(|record| Arc::ptr_eq(&record.owner, owner))
            .cloned()
            .collect();
        records.iter().map(|record| self.snapshot(record)).collect()
    }

    async fn abort_pending_spawns(
        &self,
        owner: Option<&Arc<dyn Agent>>,
        abort_reason: HarnessError,
    ) -> Result<(), SharedError> {
        let pending: Vec<_> = lock(&self.inner.state)
            .pending_spawns
            .iter()
            .filter(|pending| owner.is_none_or(|owner| Arc::ptr_eq(&pending.owner, owner)))
            .cloned()
            .collect();
        for spawn in &pending {
            spawn.controller.abort_with(abort_reason.clone());
        }
        join_all(pending.iter().map(|pending| pending.settled.clone())).await;
        let failures: Vec<_> = pending
            .iter()
            .filter_map(|pending| lock(&pending.cleanup_failure).clone())
            .collect();
        lock(&self.inner.state)
            .pending_spawns
            .retain(|candidate| !pending.iter().any(|item| Arc::ptr_eq(item, candidate)));
        if failures.is_empty() {
            Ok(())
        } else {
            Err(SharedError::new(AggregateError::new(
                "failed to roll back unpublished PTY setup",
                failures,
            )))
        }
    }

    async fn close_records(
        &self,
        records: Vec<Arc<SessionRecord>>,
        reason: &str,
    ) -> Result<(), SharedError> {
        let waits: Vec<_> = records
            .iter()
            .map(|record| self.begin_close(record, reason.into()))
            .collect();
        let results = join_all(
            records
                .into_iter()
                .zip(waits)
                .map(|(_, wait)| self.finish_close(wait)),
        )
        .await;
        let failures: Vec<_> = results.into_iter().filter_map(Result::err).collect();
        if failures.is_empty() {
            Ok(())
        } else {
            Err(SharedError::new(AggregateError::new(
                format!("failed to close {} PTY session(s)", failures.len()),
                failures,
            )))
        }
    }

    async fn abort_and_close(
        &self,
        owner: Option<&Arc<dyn Agent>>,
        abort_reason: HarnessError,
        close_reason: &str,
    ) -> Result<(), SharedError> {
        let mut failures = vec![];
        if let Err(error) = self.abort_pending_spawns(owner, abort_reason).await {
            failures.push(error);
        }
        let records: Vec<_> = lock(&self.inner.state)
            .sessions
            .values()
            .filter(|record| owner.is_none_or(|owner| Arc::ptr_eq(&record.owner, owner)))
            .cloned()
            .collect();
        if let Err(error) = self.close_records(records, close_reason).await {
            failures.push(error);
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(SharedError::new(AggregateError::new(
                "failed to clean up PTY lifecycle",
                failures,
            )))
        }
    }

    async fn dispose_owned_impl(&self, owner: Arc<dyn Agent>) -> Result<(), SharedError> {
        let result = self
            .abort_and_close(
                Some(&owner),
                TerminalError::new(
                    "PTY owner is no longer live",
                    TerminalErrorCode::OwnerNotLive,
                ),
                "PTY owner disposed",
            )
            .await;
        lock(&self.inner.state)
            .reserved_names
            .retain(|entry| !Arc::ptr_eq(&entry.owner, &owner));
        result
    }

    pub fn dispose_owned(&self, owner: Arc<dyn Agent>) -> TerminalFuture<()> {
        let global_disposal = {
            let mut state = lock(&self.inner.state);
            state.disposed_owners.push(Arc::downgrade(&owner));
            state
                .disposal
                .as_ref()
                .map(|disposal| disposal.future.clone())
        };
        if let Some(disposal) = global_disposal {
            return Self::await_shared(disposal);
        }
        let (result, receiver) = oneshot::channel();
        if self
            .inner
            .commands
            .send(Command::DisposeOwner { owner, result })
            .is_err()
        {
            return ready_error("PTY owner cleanup driver stopped");
        }
        Box::pin(async move {
            receiver
                .await
                .map_err(|_| boxed("PTY owner cleanup driver stopped"))?
                .map_err(|error| Box::new(error) as BoxError)
        })
    }

    async fn dispose_all_impl(&self) -> Result<(), SharedError> {
        let result = self
            .abort_and_close(
                None,
                TerminalError::new(
                    "PTY service is disposing",
                    TerminalErrorCode::ServiceDisposing,
                ),
                "PTY service disposed",
            )
            .await;
        let cleanups = {
            let mut state = lock(&self.inner.state);
            state.backends.clear();
            state.reserved_names.clear();
            state.pending_spawns.clear();
            let entries = state.owner_cleanups.drain(..).collect::<Vec<_>>();
            let mut cleanups = Vec::with_capacity(entries.len());
            for entry in entries {
                if let Some(disposer) = entry.disposer {
                    state.global_owner_cleanups.push(entry.generation);
                    cleanups.push(disposer);
                }
            }
            cleanups
        };
        join_all(cleanups.into_iter().map(Disposer::dispose)).await;
        let cleanup_failure = lock(&self.inner.state)
            .global_owner_failures
            .drain(..)
            .next();
        cleanup_failure.map_or(result, Err)
    }

    pub fn dispose_all(&self) -> TerminalFuture<()> {
        let disposal = {
            let mut state = lock(&self.inner.state);
            if let Some(disposal) = &state.disposal {
                return Self::await_shared(disposal.future.clone());
            }
            let (disposal, settle) = Self::install_disposal_fence(&mut state);
            let command = Command::DisposeAll { result: settle };
            if let Err(error) = self.inner.commands.send(command)
                && let Command::DisposeAll {
                    result: Some(settle),
                } = error.0
            {
                let _ = settle.send(Err(SharedError::new("PTY service cleanup driver stopped")));
            }
            disposal
        };
        Self::await_shared(disposal.future)
    }

    async fn run_actor(
        self,
        mut commands: mpsc::UnboundedReceiver<Command>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<(), BoxError> {
        let mut jobs: FuturesUnordered<BoxFuture<'static, JobOutcome>> = FuturesUnordered::new();
        let mut stopping = false;
        loop {
            tokio::select! {
                _ = cancel.cancelled(), if !stopping => {
                    stopping = true;
                    let settle = {
                        let mut state = lock(&self.inner.state);
                        let (_, settle) = Self::install_disposal_fence(&mut state);
                        commands.close();
                        settle
                    };
                    let mut responders = Self::drain_shutdown_commands(&mut commands, &mut jobs);
                    responders.extend(settle);
                    let service = self.clone();
                    jobs.push(async move {
                        let outcome = service.dispose_all_impl().await;
                        for responder in responders {
                            let _ = responder.send(outcome.clone());
                        }
                        JobOutcome::Stop(outcome)
                    }.boxed());
                }
                command = commands.recv(), if !stopping => {
                    let Some(command) = command else {
                        return Ok(());
                    };
                    match command {
                        Command::Drive(job) => jobs.push(async move {
                            job.await;
                            JobOutcome::Continue
                        }.boxed()),
                        Command::DisposeOwner { owner, result } => {
                            let service = self.clone();
                            jobs.push(async move {
                                let outcome = service.dispose_owned_impl(owner).await;
                                let _ = result.send(outcome);
                                JobOutcome::Continue
                            }.boxed());
                        }
                        Command::DisposeAll { result } => {
                            stopping = true;
                            {
                                let mut state = lock(&self.inner.state);
                                state.disposing = true;
                                commands.close();
                            }
                            let mut responders = Self::drain_shutdown_commands(&mut commands, &mut jobs);
                            responders.extend(result);
                            let service = self.clone();
                            jobs.push(async move {
                                let outcome = service.dispose_all_impl().await;
                                for responder in responders {
                                    let _ = responder.send(outcome.clone());
                                }
                                JobOutcome::Stop(outcome)
                            }.boxed());
                        }
                    }
                }
                outcome = jobs.next(), if !jobs.is_empty() => {
                    match outcome {
                        Some(JobOutcome::Continue) => {}
                        Some(JobOutcome::Stop(outcome)) => {
                            return outcome.map_err(|error| Box::new(error) as BoxError);
                        }
                        None => {}
                    }
                }
            }
        }
    }
}

struct CloseWait {
    started: bool,
    future: SharedClose,
    local_driver: Option<BoxFuture<'static, ()>>,
}

#[derive(Inject)]
pub struct TerminalSessionDeps {
    pub agents: Service<Agents>,
}

impl Plugin for TerminalSessionService {
    const NAME: &'static str = "TerminalSessionService";
    type Config = NoConfig;
    type Deps = TerminalSessionDeps;

    async fn apply(
        ctx: Context,
        deps: Self::Deps,
        _config: hoh_kernel::Cfg<Self::Config>,
    ) -> Result<(), BoxError> {
        let (commands, receiver) = mpsc::unbounded_channel();
        let service = TerminalSessionService::new(deps.agents, commands);
        let actor = service.clone();
        ctx.spawn("pty lifecycle", move |cancel| {
            actor.run_actor(receiver, cancel)
        })?;
        ctx.provide::<Terminals>(Arc::new(service)).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Backend(&'static str);

    impl TerminalBackend for Backend {
        fn kind(&self) -> &str {
            self.0
        }

        fn spawn(
            &self,
            _: TerminalBackendSpawnSpec,
        ) -> TerminalFuture<Arc<dyn TerminalBackendSession>> {
            ready_error("unused backend")
        }
    }

    #[test]
    fn stale_backend_disposer_does_not_remove_replacement_at_same_key() {
        let original: Arc<dyn TerminalBackend> = Arc::new(Backend("stub"));
        let replacement: Arc<dyn TerminalBackend> = Arc::new(Backend("stub"));
        let mut state = State::default();
        state.backends.insert("stub".into(), replacement.clone());

        TerminalSessionService::remove_backend_contribution(&mut state, "stub", &original);

        assert!(
            state
                .backends
                .get("stub")
                .is_some_and(|current| Arc::ptr_eq(current, &replacement))
        );
    }
}
