//! The session executor of one registered harness (`hoh_agent::SessionExecutor`): creates and
//! resumes sessions driven by it, owns their persistence and lifecycle.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use futures_util::future::BoxFuture;
use hoh_agent::{
    Agent, AgentDetach, AgentFactory, AgentHandle, AgentOptions, AgentSessionMeta, AgentSetup,
    Agents, CreateAgentOptions, ResumeAgentOptions, SessionExecutor, SessionStartSource,
    bound_executor as bound_harness, stored_executor_binding,
};
use hoh_destructive_classifier::DestructiveClassifier;
use hoh_error::BoxError;
use hoh_kernel::{Context, Disposer, Service, Undo};
use hoh_scope::{CreateScopeOptions, Scope, ScopeKey, create_scope};
use hoh_session::{
    CreateSessionOptions, PrepareSessionOptions, RestoredSessionOptions, Session, SessionDetach,
    SessionEvent, SessionHeader, SessionId, SessionLogOffset, Sessions, append_plugin_record,
    interrupted_turn_closers,
};
use hoh_session_persistence::{
    SessionAccess, SessionHandle, SessionPersistenceCreateOptions, SessionPersistenceService,
};
use hoh_timeout::AbortSignal;
use hoh_user_approval::Approval;
use hoh_user_questions::UserQuestions;
use hoh_working_directory::WorkingDirectoryKey;
use serde_json::{Map, Value, json};
use tokio::sync::OnceCell;

use crate::agent::{ExternalAgent, NewAgent};
use crate::contract::{AccessMode, NativeSessionId};
use crate::harness::{ExternalHarness, HarnessSession};
use crate::{Choices, Config};

/// The plugin record that binds a session to its harness. The first one is written at creation
/// (seq 0, `nativeSessionId: null`); every later one records a new native id or model. Each names
/// the conversation's model (`model`), so a resumed conversation keeps it. Declared by
/// `hoh-agent`, whose registry refuses to continue such a session on the loop.
pub use hoh_agent::EXECUTOR_BINDING_RECORD as BINDING_RECORD;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What every session of one harness shares.
pub(crate) struct Shared {
    /// The context of the `ExternalHarness` plugin: sessions live at most as long as it.
    pub ctx: Context,
    pub name: String,
    pub harness: Arc<dyn ExternalHarness>,
    pub agents: Service<Agents>,
    pub sessions: Service<Sessions>,
    pub approval: Service<Approval>,
    pub questions: Service<UserQuestions>,
    pub classifier: Service<DestructiveClassifier>,
    pub config: Arc<Config>,
    /// The user's choices in the window (the access mode).
    pub choices: Arc<Choices>,
    /// How each conversation's native session was started, while it runs; the one started last
    /// is the last entry. One entry per conversation: another chat's start does not replace it.
    pub launched: Mutex<Vec<(String, Launched)>>,
}

/// The access mode and command-line switches a native session was started with.
pub(crate) struct Launched {
    pub access: AccessMode,
    pub flags: Vec<String>,
    pub session: Weak<dyn HarnessSession>,
}

impl Shared {
    /// The access mode the user chose for this harness.
    pub(crate) fn access(&self) -> AccessMode {
        self.choices.access(&self.name)
    }

    /// The mode and switches of the running native session started last, if it still runs.
    pub(crate) fn running_launch(&self) -> Option<(AccessMode, Vec<String>)> {
        lock(&self.launched)
            .iter()
            .rev()
            .map(|(_, launched)| launched)
            .find(|launched| launched.session.strong_count() > 0)
            .map(|launched| (launched.access, launched.flags.clone()))
    }

    /// The mode and switches of the native session of conversation `session`, if it still runs.
    pub(crate) fn running_launch_of(&self, session: &str) -> Option<(AccessMode, Vec<String>)> {
        lock(&self.launched)
            .iter()
            .find(|(id, launched)| id == session && launched.session.strong_count() > 0)
            .map(|(_, launched)| (launched.access, launched.flags.clone()))
    }

