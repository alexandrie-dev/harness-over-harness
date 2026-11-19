//! Log-backed session titles, deterministic fallback, and provider registry.

pub mod normalize;
pub mod types;

use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};

use futures_util::future::BoxFuture;
use hoh_agent::{Agents, StepBoundaryKind};
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Disposer, Fiber, FiberState, Inject, Plugin, Service, Undo};
use hoh_llm::{GenerateOptions, LlmStream, LlmStreamEvent, is_agent_loop_request};
use hoh_session::{
    Session, SessionDisposed, SessionEvent, SessionEventEmitted, SessionSeq, Sessions,
};
use hoh_session_projection::{
    ProjectionDefinition, ProjectionHandle, ProjectionWire, SessionProjections,
};
use hoh_timeout::{AbortController, AbortSignal};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Notify;

pub use hoh_brand::ids::SessionTitleProviderId;
pub use normalize::{fallback_session_title, normalize_session_title, truncate_title_utf8};
pub use types::*;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn error(message: impl Into<String>) -> BoxError {
    message.into().into()
}

/// Required deterministic fallback and accepted-title limits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub fallback_max_words: f64,
    pub fallback_max_bytes: f64,
    pub max_title_bytes: f64,
}

#[derive(Clone, Copy)]
struct ResolvedConfig {
    fallback_max_words: usize,
    fallback_max_bytes: usize,
    max_title_bytes: usize,
}

impl ResolvedConfig {
    fn new(config: &Config) -> Result<Self, BoxError> {
        fn positive(name: &str, value: f64) -> Result<usize, BoxError> {
            if !value.is_finite()
                || value <= 0.0
                || value.fract() != 0.0
                || value > usize::MAX as f64
            {
                return Err(error(format!(
                    "session-title: {name} must be a positive integer"
                )));
            }
            Ok(value as usize)
        }
        let resolved = Self {
            fallback_max_words: positive("fallbackMaxWords", config.fallback_max_words)?,
            fallback_max_bytes: positive("fallbackMaxBytes", config.fallback_max_bytes)?,
            max_title_bytes: positive("maxTitleBytes", config.max_title_bytes)?,
        };
        if resolved.fallback_max_bytes > resolved.max_title_bytes {
            return Err(error(
                "session-title: fallbackMaxBytes must not exceed maxTitleBytes",
            ));
        }
        Ok(resolved)
    }
}

/// Rejection of an explicit title that normalizes to empty.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct SessionTitleInvalidError(pub String);

impl SessionTitleInvalidError {
    pub fn name(&self) -> &'static str {
        "SessionTitleInvalidError"
    }
}

/// Automatic generation cadence owned by a provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionTitleAutomaticMode {
    FirstPrompt,
    AllPrompts,
}

/// Immutable input supplied to one provider call.
#[derive(Clone)]
pub struct SessionTitleProviderRequest {
    pub session: Session,
    pub messages: Vec<SessionTitleUserMessage>,
    pub route: Option<SessionTitleModelIdentity>,
    pub current_title: Option<SessionTitleSnapshot>,
    pub signal: AbortSignal,
}

/// Provider output before service-owned normalization and acceptance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionTitleProviderResult {
    pub title: String,
    pub message_seqs: Vec<SessionSeq>,
    pub model: Option<SessionTitleModelIdentity>,
}

/// One optional asynchronous title implementation.
pub trait SessionTitleProvider: Send + Sync + 'static {
    fn id(&self) -> &SessionTitleProviderId;
    fn automatic(&self) -> SessionTitleAutomaticMode;
    fn generate(
        &self,
        request: SessionTitleProviderRequest,
    ) -> BoxFuture<'static, Result<SessionTitleProviderResult, BoxError>>;
}

struct ProviderRegistration {
    provider: Arc<dyn SessionTitleProvider>,
    closing: AtomicBool,
    active: AtomicUsize,
    drained: Notify,
}

impl ProviderRegistration {
    fn begin(self: &Arc<Self>) -> ProviderCallGuard {
        self.active.fetch_add(1, Ordering::AcqRel);
        ProviderCallGuard(self.clone())
    }
    async fn drain(&self) {
        loop {
            let notified = self.drained.notified();
            if self.active.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }
}

struct ProviderCallGuard(Arc<ProviderRegistration>);
impl Drop for ProviderCallGuard {
    fn drop(&mut self) {
        if self.0.active.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.drained.notify_waiters();
        }
    }
}

#[derive(Clone)]
struct PendingWork {
    registration: Arc<ProviderRegistration>,
    revision: u64,
    through_seq: SessionSeq,
}

#[derive(Clone)]
struct ActiveWork {
    pending: PendingWork,
    id: u64,
    controller: Arc<AbortController>,
    signal: AbortSignal,
}

struct SessionWork {
    session: Session,
    revision: u64,
    pending: Option<PendingWork>,
    active: Option<ActiveWork>,
    fallback: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Default)]