    /// Conversation `session` has worked in its folder: its native session is recorded, or its
    /// process runs. Until then (a new chat) its folder may still change.
    pub(crate) fn started_in_folder(&self, session: &Session) -> bool {
        recorded_native(&session.snapshot_events(None, None)).is_some()
            || self.running_launch_of(session.id().as_ref()).is_some()
    }

    /// The grace of a stop: a graceful end, then SIGTERM, then a kill.
    pub(crate) fn terminate_grace(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.config.terminate_grace_ms)
    }

    /// Take out the live native sessions started in another mode than `mode` whose conversation
    /// `idle` says is between turns, and forget their launch (they are about to end: nothing runs
    /// in the old mode any more, so the conversation shows no live process).
    pub(crate) fn retire_idle(
        &self,
        mode: AccessMode,
        idle: impl Fn(&str) -> bool,
    ) -> Vec<Arc<dyn HarnessSession>> {
        let mut all = lock(&self.launched);
        let mut ended = Vec::new();
        all.retain(|(id, launched)| {
            let Some(session) = launched.session.upgrade() else {
                return false;
            };
            if launched.access == mode || !idle(id) {
                return true;
            }
            ended.push(session);
            false
        });
        ended
    }

    /// A native session of conversation `session` started: it replaces that conversation's
    /// earlier entry (and any entry whose process is gone), and is the one started last.
    pub(crate) fn record_launch(&self, session: &str, launched: Launched) {
        let mut all = lock(&self.launched);
        all.retain(|(id, old)| id != session && old.session.strong_count() > 0);
        all.push((session.to_owned(), launched));
    }
}

#[derive(Clone)]
pub(crate) struct ExternalExecutor(pub Arc<Shared>);

impl AgentFactory for ExternalExecutor {
    fn create_agent(
        &self,
        owner_ctx: Context,
        options: CreateAgentOptions,
    ) -> BoxFuture<'static, Result<AgentHandle, BoxError>> {
        Box::pin(create(self.0.clone(), owner_ctx, options))
    }

    fn resume(
        &self,
        owner_ctx: Context,
        options: ResumeAgentOptions,
    ) -> BoxFuture<'static, Result<AgentHandle, BoxError>> {
        Box::pin(resume(self.0.clone(), owner_ctx, options))
    }
}

impl SessionExecutor for ExternalExecutor {
    /// Reads the first stored events for the binding record of a session this harness created.
    /// A log that cannot be read is an error, so the resume is refused rather than handed to the
    /// loop (fail closed).
    fn claims(
        &self,
        _owner_ctx: &Context,
        id: &SessionId,
    ) -> BoxFuture<'static, Result<bool, BoxError>> {
        let shared = self.0.clone();
        let id = id.clone();
        Box::pin(async move {
            let bound = stored_executor_binding(&shared.ctx, &id).await?;
            Ok(bound.is_some_and(|harness| harness == shared.name))
        })
    }
}

/// The last native id the log records.
fn recorded_native(events: &[Arc<SessionEvent>]) -> Option<NativeSessionId> {
    events
        .iter()
        .rev()
        .find(|event| event.event_type() == BINDING_RECORD)
        .and_then(|event| event.data().get("nativeSessionId").and_then(Value::as_str))
        .map(NativeSessionId::from)
}

/// The conversation's model the log records: that of the last binding record naming one.
fn recorded_model(events: &[Arc<SessionEvent>]) -> Option<String> {
    events
        .iter()
        .rev()
        .filter(|event| event.event_type() == BINDING_RECORD)
        .find_map(|event| event.data().get("model").and_then(Value::as_str))
        .map(str::to_owned)
}

/// Header metadata of a created session (the agent loop's `session_meta`).
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

/// A session's write handle and how many of its events are stored through it.
struct Stored {
    handle: Arc<dyn SessionHandle>,
    count: AtomicUsize,
}

/// Store the events appended since the last stored one (before the session is entered, the
/// persistence backend does not see its events).
async fn store_suffix(stored: Option<&Stored>, session: &Session) -> Result<(), BoxError> {
    let Some(stored) = stored else {
        return Ok(());
    };
    let count = stored.count.load(Ordering::Acquire);
    let suffix = session.snapshot_events(Some(SessionLogOffset::new(count as u64)?), None);
    if !suffix.is_empty() {
        stored.handle.append(&suffix, None).await?;
    }
    stored.count.store(count + suffix.len(), Ordering::Release);
    Ok(())
}

async fn create(
    shared: Arc<Shared>,
    owner: Context,
    options: CreateAgentOptions,
) -> Result<AgentHandle, BoxError> {
    // No id: the store gives a new one, as for the agent loop (`prepare(None)`).
    let id = options.session_id.clone();
    if options.seed.is_some() {
        return Err(format!(
            "external harness \"{}\" cannot start a session from a seed",
            shared.name
        )
        .into());
    }
    let session = shared.sessions.prepare(
        id,
        PrepareSessionOptions::Create(CreateSessionOptions {
            seed: None,
            inherited_event_count: None,
            meta: options.meta.as_ref().map(session_meta),
        }),
    )?;
    let stored = match shared.ctx.get::<SessionPersistenceService>() {
        None => None,
        Some(persistence) => {
            let header: SessionHeader = serde_json::from_value((*session.header()).clone())?;
            let create = SessionPersistenceCreateOptions {
                signal: None,
                inherited_event_count: Some(session.inherited_event_count()),
            };
            let handle = persistence.create(&header, Some(&create)).await?;
            Some(Stored {
                handle,
                count: AtomicUsize::new(0),
            })
        }
    };
    initialize(
        shared,
        owner,
        Init {
            session,
            stored,
            options: options.agent_options.unwrap_or_default(),
            native: None,
            bind: true,
            source: SessionStartSource::Startup,
            setup: options.setup,
            signal: options.signal,
        },
    )
    .await
}

async fn resume(
    shared: Arc<Shared>,
    owner: Context,
    options: ResumeAgentOptions,
) -> Result<AgentHandle, BoxError> {
    let persistence = shared.ctx.get::<SessionPersistenceService>().ok_or(
        "cannot resume: session persistence is not configured (load a hoh-session-persistence backend)",
    )?;
    let id = options.resume_session_id.clone();
    let handle = persistence.open(&id, SessionAccess::Write, None).await?;
    let restored = restore(&shared, &handle, &id).await;
    let (session, count, native) = match restored {
        Ok(restored) => restored,
        Err(error) => {
            let _ = handle.close().await;
            return Err(error);
        }
    };
    initialize(
        shared,
        owner,
        Init {
            session,
            stored: Some(Stored {
                handle,
                count: AtomicUsize::new(count),
            }),
            options: options.agent_options.unwrap_or_default(),
            native,
            bind: false,
            source: SessionStartSource::Resume,
            setup: options.setup,
            signal: options.signal,
        },
    )
    .await
}

/// Read a stored session, close a turn a crash left open, and prepare it.
async fn restore(
    shared: &Shared,
    handle: &Arc<dyn SessionHandle>,
    id: &SessionId,
) -> Result<(Session, usize, Option<NativeSessionId>), BoxError> {
    let read = handle.read(Some(0), None, None).await?;
    let persisted = read.events;
    match bound_harness(&persisted) {
        Some(harness) if harness == shared.name => {}
        Some(other) => {
            return Err(format!(
                "session \"{id}\" belongs to external harness \"{other}\", not \"{}\"",
                shared.name
            )
            .into());
        }
        None => {
            return Err(format!(
                "session \"{id}\" was not created by external harness \"{}\"",
                shared.name
            )
            .into());
        }
    }
    // Steps are advertised in `assistant/message` as the loop's are, so the store's own repair
    // closes a turn a crash left open, its steps included.
    let added = interrupted_turn_closers(&persisted)?;
    if !added.is_empty() {
        handle.append(&added, None).await?;
    }
    let mut seed = persisted;
    seed.extend(added);
    let native = recorded_native(&seed);
    let count = seed.len();
    let session = shared.sessions.prepare(
        Some(id.clone()),
        PrepareSessionOptions::Restored(RestoredSessionOptions {
            seed,
            meta: serde_json::to_value(handle.header())?,
            inherited_event_count: handle.inherited_event_count(),
            event_state: read.event_state,
        }),
    )?;
    Ok((session, count, native))
}