struct State {
    registration: Option<Arc<ProviderRegistration>>,
    work: Vec<SessionWork>,
}

struct Inner {
    ctx: Context,
    owner: Fiber,
    sessions: hoh_session::SessionStore,
    agents: hoh_agent::AgentRegistry,
    config: ResolvedConfig,
    lifetime: AbortSignal,
    title_input: Mutex<Option<ProjectionHandle<TitleInputState>>>,
    state: Mutex<State>,
}

#[cfg(test)]
struct RenameLiveCheckHook {
    session_id: String,
    reached: std::sync::mpsc::SyncSender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
static RENAME_LIVE_CHECK_HOOK: Mutex<Option<RenameLiveCheckHook>> = Mutex::new(None);

/// Log-backed title service.
#[derive(Clone)]
pub struct SessionTitleService(Arc<Inner>);
hoh_kernel::service!(pub SessionTitle: SessionTitleService = "sessionTitle");

struct ActiveCleanup {
    service: SessionTitleService,
    session: Session,
    active_id: u64,
}

impl Drop for ActiveCleanup {
    fn drop(&mut self) {
        let mut state = lock(&self.service.0.state);
        if let Some(index) = SessionTitleService::state_index(&state, &self.session)
            && state.work[index]
                .active
                .as_ref()
                .is_some_and(|active| active.id == self.active_id)
        {
            state.work[index].active = None;
        }
    }
}

fn session_title_user_message_of(event: &SessionEvent) -> Option<SessionTitleUserMessage> {
    if event.event_type() != "user/message" || event.data()["source"]["kind"] != "user" {
        return None;
    }
    let text = event.data()["content"]
        .as_array()?
        .iter()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if normalize_session_title(&text, usize::MAX).ok()?.is_empty() {
        return None;
    }
    Some(SessionTitleUserMessage {
        seq: event.seq(),
        text,
    })
}

fn title_input_valid(state: &TitleInputState) -> bool {
    (state.first.is_none() && state.last_seq.is_none() && state.count == 0)
        || (state.first.is_some()
            && state.last_seq.is_some()
            && state.count > 0
            && state
                .first
                .as_ref()
                .is_some_and(|first| state.last_seq.is_some_and(|last| first.seq <= last)))
}

fn parse_title_input(value: Option<&Value>) -> Result<Arc<TitleInputState>, BoxError> {
    let value = value.ok_or_else(|| error("expected title input state object"))?;
    let object = value
        .as_object()
        .ok_or_else(|| error("expected title input state object"))?;
    if !["first", "count", "lastSeq"]
        .iter()
        .all(|key| object.contains_key(*key))
    {
        return Err(error(
            "title input state must contain first, count, and lastSeq",
        ));
    }
    let state: TitleInputState = serde_json::from_value(value.clone())?;
    if !title_input_valid(&state) {
        return Err(error(
            "title input state must pair its count with first and last message seqs",
        ));
    }
    Ok(Arc::new(state))
}

/// Client-visible last-wins `title` projection.
pub fn title_projection_definition() -> ProjectionDefinition<Value> {
    let parse = Arc::new(|value: Option<&Value>| -> Result<Arc<Value>, BoxError> {
        match value {
            Some(Value::Null) => Ok(Arc::new(Value::Null)),
            Some(Value::String(value)) if !value.is_empty() => {
                Ok(Arc::new(Value::String(value.clone())))
            }
            _ => Err(error("title projection must be a non-empty string or null")),
        }
    });
    ProjectionDefinition {
        key: "title".into(),
        state_version: 1,
        state_schema: parse.clone(),
        init: Arc::new(|_, _| Ok(Arc::new(Value::Null))),
        apply: Arc::new(|state, event| {
            if event.event_type() != "session/title" {
                return Ok(state.clone());
            }
            let title = event.data()["title"]
                .as_str()
                .ok_or_else(|| error("session title event title must be a string"))?;
            Ok(Arc::new(Value::String(title.to_owned())))
        }),
        wire: Some(ProjectionWire::new(
            Arc::new(move |value| {
                let parsed = parse(value)?;
                Ok(parsed)
            }),
            Arc::new(|state| Ok(Some(state.clone()))),
        )),
    }
}

fn title_input_projection_definition() -> ProjectionDefinition<TitleInputState> {
    ProjectionDefinition {
        key: "titleInput".into(),
        state_version: 3,
        state_schema: Arc::new(parse_title_input),
        init: Arc::new(|_, _| Ok(Arc::new(TitleInputState::default()))),
        apply: Arc::new(|state, event| {
            let Some(message) = session_title_user_message_of(event) else {
                return Ok(state.clone());
            };
            Ok(Arc::new(TitleInputState {
                first: state.first.clone().or(Some(message.clone())),
                count: state.count + 1,
                last_seq: Some(message.seq),
            }))
        }),
        wire: None,
    }
}

/// Fold the latest logged title without consulting mutable metadata.
pub fn fold_session_title(events: &[Arc<SessionEvent>]) -> Option<SessionTitleSnapshot> {
    let event = events
        .iter()
        .rev()
        .find(|event| event.event_type() == "session/title")?;
    let data: SessionTitleEventData = serde_json::from_value(event.data().clone()).ok()?;
    Some(SessionTitleSnapshot {
        title: data.title,
        message_seqs: data.message_seqs,
        source: data.source,
        event_seq: event.seq(),
        updated_at: event.time(),
    })
}

impl SessionTitleService {
    fn service_active(&self) -> bool {
        !self.0.lifetime.aborted() && self.0.owner.state() == FiberState::Active
    }
    fn assert_service_active(&self) -> Result<(), BoxError> {
        if !self.service_active() {
            return Err(error("session-title service disposed"));
        }
        Ok(())
    }
    fn assert_live(&self, session: &Session) -> Result<(), BoxError> {
        if self.0.sessions.get(session.id()).as_ref() != Some(session) {
            return Err(error(format!(
                "session \"{}\" is not live in this store",
                session.id()
            )));
        }
        Ok(())
    }
    fn title_input_of(&self, session: &Session) -> Result<Arc<TitleInputState>, BoxError> {
        lock(&self.0.title_input)
            .as_ref()
            .ok_or_else(|| error("session-title input projection is not ready"))?
            .state_of(session)
    }
    fn state_index(state: &State, session: &Session) -> Option<usize> {
        state.work.iter().position(|work| work.session == *session)
    }
    fn state_for<'a>(state: &'a mut State, session: &Session) -> &'a mut SessionWork {
        let index = Self::state_index(state, session).unwrap_or_else(|| {
            state.work.push(SessionWork {
                session: session.clone(),
                revision: 0,
                pending: None,
                active: None,
                fallback: Arc::new(tokio::sync::Mutex::new(())),
            });
            state.work.len() - 1
        });
        &mut state.work[index]
    }
    fn state_for_live<'a>(
        &self,
        state: &'a mut State,
        session: &Session,
    ) -> Result<&'a mut SessionWork, BoxError> {
        if self.0.sessions.get(session.id()).as_ref() != Some(session) {
            return Err(error(format!(
                "session \"{}\" is not live in this store",
                session.id()
            )));
        }
        Ok(Self::state_for(state, session))
    }
    #[cfg(test)]
    fn pause_rename_after_live_check(session: &Session) {
        let hook = {
            let mut hook = lock(&RENAME_LIVE_CHECK_HOOK);
            if hook
                .as_ref()
                .is_some_and(|hook| hook.session_id == session.id().as_str())
            {
                hook.take()
            } else {
                None
            }
        };
        if let Some(hook) = hook {
            hook.reached.send(()).expect("race test is listening");
            hook.release.recv().expect("race test releases rename");
        }
    }
    fn supersede(work: &mut SessionWork, reason: &str) -> u64 {
        if let Some(active) = &work.active {
            active.controller.abort_with(reason.to_owned());
        }
        work.pending = None;
        work.revision += 1;
        work.revision
    }
    fn append_title(&self, session: &Session, data: SessionTitleEventData) -> Result<(), BoxError> {
        session.append("session/title", serde_json::to_value(data)?, None)?;
        Ok(())
    }
    pub fn get(&self, session: &Session) -> Option<SessionTitleSnapshot> {
        fold_session_title(&session.snapshot_events(None, None))
    }
    pub fn rename(&self, session: &Session, title: &str) -> Result<SessionTitleSnapshot, BoxError> {
        self.assert_service_active()?;
        self.assert_live(session)?;
        #[cfg(test)]
        Self::pause_rename_after_live_check(session);
        let normalized =
            normalize_session_title(title, self.0.config.max_title_bytes).map_err(error)?;
        if normalized.is_empty() {
            return Err(Box::new(SessionTitleInvalidError(
                "session title must contain visible characters".into(),
            )));
        }
        {
            let mut state = lock(&self.0.state);
            Self::supersede(
                self.state_for_live(&mut state, session)?,
                "user rename superseded automatic title generation",
            );
        }
        self.append_title(
            session,
            SessionTitleEventData {
                title: normalized,
                message_seqs: vec![],
                source: SessionTitleSource::User,
            },
        )?;
        self.get(session)
            .ok_or_else(|| error("renamed title failed to fold"))
    }
    pub fn register(
        &self,
        owner: &Context,
        provider: Arc<dyn SessionTitleProvider>,
    ) -> Result<Disposer, BoxError> {
        self.assert_service_active()?;
        if provider.id().as_str().is_empty() {
            return Err(error(
                "session-title provider id must be a non-empty string",
            ));
        }
        let registration = Arc::new(ProviderRegistration {
            provider,
            closing: AtomicBool::new(false),
            active: AtomicUsize::new(0),
            drained: Notify::new(),
        });
        let service = self.clone();
        owner.try_effect(
            "sessionTitle.register()",
            move |undos| -> Result<(), BoxError> {
                let drain_service = service.clone();
                let drain_registration = registration.clone();
                undos.push(Undo::future(move || {
                    let service = drain_service.clone();
                    let registration = drain_registration.clone();
                    async move {
                        registration.drain().await;
                        let mut state = lock(&service.0.state);
                        if state
                            .registration
                            .as_ref()
                            .is_some_and(|current| Arc::ptr_eq(current, &registration))
                        {
                            state.registration = None;
                        }
                        Ok(())
                    }
                }))?;
                let close_service = service.clone();
                let close_registration = registration.clone();
                undos.push(Undo::sync(move || {
                    close_service.close_registration(&close_registration);
                }))?;
                let mut state = lock(&service.0.state);
                service.assert_service_active()?;
                if registration.closing.load(Ordering::Acquire) {
                    return Err(error("session-title provider registration was disposed"));
                }
                if let Some(current) = &state.registration
                    && !current.closing.load(Ordering::Acquire)
                {
                    return Err(error(format!(
                        "session-title provider \"{}\" is already registered",
                        current.provider.id()
                    )));
                }
                state.registration = Some(registration);
                Ok(())
            },
        )
    }
    fn close_registration(&self, registration: &Arc<ProviderRegistration>) {
        let mut state = lock(&self.0.state);
        registration.closing.store(true, Ordering::Release);
        for work in &mut state.work {
            if work
                .pending
                .as_ref()
                .is_some_and(|p| Arc::ptr_eq(&p.registration, registration))
            {
                work.pending = None;
            }
            if let Some(active) = &work.active
                && Arc::ptr_eq(&active.pending.registration, registration)
            {
                active.controller.abort_with(format!(
                    "session-title provider \"{}\" was disposed",
                    registration.provider.id()
                ));
            }
        }
    }
    fn activate(&self, pending: PendingWork, upstream: Option<AbortSignal>) -> ActiveWork {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let controller = Arc::new(AbortController::new());
        let mut signals = vec![controller.signal(), self.0.lifetime.clone()];
        if let Some(signal) = upstream {
            signals.push(signal);
        }
        ActiveWork {
            pending,
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            controller,
            signal: AbortSignal::any(signals),
        }
    }
    fn route_of(session: &Session) -> Option<SessionTitleModelIdentity> {
        let header = session.request_header()?;
        Some(SessionTitleModelIdentity {
            provider: header["config"]["provider"].as_str()?.to_owned(),
            model: header["config"]["model"].as_str()?.to_owned(),
        })
    }
    pub async fn refresh(
        &self,
        session: &Session,
        signal: Option<AbortSignal>,
    ) -> Result<Option<SessionTitleSnapshot>, BoxError> {
        if let Some(signal) = &signal {
            signal.throw_if_aborted()?;
        }
        self.assert_service_active()?;
        self.assert_live(session)?;
        let input = self.title_input_of(session)?;
        let registration = lock(&self.0.state).registration.clone();
        if registration
            .as_ref()
            .is_none_or(|r| r.closing.load(Ordering::Acquire))
            || input.last_seq.is_none()
        {
            if self
                .get(session)
                .as_ref()
                .is_some_and(|t| matches!(t.source, SessionTitleSource::User))
                && let Some(first) = &input.first
            {
                self.append_fallback(session, first)?;
                if let Some(signal) = &signal {
                    signal.throw_if_aborted()?;
                }
                return Ok(self.get(session));
            }
            let result = self.ensure_fallback(session).await?;
            if let Some(signal) = &signal {
                signal.throw_if_aborted()?;
            }
            return Ok(result);
        }
        let registration = registration.expect("checked above");
        let (active, call) = {
            let mut state = lock(&self.0.state);
            if state
                .registration
                .as_ref()
                .is_none_or(|current| !Arc::ptr_eq(current, &registration))
                || registration.closing.load(Ordering::Acquire)
            {
                return Err(error(
                    "session title generation state changed without cancellation",
                ));
            }
            if self.0.sessions.get(session.id()).as_ref() != Some(session) {
                return Err(error(format!(
                    "session \"{}\" is not live in this store",
                    session.id()
                )));
            }
            let work = Self::state_for(&mut state, session);
            let revision =
                Self::supersede(work, "explicit title refresh superseded older generation");
            let pending = PendingWork {
                registration,
                revision,
                through_seq: input.last_seq.expect("checked above"),
            };
            let active = self.activate(pending, signal);
            let call = active.pending.registration.begin();
            work.active = Some(active.clone());
            (active, call)
        };
        self.start_provider(session.clone(), active, Self::route_of(session), call)
            .await
    }
    fn append_fallback(
        &self,
        session: &Session,
        first: &SessionTitleUserMessage,
    ) -> Result<(), BoxError> {
        let title = fallback_session_title(
            &first.text,
            self.0.config.fallback_max_words,
            self.0.config.fallback_max_bytes,
        )
        .map_err(error)?;
        if !title.is_empty() {
            self.append_title(
                session,
                SessionTitleEventData {
                    title,
                    message_seqs: vec![first.seq],
                    source: SessionTitleSource::Fallback,
                },
            )?;
        }
        Ok(())
    }
    async fn ensure_fallback(
        &self,
        session: &Session,
    ) -> Result<Option<SessionTitleSnapshot>, BoxError> {
        self.assert_service_active()?;
        if let Some(current) = self.get(session) {
            return Ok(Some(current));
        }
        let input = self.title_input_of(session)?;
        let Some(first) = input.first.clone() else {
            return Ok(None);
        };
        let title = fallback_session_title(
            &first.text,
            self.0.config.fallback_max_words,
            self.0.config.fallback_max_bytes,
        )
        .map_err(error)?;
        if title.is_empty() {
            return Ok(None);
        }
        let fallback = {
            let mut state = lock(&self.0.state);
            if self.0.sessions.get(session.id()).as_ref() != Some(session) {
                return Err(error(format!(
                    "session \"{}\" is not live in this store",
                    session.id()
                )));
            }
            Self::state_for(&mut state, session).fallback.clone()
        };
        let _guard = fallback.lock().await;
        self.assert_service_active()?;
        self.assert_live(session)?;
        if let Some(current) = self.get(session) {
            return Ok(Some(current));
        }
        self.append_title(
            session,
            SessionTitleEventData {
                title,
                message_seqs: vec![first.seq],
                source: SessionTitleSource::Fallback,
            },
        )?;
        Ok(self.get(session))
    }
    fn collect_messages(
        events: &[Arc<SessionEvent>],
        through: SessionSeq,
    ) -> Vec<SessionTitleUserMessage> {
        events
            .iter()
            .take_while(|e| e.seq() <= through)
            .filter_map(|e| session_title_user_message_of(e))
            .collect()
    }
    fn assert_current(&self, session: &Session, active: &ActiveWork) -> Result<(), BoxError> {
        self.assert_service_active()?;
        active.signal.throw_if_aborted()?;
        let state = lock(&self.0.state);
        let index = Self::state_index(&state, session);
        let current = index.and_then(|i| state.work[i].active.as_ref());
        if state
            .registration
            .as_ref()
            .is_none_or(|r| !Arc::ptr_eq(r, &active.pending.registration))
            || current.is_none_or(|w| w.id != active.id)
            || index.is_none_or(|i| state.work[i].revision != active.pending.revision)
            || self.0.sessions.get(session.id()).as_ref() != Some(session)
        {
            return Err(error(
                "session title generation state changed without cancellation",
            ));
        }
        Ok(())
    }
    fn validate_result(
        &self,
        result: SessionTitleProviderResult,
        messages: &[SessionTitleUserMessage],
    ) -> Result<SessionTitleProviderResult, BoxError> {
        let title =
            normalize_session_title(&result.title, self.0.config.max_title_bytes).map_err(error)?;
        if title.is_empty() {
            return Err(error("session-title provider returned an empty title"));
        }
        if result.message_seqs.is_empty() {
            return Err(error(
                "session-title provider must identify at least one source message seq",
            ));
        }
        let mut previous = None;
        for seq in &result.message_seqs {
            let index = messages.iter().position(|m| m.seq == *seq);
            if index.is_none() || previous.is_some_and(|p| index <= Some(p)) {
                return Err(error(
                    "session-title provider messageSeqs must be unique, ordered seqs from the request",
                ));
            }
            previous = index;
        }
        if result
            .model
            .as_ref()
            .is_some_and(|m| m.provider.is_empty() || m.model.is_empty())
        {
            return Err(error(
                "session-title provider result model must contain non-empty provider and model strings",
            ));
        }
        Ok(SessionTitleProviderResult { title, ..result })
    }
    async fn start_provider(
        &self,
        session: Session,
        active: ActiveWork,
        route: Option<SessionTitleModelIdentity>,
        _call: ProviderCallGuard,
    ) -> Result<Option<SessionTitleSnapshot>, BoxError> {
        let _active = ActiveCleanup {
            service: self.clone(),
            session: session.clone(),
            active_id: active.id,
        };
        self.run_provider(&session, &active, route).await
    }
    async fn run_provider(
        &self,
        session: &Session,
        active: &ActiveWork,
        route: Option<SessionTitleModelIdentity>,
    ) -> Result<Option<SessionTitleSnapshot>, BoxError> {
        self.assert_current(session, active)?;
        self.ensure_fallback(session).await?;
        self.assert_current(session, active)?;
        let events = session.snapshot_events(None, None);
        let messages = Self::collect_messages(&events, active.pending.through_seq);
        let current_title = fold_session_title(&events);
        let result = active
            .pending
            .registration
            .provider
            .generate(SessionTitleProviderRequest {
                session: session.clone(),
                messages: messages.clone(),
                route,
                current_title,
                signal: active.signal.clone(),
            })
            .await?;
        self.assert_current(session, active)?;
        let accepted = self.validate_result(result, &messages)?;
        self.append_title(
            session,
            SessionTitleEventData {
                title: accepted.title,
                message_seqs: accepted.message_seqs,
                source: SessionTitleSource::Provider {
                    provider: active.pending.registration.provider.id().clone(),
                    model: accepted.model,
                },
            },
        )?;
        Ok(self.get(session))
    }
    fn defer_fallback(&self, session: Session) -> Result<(), BoxError> {
        let service = self.clone();
        let log = self.0.ctx.logger();
        self.0
            .ctx
            .spawn("sessionTitle fallback", move |_| async move {
                if let Err(failure) = service.ensure_fallback(&session).await
                    && service.service_active()
                {
                    log.warn(format_args!(
                        "session \"{}\": fallback title update failed: Error: {failure}",
                        session.id()
                    ));
                }
                Ok(())
            })?;
        Ok(())
    }
    async fn on_user_message(
        &self,
        session: Session,
        event: Arc<SessionEvent>,
    ) -> Result<(), BoxError> {
        if session_title_user_message_of(&event).is_none() {
            return Ok(());
        }
        if self.0.owner.state() == FiberState::Loading {
            self.0.owner.settled().await?;
            if self.service_active() && self.0.sessions.get(session.id()).as_ref() == Some(&session)
            {
                self.ensure_fallback(&session).await?;
            }
            return Ok(());
        }
        if !self.service_active() {
            return Ok(());
        }
        if self
            .get(&session)
            .is_some_and(|t| matches!(t.source, SessionTitleSource::User))
        {
            return Ok(());
        }
        let registration = lock(&self.0.state).registration.clone();
        if let Some(registration) = registration
            && !registration.closing.load(Ordering::Acquire)
        {
            let input = self.title_input_of(&session)?;
            let header = session.header();
            let schedule = registration.provider.automatic()
                == SessionTitleAutomaticMode::AllPrompts
                || (header["isSeeded"] != true
                    && header["origin"].as_str() != Some("subagent")
                    && input.count == 1
                    && self.get(&session).is_none());
            if schedule {
                let mut state = lock(&self.0.state);
                let current = state
                    .registration
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &registration))
                    && !registration.closing.load(Ordering::Acquire);
                if current {
                    let work = self.state_for_live(&mut state, &session)?;
                    let revision =
                        Self::supersede(work, "newer user message superseded title generation");
                    work.pending = Some(PendingWork {
                        registration,
                        revision,
                        through_seq: event.seq(),
                    });
                }
            }
        }
        self.defer_fallback(session)
    }
    fn start_pending(
        &self,
        session: Session,
        route: SessionTitleModelIdentity,
    ) -> Result<(), BoxError> {
        let pending = {
            let mut state = lock(&self.0.state);
            let Some(index) = Self::state_index(&state, &session) else {
                return Ok(());
            };
            state.work[index].pending.take()
        };
        let Some(pending) = pending else {
            return Ok(());
        };
        let service = self.clone();
        let log = self.0.ctx.logger();
        self.0
            .ctx
            .spawn("sessionTitle provider", move |_| async move {
                let (active, call) = {
                    let mut state = lock(&service.0.state);
                    let eligible = state
                        .registration
                        .as_ref()
                        .is_some_and(|r| Arc::ptr_eq(r, &pending.registration))
                        && !pending.registration.closing.load(Ordering::Acquire);
                    let Some(index) = SessionTitleService::state_index(&state, &session) else {
                        return Ok(());
                    };
                    if !eligible || state.work[index].revision != pending.revision {
                        return Ok(());
                    }
                    let active = service.activate(pending, None);
                    let call = active.pending.registration.begin();
                    state.work[index].active = Some(active.clone());
                    (active, call)
                };
                if let Err(failure) = service
                    .start_provider(session.clone(), active.clone(), Some(route), call)
                    .await
                    && !active.signal.aborted()
                    && service.service_active()
                {
                    log.warn(format_args!(
                        "session \"{}\": automatic title generation failed: Error: {failure}",
                        session.id()
                    ));
                }
                Ok(())
            })?;
        Ok(())
    }
    fn on_request_header(&self, session: &Session, event: &SessionEvent) -> Result<(), BoxError> {
        if !self.service_active() {
            return Ok(());
        }
        let should_start = {
            let state = lock(&self.0.state);
            Self::state_index(&state, session)
                .and_then(|i| state.work[i].pending.as_ref())
                .is_some_and(|p| p.through_seq < event.seq())
        };
        if !should_start {
            return Ok(());
        }
        self.start_pending(
            session.clone(),
            SessionTitleModelIdentity {
                provider: event.data()["header"]["config"]["provider"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                model: event.data()["header"]["config"]["model"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            },
        )
    }
    fn on_main_request(&self, options: &GenerateOptions) -> Result<(), BoxError> {
        if !self.service_active() || !is_agent_loop_request(options) {
            return Ok(());
        }
        let Some(id) = &options.session_id else {
            return Ok(());
        };
        let Some(session) = self.0.sessions.get(id) else {
            return Ok(());
        };
        let pending = {
            let state = lock(&self.0.state);
            Self::state_index(&state, &session).is_some_and(|i| state.work[i].pending.is_some())
        };
        if !pending {
            return Ok(());
        }
        let boundary = self.0.agents.turn_boundary_of(&session)?;
        let route = Self::route_of(&session);
        if boundary
            .last_step_boundary
            .is_none_or(|b| b.kind != StepBoundaryKind::Start)
            || route
                .as_ref()
                .is_none_or(|r| r.provider != options.provider || r.model != options.model)
        {
            return Ok(());
        }
        self.start_pending(
            session,
            SessionTitleModelIdentity {
                provider: options.provider.clone(),
                model: options.model.clone(),
            },
        )
    }
    fn disposed(&self, session: &Session) {
        let mut state = lock(&self.0.state);
        if let Some(index) = Self::state_index(&state, session) {
            if let Some(active) = &state.work[index].active {
                active
                    .controller
                    .abort_with("session disposed during title generation".to_owned());
            }
            state.work.remove(index);
        }
    }

    async fn drain_shutdown(&self) {
        let registrations = {
            let state = lock(&self.0.state);
            let mut registrations = Vec::new();
            let mut push = |registration: &Arc<ProviderRegistration>| {
                if !registrations
                    .iter()
                    .any(|current| Arc::ptr_eq(current, registration))
                {
                    registrations.push(registration.clone());
                }
            };
            if let Some(registration) = &state.registration {
                push(registration);
            }
            for work in &state.work {
                if let Some(pending) = &work.pending {
                    push(&pending.registration);
                }
                if let Some(active) = &work.active {
                    push(&active.pending.registration);
                }
            }
            registrations
        };
        for registration in &registrations {
            self.close_registration(registration);
        }
        for registration in registrations {
            registration.drain().await;
        }
    }
}

#[derive(Inject)]
pub struct Deps {
    sessions: Service<Sessions>,
    projections: Service<SessionProjections>,
    agents: Service<Agents>,
}

/// The source class plugin, named after its class.
pub struct SessionTitlePlugin;
impl Plugin for SessionTitlePlugin {
    const NAME: &'static str = "SessionTitleService";
    type Config = Config;
    type Deps = Deps;
    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let resolved = ResolvedConfig::new(&config.get())?;
        let service = SessionTitleService(Arc::new(Inner {
            owner: ctx.fiber(),
            sessions: (*deps.sessions).clone(),
            agents: (*deps.agents).clone(),
            config: resolved,
            lifetime: AbortSignal::from_token(ctx.token()),
            title_input: Mutex::new(None),
            state: Mutex::new(State::default()),
            ctx: ctx.clone(),
        }));
        let user = service.clone();
        ctx.on_contained_async::<SessionEventEmitted, _>(move |emitted| {
            let service = user.clone();
            async move {
                if emitted.event.event_type() == "user/message" {
                    service
                        .on_user_message(emitted.session.clone(), emitted.event.clone())
                        .await?;
                }
                Ok(())
            }
        })?;
        let header = service.clone();
        ctx.on_contained::<SessionEventEmitted>(move |emitted| {
            if emitted.event.event_type() == "request/header" {
                header.on_request_header(&emitted.session, &emitted.event)?;
            }
            Ok(())
        })?;
        let disposed = service.clone();
        ctx.on_contained::<SessionDisposed>(move |event| {
            disposed.disposed(&event.session);
            Ok(())
        })?;
        let stream = service.clone();
        ctx.listen()
            .global()
            .prepend()
            .on_waterfall::<LlmStreamEvent, LlmStream, _>(move |event, next| {
                let service = stream.clone();
                async move {
                    service.on_main_request(&event.options())?;
                    next.run().await
                }
            })?;
        deps.projections
            .register(&ctx, title_projection_definition())
            .await?;
        let input = deps
            .projections
            .register(&ctx, title_input_projection_definition())
            .await?;
        *lock(&service.0.title_input) = Some(input);
        ctx.provide::<SessionTitle>(Arc::new(service)).await?;
        let cleanup = ctx.clone();
        let shutdown = cleanup
            .get::<SessionTitle>()
            .expect("title service was just provided");
        ctx.effect("sessionTitle shutdown drain", move || {
            Undo::future(move || {
                let shutdown = shutdown.clone();
                async move {
                    shutdown.drain_shutdown().await;
                    Ok(())
                }
            })
        })?;
        Ok(())
    }
}