struct Init {
    session: Session,
    stored: Option<Stored>,
    options: AgentOptions,
    native: Option<NativeSessionId>,
    /// A new session: an early event binds it to this harness (written once the session is
    /// announced: the store accepts no appends before).
    bind: bool,
    source: SessionStartSource,
    setup: Option<AgentSetup>,
    signal: Option<AbortSignal>,
}

/// The folder the harness works in: the working-directory service's, else the session header's,
/// else the process's.
pub(crate) fn session_cwd(shared: &Shared, session: &Session) -> String {
    if let Some(working_directory) = shared.ctx.get::<WorkingDirectoryKey>() {
        return working_directory.get(session);
    }
    if let Some(cwd) = session.header().get("cwd").and_then(Value::as_str) {
        return cwd.to_owned();
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .to_string_lossy()
        .into_owned()
}

async fn initialize(
    shared: Arc<Shared>,
    owner: Context,
    init: Init,
) -> Result<AgentHandle, BoxError> {
    let Init {
        session,
        stored,
        options,
        native,
        bind,
        source,
        setup,
        signal,
    } = init;
    let prepared = (|| -> Result<(Scope, ScopeKey, u64), BoxError> {
        let last_turn = shared.agents.turn_boundary_of(&session)?.last_turn;
        let scope_key = ScopeKey::new();
        let scope = create_scope(
            &shared.ctx,
            scope_key.clone(),
            CreateScopeOptions::default(),
        )?;
        Ok((scope, scope_key, last_turn))
    })();
    let (scope, scope_key, last_turn) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            if let Some(stored) = &stored {
                let _ = stored.handle.close().await;
            }
            return Err(error);
        }
    };
    // The conversation's own model: the route it is given when that names this harness (a new
    // chat), else the one its log records (a resume is given the engine's route, not Claude's).
    let model = match options.provider() {
        Some(provider) if provider == shared.name => options.model(),
        _ => recorded_model(&session.snapshot_events(None, None)),
    };
    let agent = ExternalAgent::new(NewAgent {
        shared: shared.clone(),
        session: session.clone(),
        ctx: scope.ctx.clone(),
        scope_key,
        options,
        model: model.clone(),
        last_turn,
        native,
    });
    let lifecycle = Arc::new(Lifecycle {
        agent: agent.clone(),
        scope,
        stored,
        detach_session: Mutex::new(None),
        detach_agent: Mutex::new(None),
        driver: Mutex::new(None),
        follows: Mutex::new(Vec::new()),
        done: OnceCell::new(),
    });
    let dyn_agent: Arc<dyn Agent> = agent.clone();
    let published: Result<(), BoxError> = async {
        store_suffix(lifecycle.stored.as_ref(), &session).await?;
        let detach = shared.sessions.enter(dyn_agent.ctx(), session.clone())?;
        *lock(&lifecycle.detach_session) = Some(detach);
        shared.sessions.announce(&session)?;
        if bind {
            let mut binding = json!({"harness": shared.name, "nativeSessionId": null});
            if let Some(model) = &model {
                binding["model"] = json!(model);
            }
            append_plugin_record(&session, BINDING_RECORD, binding)?;
        }
        if let Some(setup) = &setup
            && let Some(commit) = setup(dyn_agent.ctx().clone(), dyn_agent.clone()).await?
        {
            commit()?;
        }
        let detach = shared.agents.enter(&shared.ctx, dyn_agent.clone())?;
        *lock(&lifecycle.detach_agent) = Some(detach);
        shared
            .agents
            .announce(&shared.ctx, &dyn_agent, source, signal)
            .await?;
        let driver = agent.clone();
        let task = dyn_agent.ctx().spawn(
            format!("externalHarness.driver({})", dyn_agent.id()),
            move |token| async move {
                driver.drive(token).await;
                Ok(())
            },
        )?;
        *lock(&lifecycle.driver) = Some(task);
        // The session ends with its owner and with the harness plugin; whichever ends first
        // also leaves the other's effect, which would otherwise hold the session until the
        // plugin unloads (review 221315, item 2).
        for ctx in [&owner, &shared.ctx] {
            let follow = lifecycle.clone();
            let disposer = ctx.try_effect(
                format!("externalHarness.lifecycle({})", dyn_agent.id()),
                move |undos| -> Result<(), BoxError> {
                    undos.push(Undo::future(move || async move {
                        follow.teardown().await;
                        follow.leave().await;
                        Ok(())
                    }))?;
                    Ok(())
                },
            )?;
            lock(&lifecycle.follows).push(disposer);
        }
        Ok(())
    }
    .await;
    match published {
        Ok(()) => {
            let handle_lifecycle = lifecycle.clone();
            Ok(AgentHandle::new(dyn_agent, move || {
                let lifecycle = handle_lifecycle.clone();
                Box::pin(async move {
                    lifecycle.dispose().await;
                    Ok(())
                })
            }))
        }
        Err(error) => {
            lifecycle.dispose().await;
            Err(error)
        }
    }
}