pub fn session_title_provider_id(id: impl Into<String>) -> SessionTitleProviderId {
    id.into().into()
}

#[cfg(test)]
mod review_regression_tests {
    use super::*;
    use hoh_agent::{AgentRegistryPlugin, Agents};
    use hoh_kernel::{Kernel, NoConfig};
    use hoh_session::{SessionPlugin, Sessions};
    use hoh_session_projection::SessionProjectionRegistry;
    use serde_json::json;
    use std::time::Duration;

    struct InactiveOwnerPlugin;
    impl Plugin for InactiveOwnerPlugin {
        const NAME: &'static str = "InactiveSessionTitleOwner";
        type Config = NoConfig;
        type Deps = ();

        async fn apply(_: Context, _: (), _: Cfg<NoConfig>) -> Result<(), BoxError> {
            Ok(())
        }
    }

    struct UnusedProvider(SessionTitleProviderId);
    impl SessionTitleProvider for UnusedProvider {
        fn id(&self) -> &SessionTitleProviderId {
            &self.0
        }
        fn automatic(&self) -> SessionTitleAutomaticMode {
            SessionTitleAutomaticMode::FirstPrompt
        }
        fn generate(
            &self,
            _: SessionTitleProviderRequest,
        ) -> BoxFuture<'static, Result<SessionTitleProviderResult, BoxError>> {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn assert_current_checks_owner_liveness_before_mutable_generation_state() {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<SessionPlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
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
        let owner = ctx.plugin::<InactiveOwnerPlugin>(json!({})).unwrap();
        owner.settled().await.unwrap();
        owner.dispose().await;
        let session = Session::create("inactive-owner".into(), None, None, None, None).unwrap();
        let registration = Arc::new(ProviderRegistration {
            provider: Arc::new(UnusedProvider("unused".into())),
            closing: AtomicBool::new(false),
            active: AtomicUsize::new(0),
            drained: Notify::new(),
        });
        let active = ActiveWork {
            pending: PendingWork {
                registration,
                revision: 0,
                through_seq: SessionSeq::new(0).unwrap(),
            },
            id: 1,
            controller: Arc::new(AbortController::new()),
            signal: AbortController::new().signal(),
        };
        let service = SessionTitleService(Arc::new(Inner {
            ctx: ctx.clone(),
            owner,
            sessions: (*ctx.get::<Sessions>().unwrap()).clone(),
            agents: (*ctx.get::<Agents>().unwrap()).clone(),
            config: ResolvedConfig {
                fallback_max_words: 5,
                fallback_max_bytes: 40,
                max_title_bytes: 80,
            },
            lifetime: AbortController::new().signal(),
            title_input: Mutex::new(None),
            state: Mutex::new(State::default()),
        }));
        assert_eq!(
            service
                .assert_current(&session, &active)
                .unwrap_err()
                .to_string(),
            "session-title service disposed"
        );
        kernel.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rename_rechecks_liveness_before_creating_session_work() {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<SessionPlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
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
        ctx.plugin::<SessionTitlePlugin>(json!({
            "fallbackMaxWords":5,
            "fallbackMaxBytes":40,
            "maxTitleBytes":80
        }))
        .unwrap()
        .settled()
        .await
        .unwrap();

        let sessions = (*ctx.get::<Sessions>().unwrap()).clone();
        let titles = (*ctx.get::<SessionTitle>().unwrap()).clone();
        let session = sessions
            .prepare(Some("rename-detach-race".into()), Default::default())
            .unwrap();
        let detach = sessions.enter(&ctx, session.clone()).unwrap();
        sessions.announce(&session).unwrap();

        let (reached_tx, reached_rx) = std::sync::mpsc::sync_channel(0);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        *lock(&RENAME_LIVE_CHECK_HOOK) = Some(RenameLiveCheckHook {
            session_id: session.id().to_string(),
            reached: reached_tx,
            release: release_rx,
        });
        let rename_titles = titles.clone();
        let rename_session = session.clone();
        let rename = tokio::task::spawn_blocking(move || {
            rename_titles
                .rename(&rename_session, "Detached title")
                .map_err(|error| error.to_string())
        });

        reached_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("rename reached the post-live-check barrier");
        detach.detach();
        release_tx.send(()).unwrap();

        assert_eq!(
            rename.await.unwrap().unwrap_err(),
            "session \"rename-detach-race\" is not live in this store"
        );
        assert!(SessionTitleService::state_index(&lock(&titles.0.state), &session).is_none());
        assert!(titles.get(&session).is_none());
        kernel.shutdown().await;
    }
}