/// Everything one external session holds, released once.
struct Lifecycle {
    agent: Arc<ExternalAgent>,
    scope: Scope,
    stored: Option<Stored>,
    detach_session: Mutex<Option<SessionDetach>>,
    detach_agent: Mutex<Option<AgentDetach>>,
    driver: Mutex<Option<Disposer>>,
    follows: Mutex<Vec<Disposer>>,
    done: OnceCell<()>,
}

impl Lifecycle {
    /// Stop the turn, the driver and the harness process, unwind the scope, close the write
    /// handle, leave the registries. Memoized: every owner awaits the same teardown.
    async fn teardown(self: &Arc<Self>) {
        let this = self.clone();
        self.done.get_or_init(|| this.run_teardown()).await;
    }

    async fn run_teardown(self: Arc<Self>) {
        // No turn starts from here on, even from input queued before.
        self.agent.close();
        let driver = lock(&self.driver).take();
        if let Some(driver) = driver {
            driver.dispose().await;
        }
        if let Some(control) = self.agent.control() {
            let grace =
                std::time::Duration::from_millis(self.agent.shared.config.terminate_grace_ms);
            control.terminate(grace).await;
        }
        self.scope.dispose().await;
        if let Some(stored) = &self.stored
            && let Err(error) = stored.handle.close().await
        {
            self.agent
                .shared
                .ctx
                .logger()
                .warn(format_args!("closing the session log failed: {error}"));
        }
        let detach = lock(&self.detach_agent).take();
        if let Some(detach) = detach {
            detach.detach();
        }
        let detach = lock(&self.detach_session).take();
        if let Some(detach) = detach {
            detach.detach();
        }
    }

    /// Teardown, then leave the owners' effects.
    async fn dispose(self: &Arc<Self>) {
        self.teardown().await;
        self.leave().await;
    }

    /// Leave the owners' effects. The one being undone right now returns at once (a repeat
    /// dispose); the other one runs its undo, which finds the teardown done.
    async fn leave(&self) {
        let follows = std::mem::take(&mut *lock(&self.follows));
        for follow in follows {
            follow.dispose().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(seq: u64, kind: &str, data: Value) -> Arc<SessionEvent> {
        Arc::new(SessionEvent::from_value(
            json!({"type": kind, "seq": seq, "time": 10, "data": data}),
        ))
    }

    fn binding(seq: u64, native: Value) -> Arc<SessionEvent> {
        event(
            seq,
            BINDING_RECORD,
            json!({"harness": "stub", "nativeSessionId": native}),
        )
    }

    #[test]
    fn the_binding_may_follow_other_leading_records() {
        let events = vec![
            event(0, "plugin:other/record", json!({})),
            binding(1, Value::Null),
            binding(2, json!("native-1")),
        ];
        assert_eq!(bound_harness(&events), Some("stub"));
        assert_eq!(
            recorded_native(&events),
            Some(NativeSessionId::from("native-1"))
        );
        assert_eq!(bound_harness(&events[..1]), None);
    }
}
