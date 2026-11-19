//! Canonical Session-log feedback for finalized assistant messages.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use futures_util::future::BoxFuture;
use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service, Undo};
use hoh_session::{
    Session, SessionEvent, SessionHeader, SessionId, SessionSeq, Sessions, derive_event_message,
    is_append_surface_event,
};
use hoh_session_persistence::{
    SessionAccess, SessionHandle, SessionInspection, SessionPersistenceService,
};
use hoh_util_values::js_trim;
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{Notify, oneshot};

use crate::*;

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub max_note_bytes: f64,
}

impl Config {
    fn resolve(&self) -> Result<usize, BoxError> {
        if !self.max_note_bytes.is_finite()
            || self.max_note_bytes.fract() != 0.0
            || !(1.0..=MAX_SAFE_INTEGER as f64).contains(&self.max_note_bytes)
            || self.max_note_bytes > usize::MAX as f64
        {
            return Err("message-feedback: maxNoteBytes must be a positive safe integer".into());
        }
        Ok(self.max_note_bytes as usize)
    }
}

#[derive(Clone, Debug)]
pub struct FeedbackCommitted {
    pub inspection: SessionInspection,
}
hoh_kernel::event!(
    FeedbackCommitted = "feedback/committed",
    hoh_kernel::Parallel
);

#[derive(Default)]
struct Completion {
    done: AtomicBool,
    notify: Notify,
}

impl Completion {
    async fn wait(&self) {
        loop {
            if self.done.load(Ordering::Acquire) {
                return;
            }
            let notified = self.notify.notified();
            if self.done.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    fn finish(&self) {
        self.done.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }
}

struct OperationState {
    admission_open: bool,
    active: usize,
    tails: HashMap<SessionId, Arc<Completion>>,
}

struct Inner {
    ctx: Context,
    sessions: Service<Sessions>,
    persistence: Service<SessionPersistenceService>,
    max_note_bytes: usize,
    operations: Mutex<OperationState>,
    drained: Notify,
    now: Mutex<Arc<dyn Fn() -> u64 + Send + Sync>>,
}

struct TaskGuard {
    inner: Arc<Inner>,
    session_id: SessionId,
    completion: Arc<Completion>,
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.completion.finish();
        let empty = {
            let mut state = lock(&self.inner.operations);
            if state
                .tails
                .get(&self.session_id)
                .is_some_and(|tail| Arc::ptr_eq(tail, &self.completion))
            {
                state.tails.remove(&self.session_id);
            }
            state.active = state.active.saturating_sub(1);
            state.active == 0
        };
        if empty {
            self.inner.drained.notify_waiters();
        }
    }
}

#[derive(Clone)]
pub struct MessageFeedbackService(Arc<Inner>);
hoh_kernel::service!(pub MessageFeedback: MessageFeedbackService = "messageFeedback");

enum Mutation {
    Put(MessageFeedbackPut),
    Delete(MessageFeedbackDelete),
}

impl Mutation {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Put(_) => "feedback/message-put",
            Self::Delete(_) => "feedback/message-delete",
        }
    }

    fn data(&self) -> Result<serde_json::Value, BoxError> {
        Ok(match self {
            Self::Put(value) => serde_json::to_value(value)?,
            Self::Delete(value) => serde_json::to_value(value)?,
        })
    }
}

enum LoadedOwner {
    Live(Session),
    Cold(Arc<dyn SessionHandle>),
}

struct LoadedSession {
    service: MessageFeedbackService,
    session_id: SessionId,
    events: Vec<Arc<SessionEvent>>,
    owner: LoadedOwner,
}

impl LoadedSession {
    async fn commit(&self, mutation: Option<Mutation>) -> Result<(), BoxError> {
        match &self.owner {
            LoadedOwner::Live(session) => {
                if let Some(mutation) = mutation {
                    session.append(mutation.event_type(), mutation.data()?, None)?;
                }
                let last = session.snapshot_events(None, None).last().cloned();
                if !self.service.0.sessions.flush(session).await? {
                    return Err(format!(
                        "message-feedback: no durability listener participated for live session '{}'",
                        self.session_id
                    )
                    .into());
                }
                let handle = self
                    .service
                    .0
                    .persistence
                    .open(&self.session_id, SessionAccess::Read, None)
                    .await?;
                let verification = async {
                    let stored = handle
                        .read(last.as_ref().map_or(0, |event| event.seq().get()).into(), Some(1), None)
                        .await?;
                    let live_header: SessionHeader =
                        serde_json::from_value((*session.header()).clone())?;
                    let header_matches = handle.header().id == live_header.id
                        && handle.header().created_at == live_header.created_at
                        && handle.header().cwd == live_header.cwd;
                    let tail_matches = last
                        .as_ref()
                        .is_none_or(|event| stored.events.first() == Some(event));
                    if !header_matches || !tail_matches {
                        return Err(format!(
                            "message-feedback: feedback prefix is not durable for live session '{}'",
                            self.session_id
                        )
                        .into());
                    }
                    Ok(())
                }
                .await;
                close_overrides(handle, verification).await
            }
            LoadedOwner::Cold(handle) => {
                let entry = match mutation {
                    Some(mutation) => {
                        let seq = SessionSeq::new(self.events.len() as u64)?;
                        Some(Arc::new(SessionEvent::from_value(json!({
                            "type": mutation.event_type(),
                            "seq": seq,
                            "time": (lock(&self.service.0.now))(),
                            "data": mutation.data()?,
                        }))))
                    }
                    None => None,
                };
                if let Some(entry) = &entry {
                    handle.append(std::slice::from_ref(entry), None).await?;
                }
                handle.flush(None).await?;
                if let Some(entry) = entry {
                    let mut events = self.events.clone();
                    events.push(entry);
                    let committed = FeedbackCommitted {
                        inspection: SessionInspection {
                            meta: handle.header().clone(),
                            inherited_event_count: handle.inherited_event_count(),
                            events,
                        },
                    };
                    if let Err(error) = self
                        .service
                        .0
                        .ctx
                        .parallel::<FeedbackCommitted>(committed)
                        .await
                    {
                        self.service.0.ctx.logger().warn(format!(
                            "message-feedback: committed feedback observer failed: {error}"
                        ));
                    }
                }
                Ok(())
            }
        }
    }

    async fn finish<T>(self, result: Result<T, BoxError>) -> Result<T, BoxError> {
        match self.owner {
            LoadedOwner::Cold(handle) => close_overrides(handle, result).await,
            LoadedOwner::Live(_) => result,
        }
    }
}

async fn close_overrides<T>(
    handle: Arc<dyn SessionHandle>,
    result: Result<T, BoxError>,
) -> Result<T, BoxError> {
    handle.close().await?;
    result
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(MAX_SAFE_INTEGER as u128) as u64)
        .unwrap_or(0)
}

fn valid_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    let shape = bytes.len() == 36
        && bytes.iter().copied().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        });
    if !shape {
        return false;
    }
    let nil = bytes
        .iter()
        .copied()
        .filter(|byte| *byte != b'-')
        .all(|byte| byte == b'0');
    let max = bytes
        .iter()
        .copied()
        .filter(|byte| *byte != b'-')
        .all(|byte| byte == b'f');
    nil || max
        || (matches!(bytes[14], b'1'..=b'8')
            && matches!(bytes[19], b'8' | b'9' | b'a' | b'b' | b'A' | b'B'))
}

fn validate_item(item: &MessageFeedbackItem) -> Result<(), BoxError> {
    if item.message_id.as_str().is_empty()
        || item
            .note
            .as_deref()
            .is_some_and(|note| js_trim(note).is_empty())
        || !valid_uuid(item.version.as_str())
        || item.created_at > MAX_SAFE_INTEGER
        || item.updated_at > MAX_SAFE_INTEGER
        || item.updated_at < item.created_at
    {
        return Err("message-feedback: invalid persisted feedback item".into());
    }
    Ok(())
}

fn current_items(
    session_id: &SessionId,
    events: &[Arc<SessionEvent>],
) -> Result<Vec<MessageFeedbackItem>, BoxError> {
    let mut items = IndexMap::new();
    for event in events {
        match event.event_type() {
            "feedback/message-put" => {
                let put: MessageFeedbackPut = serde_json::from_value(event.data().clone())?;
                if put.session_id.as_str().is_empty() {
                    return Err("message-feedback: invalid feedback Session id".into());
                }
                validate_item(&put.item)?;
                if &put.session_id == session_id {
                    items.insert(put.item.message_id.clone(), put.item);
                }
            }
            "feedback/message-delete" => {
                let delete: MessageFeedbackDelete = serde_json::from_value(event.data().clone())?;
                if delete.session_id.as_str().is_empty() || delete.message_id.as_str().is_empty() {
                    return Err("message-feedback: invalid persisted feedback deletion".into());
                }
                if &delete.session_id == session_id {
                    items.shift_remove(&delete.message_id);
                }
            }
            _ => {}
        }
    }
    Ok(items.into_values().collect())
}

fn target_exists(events: &[Arc<SessionEvent>], message_id: &hoh_llm::MessageId) -> bool {
    events.iter().any(|event| {
        event.event_type() == "assistant/message"
            && is_append_surface_event(event)
            && derive_event_message(event, None)
                .and_then(|message| message["id"].as_str().map(str::to_owned))
                .as_deref()
                == Some(message_id.as_str())
    })
}

impl MessageFeedbackService {
    fn new(
        ctx: Context,
        sessions: Service<Sessions>,
        persistence: Service<SessionPersistenceService>,
        max_note_bytes: usize,
    ) -> Self {
        Self(Arc::new(Inner {
            ctx,
            sessions,
            persistence,
            max_note_bytes,
            operations: Mutex::new(OperationState {
                admission_open: true,
                active: 0,
                tails: HashMap::new(),
            }),
            drained: Notify::new(),
            now: Mutex::new(Arc::new(system_now_ms)),
        }))
    }

    fn enqueue<T, F>(
        &self,
        session_id: SessionId,
        operation: F,
    ) -> BoxFuture<'static, Result<T, BoxError>>
    where
        T: Send + 'static,
        F: FnOnce(MessageFeedbackService) -> BoxFuture<'static, Result<T, BoxError>>
            + Send
            + 'static,
    {
        let (previous, completion) = {
            let mut state = lock(&self.0.operations);
            if !state.admission_open {
                return Box::pin(async { Err("message-feedback: service is disposing".into()) });
            }
            let previous = state.tails.get(&session_id).cloned();
            let completion = Arc::new(Completion::default());
            state.tails.insert(session_id.clone(), completion.clone());
            state.active += 1;
            (previous, completion)
        };
        let service = self.clone();
        let task_service = service.clone();
        let task_completion = completion.clone();
        let task_session_id = session_id.clone();
        let (send, receive) = oneshot::channel();
        tokio::spawn(async move {
            let _guard = TaskGuard {
                inner: task_service.0.clone(),
                session_id: task_session_id,
                completion: task_completion,
            };
            if let Some(previous) = previous {
                previous.wait().await;
            }
            let _ = send.send(operation(task_service).await);
        });
        Box::pin(async move {
            receive.await.map_err(|_| -> BoxError {
                format!(
                    "message-feedback: operation task ended for session '{}'",
                    session_id
                )
                .into()
            })?
        })
    }

    async fn load_session(
        &self,
        session_id: &SessionId,
        write: bool,
    ) -> Result<Option<LoadedSession>, BoxError> {
        if self.0.sessions.get(session_id).is_none()
            && self.0.persistence.stat(session_id, None).await?.is_none()
            && self.0.sessions.get(session_id).is_none()
        {
            return Ok(None);
        }
        if let Some(session) = self.0.sessions.get(session_id) {
            return Ok(Some(LoadedSession {
                service: self.clone(),
                session_id: session_id.clone(),
                events: session.snapshot_events(None, None).to_vec(),
                owner: LoadedOwner::Live(session),
            }));
        }
        let handle = self
            .0
            .persistence
            .open(
                session_id,
                if write {
                    SessionAccess::Write
                } else {
                    SessionAccess::Read
                },
                None,
            )
            .await?;
        let read = handle.read(None, None, None).await;
        let events = close_on_error(handle.clone(), read).await?.events;
        Ok(Some(LoadedSession {
            service: self.clone(),
            session_id: session_id.clone(),
            events,
            owner: LoadedOwner::Cold(handle),
        }))
    }

    /// Remote: messageFeedback.list
    pub fn list(
        &self,
        request: MessageFeedbackListRequest,
    ) -> BoxFuture<'static, Result<MessageFeedbackListResult, BoxError>> {
        let session_id = request.session_id.clone();
        self.enqueue(request.session_id, move |service| {
            Box::pin(async move {
                let Some(loaded) = service.load_session(&session_id, false).await? else {
                    return Ok(MessageFeedbackResult::rejected(
                        MessageFeedbackListFailure::SessionNotFound { session_id },
                    ));
                };
                let result = current_items(&session_id, &loaded.events).map(|items| {
                    MessageFeedbackResult::success(MessageFeedbackListValue { items })
                });
                loaded.finish(result).await
            })
        })
    }

    /// Remote: messageFeedback.put
    pub fn put(
        &self,
        request: MessageFeedbackPutRequest,
    ) -> BoxFuture<'static, Result<MessageFeedbackPutResult, BoxError>> {
        let note = match self.resolve_note(request.note.as_deref()) {
            Ok(note) => note,
            Err(failure) => {
                return Box::pin(async move { Ok(MessageFeedbackResult::rejected(failure)) });
            }
        };
        let session_id = request.session_id.clone();
        self.enqueue(request.session_id.clone(), move |service| {
            Box::pin(async move {
                let Some(loaded) = service.load_session(&session_id, true).await? else {
                    return Ok(MessageFeedbackResult::rejected(
                        MessageFeedbackPutFailure::SessionNotFound { session_id },
                    ));
                };
                let result = async {
                    let items = current_items(&session_id, &loaded.events)?;
                    if !target_exists(&loaded.events, &request.message_id) {
                        return Ok(MessageFeedbackResult::rejected(
                            MessageFeedbackPutFailure::TargetNotFound {
                                session_id: session_id.clone(),
                                message_id: request.message_id,
                            },
                        ));
                    }
                    let existing = items
                        .iter()
                        .find(|item| item.message_id == request.message_id);
                    if request.if_version.as_ref() != existing.map(|item| &item.version) {
                        return Ok(MessageFeedbackResult::rejected(
                            MessageFeedbackPutFailure::VersionConflict {
                                current: existing.cloned(),
                            },
                        ));
                    }
                    if let Some(existing) = existing
                        && existing.rating == request.rating
                        && existing.note == note
                        && existing.category == request.category
                    {
                        loaded.commit(None).await?;
                        return Ok(MessageFeedbackResult::success(existing.clone()));
                    }
                    let now = (lock(&service.0.now))();
                    let item = MessageFeedbackItem {
                        message_id: request.message_id,
                        rating: request.rating,
                        note,
                        category: request.category,
                        version: MessageFeedbackVersion::from(
                            hoh_util_crypto::random_uuid().as_str(),
                        ),
                        created_at: existing.map_or(now, |item| item.created_at),
                        updated_at: existing.map_or(now, |item| now.max(item.updated_at)),
                        extensions: Box::default(),
                    };
                    loaded
                        .commit(Some(Mutation::Put(MessageFeedbackPut {
                            session_id: session_id.clone(),
                            item: item.clone(),
                        })))
                        .await?;
                    Ok(MessageFeedbackResult::success(item))
                }
                .await;
                loaded.finish(result).await
            })
        })
    }

    /// Remote: messageFeedback.delete
    pub fn delete(
        &self,
        request: MessageFeedbackDeleteRequest,
    ) -> BoxFuture<'static, Result<MessageFeedbackDeleteResult, BoxError>> {
        let session_id = request.session_id.clone();
        self.enqueue(request.session_id.clone(), move |service| {
            Box::pin(async move {
                let Some(loaded) = service.load_session(&session_id, true).await? else {
                    return Ok(MessageFeedbackResult::rejected(
                        MessageFeedbackDeleteFailure::SessionNotFound { session_id },
                    ));
                };
                let result = async {
                    let items = current_items(&session_id, &loaded.events)?;
                    let existing = items
                        .iter()
                        .find(|item| item.message_id == request.message_id);
                    if let Some(existing) = existing {
                        if existing.version != request.if_version {
                            return Ok(MessageFeedbackResult::rejected(
                                MessageFeedbackDeleteFailure::VersionConflict {
                                    current: Some(existing.clone()),
                                },
                            ));
                        }
                        loaded
                            .commit(Some(Mutation::Delete(MessageFeedbackDelete {
                                session_id: session_id.clone(),
                                message_id: request.message_id,
                            })))
                            .await?;
                    } else {
                        loaded.commit(None).await?;
                    }
                    Ok(MessageFeedbackResult::success(MessageFeedbackDeleteValue {
                        absent: LiteralTrue,
                    }))
                }
                .await;
                loaded.finish(result).await
            })
        })
    }

    fn resolve_note(
        &self,
        note: Option<&str>,
    ) -> Result<Option<String>, MessageFeedbackPutFailure> {
        let Some(note) = note else {
            return Ok(None);
        };
        if js_trim(note).is_empty() {
            return Err(MessageFeedbackPutFailure::NoteBlank);
        }
        let actual_bytes = note.len();
        if actual_bytes > self.0.max_note_bytes {
            return Err(MessageFeedbackPutFailure::NoteTooLarge {
                max_bytes: self.0.max_note_bytes,
                actual_bytes,
            });
        }
        Ok(Some(note.to_owned()))
    }

    pub async fn close(&self) {
        {
            let mut state = lock(&self.0.operations);
            state.admission_open = false;
        }
        loop {
            let notified = self.0.drained.notified();
            if lock(&self.0.operations).active == 0 {
                return;
            }
            notified.await;
        }
    }
}

async fn close_on_error<T>(
    handle: Arc<dyn SessionHandle>,
    result: Result<T, BoxError>,
) -> Result<T, BoxError> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => close_overrides(handle, Err(error)).await,
    }
}

#[derive(Inject)]
pub struct Deps {
    sessions: Service<Sessions>,
    session_persistence: Service<SessionPersistenceService>,
}

pub struct MessageFeedbackPlugin;

impl Plugin for MessageFeedbackPlugin {
    const NAME: &'static str = "message-feedback";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let service = MessageFeedbackService::new(
            ctx.clone(),
            deps.sessions,
            deps.session_persistence,
            config.get().resolve()?,
        );
        let drain = service.clone();
        ctx.effect("message-feedback.drain", move || {
            Undo::future(move || async move {
                drain.close().await;
                Ok(())
            })
        })?;
        ctx.provide::<MessageFeedback>(Arc::new(service)).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::future::BoxFuture;
    use hoh_kernel::Kernel;
    use hoh_llm::{
        AssistantProviderMetadata, ContentBlock, MessageId, MessageSource, NewAssistantMessage,
        NewUserMessage, create_assistant_message, create_user_message,
    };
    use hoh_session::{
        CreateSessionOptions, SessionLogOffset, SessionSeedEventState, SessionStore, SurfaceIntent,
    };
    use hoh_session_persistence::{
        SessionHandleAppendOptions, SessionHandleFlushOptions, SessionHandleReadOptions,
        SessionHandleReadResult, SessionPersistence, SessionPersistenceCreateOptions,
        SessionPersistenceIdentity, SessionPersistenceListOptions, SessionPersistenceOpenOptions,
        SessionPersistenceRevision, SessionPersistenceSnapshot, SessionPersistenceStatOptions,
    };
    use hoh_session_persistence_jsonl::JsonlSessionPersistencePlugin;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use tempfile::TempDir;

    #[derive(Clone)]
    struct Stored {
        header: SessionHeader,
        events: Vec<Arc<SessionEvent>>,
    }

    struct TestPersistence {
        identity: SessionPersistenceIdentity,
        stored: Mutex<HashMap<SessionId, Stored>>,
        opens: Mutex<Vec<SessionAccess>>,
        append_calls: AtomicUsize,
        close_calls: AtomicUsize,
        stat_calls: AtomicUsize,
        read_calls: AtomicUsize,
        read_failure: Mutex<Option<&'static str>>,
        append_failure: Mutex<Option<&'static str>>,
        flush_failure: Mutex<Option<&'static str>>,
        close_failure: Mutex<Option<&'static str>>,
        block_stat: AtomicBool,
        stat_started: Notify,
        stat_release: Notify,
        block_read: AtomicBool,
        read_started: Notify,
        read_release: Notify,
        self_weak: Mutex<std::sync::Weak<TestPersistence>>,
    }

    impl TestPersistence {
        fn new() -> Arc<Self> {
            let persistence = Arc::new(Self {
                identity: SessionPersistenceIdentity::new(),
                stored: Mutex::new(HashMap::new()),
                opens: Mutex::new(Vec::new()),
                append_calls: AtomicUsize::new(0),
                close_calls: AtomicUsize::new(0),
                stat_calls: AtomicUsize::new(0),
                read_calls: AtomicUsize::new(0),
                read_failure: Mutex::new(None),
                append_failure: Mutex::new(None),
                flush_failure: Mutex::new(None),
                close_failure: Mutex::new(None),
                block_stat: AtomicBool::new(false),
                stat_started: Notify::new(),
                stat_release: Notify::new(),
                block_read: AtomicBool::new(false),
                read_started: Notify::new(),
                read_release: Notify::new(),
                self_weak: Mutex::new(std::sync::Weak::new()),
            });
            *lock(&persistence.self_weak) = Arc::downgrade(&persistence);
            persistence
        }

        fn persist(&self, session: &Session) {
            let header = serde_json::from_value((*session.header()).clone()).unwrap();
            lock(&self.stored).insert(
                session.id().clone(),
                Stored {
                    header,
                    events: session.snapshot_events(None, None).to_vec(),
                },
            );
        }

        fn events(&self, id: &SessionId) -> Vec<Arc<SessionEvent>> {
            lock(&self.stored).get(id).unwrap().events.clone()
        }
    }

    struct TestHandle {
        persistence: Arc<TestPersistence>,
        id: SessionId,
        header: SessionHeader,
        access: SessionAccess,
        closed: AtomicBool,
    }

    impl SessionHandle for TestHandle {
        fn id(&self) -> &SessionId {
            &self.id
        }

        fn header(&self) -> &SessionHeader {
            &self.header
        }

        fn inherited_event_count(&self) -> SessionLogOffset {
            SessionLogOffset::default()
        }

        fn access(&self) -> SessionAccess {
            self.access
        }

        fn read<'a>(
            &'a self,
            offset: Option<u64>,
            length: Option<u64>,
            _: Option<&'a SessionHandleReadOptions>,
        ) -> BoxFuture<'a, Result<SessionHandleReadResult, BoxError>> {
            Box::pin(async move {
                if self.closed.load(Ordering::Acquire) {
                    return Err("test handle is closed".into());
                }
                self.persistence
                    .read_calls
                    .fetch_add(1, AtomicOrdering::AcqRel);
                if self.persistence.block_read.swap(false, Ordering::AcqRel) {
                    self.persistence.read_started.notify_one();
                    self.persistence.read_release.notified().await;
                }
                if let Some(failure) = *lock(&self.persistence.read_failure) {
                    return Err(failure.into());
                }
                let stored = lock(&self.persistence.stored)
                    .get(&self.id)
                    .cloned()
                    .ok_or_else(|| -> BoxError { "test Session not found".into() })?;
                let start = usize::try_from(offset.unwrap_or(0)).unwrap_or(usize::MAX);
                let end = length
                    .and_then(|length| usize::try_from(length).ok())
                    .map_or(stored.events.len(), |length| start.saturating_add(length))
                    .min(stored.events.len());
                let start = start.min(end);
                Ok(SessionHandleReadResult {
                    event_state: SessionSeedEventState::Detached,
                    events: stored.events[start..end].to_vec(),
                })
            })
        }

        fn append<'a>(
            &'a self,
            events: &'a [Arc<SessionEvent>],
            _: Option<&'a SessionHandleAppendOptions>,
        ) -> BoxFuture<'a, Result<(), BoxError>> {
            Box::pin(async move {
                if self.access != SessionAccess::Write {
                    return Err("test handle is read-only".into());
                }
                if let Some(failure) = *lock(&self.persistence.append_failure) {
                    return Err(failure.into());
                }
                self.persistence
                    .append_calls
                    .fetch_add(1, AtomicOrdering::AcqRel);
                lock(&self.persistence.stored)
                    .get_mut(&self.id)
                    .unwrap()
                    .events
                    .extend_from_slice(events);
                Ok(())
            })
        }

        fn flush<'a>(
            &'a self,
            _: Option<&'a SessionHandleFlushOptions>,
        ) -> BoxFuture<'a, Result<(), BoxError>> {
            Box::pin(async move {
                if let Some(failure) = *lock(&self.persistence.flush_failure) {
                    return Err(failure.into());
                }
                Ok(())
            })
        }

        fn close(&self) -> BoxFuture<'_, Result<(), BoxError>> {
            Box::pin(async move {
                self.closed.store(true, Ordering::Release);
                self.persistence
                    .close_calls
                    .fetch_add(1, AtomicOrdering::AcqRel);
                if let Some(failure) = *lock(&self.persistence.close_failure) {
                    return Err(failure.into());
                }
                Ok(())
            })
        }
    }

    impl SessionPersistence for TestPersistence {
        fn identity(&self) -> SessionPersistenceIdentity {
            self.identity.clone()
        }

        fn create<'a>(
            &'a self,
            header: &'a SessionHeader,
            _: Option<&'a SessionPersistenceCreateOptions>,
        ) -> BoxFuture<'a, Result<Arc<dyn SessionHandle>, BoxError>> {
            let header = header.clone();
            Box::pin(async move {
                lock(&self.stored).insert(
                    header.id.clone(),
                    Stored {
                        header: header.clone(),
                        events: Vec::new(),
                    },
                );
                Ok(Arc::new(TestHandle {
                    persistence: lock(&self.self_weak)
                        .upgrade()
                        .expect("test persistence is Arc-owned"),
                    id: header.id.clone(),
                    header,
                    access: SessionAccess::Write,
                    closed: AtomicBool::new(false),
                }) as Arc<dyn SessionHandle>)
            })
        }

        fn open<'a>(
            &'a self,
            id: &'a SessionId,
            access: SessionAccess,
            _: Option<&'a SessionPersistenceOpenOptions>,
        ) -> BoxFuture<'a, Result<Arc<dyn SessionHandle>, BoxError>> {
            let id = id.clone();
            Box::pin(async move {
                lock(&self.opens).push(access);
                let header = lock(&self.stored)
                    .get(&id)
                    .map(|stored| stored.header.clone())
                    .ok_or_else(|| -> BoxError { "test Session not found".into() })?;
                Ok(Arc::new(TestHandle {
                    persistence: lock(&self.self_weak)
                        .upgrade()
                        .expect("test persistence is Arc-owned"),
                    id,
                    header,
                    access,
                    closed: AtomicBool::new(false),
                }) as Arc<dyn SessionHandle>)
            })
        }

        fn flush(&self) -> BoxFuture<'_, Result<(), BoxError>> {
            Box::pin(async { Ok(()) })
        }

        fn stat<'a>(
            &'a self,
            id: &'a SessionId,
            _: Option<&'a SessionPersistenceStatOptions>,
        ) -> BoxFuture<'a, Result<Option<SessionPersistenceSnapshot>, BoxError>> {
            let id = id.clone();
            Box::pin(async move {
                self.stat_calls.fetch_add(1, AtomicOrdering::AcqRel);
                if self.block_stat.swap(false, Ordering::AcqRel) {
                    self.stat_started.notify_one();
                    self.stat_release.notified().await;
                }
                Ok(lock(&self.stored)
                    .get(&id)
                    .map(|stored| SessionPersistenceSnapshot {
                        header: stored.header.clone(),
                        format_status: None,
                        revision: SessionPersistenceRevision::from("test"),
                        event_count: Some(stored.events.len() as u64),
                        size_bytes: None,
                        location: None,
                    }))
            })
        }

        fn list<'a>(
            &'a self,
            _: Option<&'a SessionPersistenceListOptions>,
        ) -> BoxFuture<'a, Result<Vec<SessionPersistenceSnapshot>, BoxError>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    struct Fixture {
        kernel: Kernel,
        ctx: Context,
        persistence: Arc<TestPersistence>,
        service: MessageFeedbackService,
    }

    impl Fixture {
        async fn new(max_note_bytes: f64) -> Self {
            let kernel = Kernel::start();
            let ctx = kernel.root();
            ctx.plugin::<hoh_session::SessionPlugin>(json!({}))
                .unwrap()
                .settled()
                .await
                .unwrap();
            let persistence = TestPersistence::new();
            ctx.provide::<SessionPersistenceService>(
                persistence.clone() as Arc<dyn SessionPersistence>
            )
            .await
            .unwrap();
            ctx.plugin::<MessageFeedbackPlugin>(json!({"maxNoteBytes":max_note_bytes}))
                .unwrap()
                .settled()
                .await
                .unwrap();
            let service = (*ctx.get::<MessageFeedback>().unwrap()).clone();
            Self {
                kernel,
                ctx,
                persistence,
                service,
            }
        }

        async fn finish(self) {
            self.kernel.shutdown().await;
        }
    }

    struct Messages {
        first: MessageId,
        second: MessageId,
        empty: MessageId,
        user: MessageId,
    }

    fn append_messages(session: &Session) -> Messages {
        session
            .append("turn/start", json!({"turn":1}), None)
            .unwrap();
        session
            .append("step/start", json!({"turn":1,"step":1}), None)
            .unwrap();
        let user = create_user_message(NewUserMessage {
            content: vec![ContentBlock::text("Question")],
            source: MessageSource::new("user"),
        });
        session
            .append(
                "user/message",
                serde_json::to_value(&user).unwrap(),
                Some(SurfaceIntent::append()),
            )
            .unwrap();
        let assistant = |content| {
            create_assistant_message(NewAssistantMessage {
                content,
                source: AssistantProviderMetadata {
                    provider: "test".into(),
                    model: "test".into(),
                    replay_state: None,
                },
            })
        };
        let first = assistant(vec![ContentBlock::text("First")]);
        let second = assistant(vec![ContentBlock::text("Second")]);
        let empty = assistant(Vec::new());
        for message in [&first, &second, &empty] {
            session
                .append(
                    "assistant/message",
                    json!({"stream":[],"turn":1,"step":1,"message":message}),
                    Some(SurfaceIntent::append()),
                )
                .unwrap();
        }
        Messages {
            first: first.id,
            second: second.id,
            empty: empty.id,
            user: user.id,
        }
    }

    fn cold_session(id: &str) -> (Session, Messages) {
        let session = Session::create(id.into(), None, None, None, None).unwrap();
        let messages = append_messages(&session);
        (session, messages)
    }

    fn version() -> MessageFeedbackVersion {
        MessageFeedbackVersion::from(hoh_util_crypto::random_uuid().as_str())
    }

    fn item(result: MessageFeedbackPutResult) -> MessageFeedbackItem {
        match result {
            MessageFeedbackResult::Success(MessageFeedbackSuccess { value, .. }) => value,
            other => panic!("expected item, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn missing_sessions_and_target_validation_match_the_public_failures() {
        let fixture = Fixture::new(64.0).await;
        let missing = SessionId::from("missing");
        assert_eq!(
            fixture
                .service
                .list(MessageFeedbackListRequest {
                    session_id: missing.clone(),
                })
                .await
                .unwrap(),
            MessageFeedbackResult::rejected(MessageFeedbackListFailure::SessionNotFound {
                session_id: missing.clone(),
            })
        );
        assert_eq!(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    session_id: missing.clone(),
                    message_id: MessageId::from("missing-message"),
                    rating: MessageFeedbackRating::Positive,
                    note: None,
                    category: None,
                    if_version: None,
                })
                .await
                .unwrap(),
            MessageFeedbackResult::rejected(MessageFeedbackPutFailure::SessionNotFound {
                session_id: missing.clone(),
            })
        );
        assert_eq!(
            fixture
                .service
                .delete(MessageFeedbackDeleteRequest {
                    session_id: missing.clone(),
                    message_id: MessageId::from("missing-message"),
                    if_version: version(),
                })
                .await
                .unwrap(),
            MessageFeedbackResult::rejected(MessageFeedbackDeleteFailure::SessionNotFound {
                session_id: missing,
            })
        );
        let (session, messages) = cold_session("targets");
        fixture.persistence.persist(&session);
        assert_eq!(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    session_id: session.id().clone(),
                    message_id: messages.first.clone(),
                    rating: MessageFeedbackRating::Positive,
                    note: None,
                    category: None,
                    if_version: Some(version()),
                })
                .await
                .unwrap(),
            MessageFeedbackResult::rejected(MessageFeedbackPutFailure::VersionConflict {
                current: None,
            })
        );
        for message_id in [messages.user, messages.empty] {
            assert_eq!(
                fixture
                    .service
                    .put(MessageFeedbackPutRequest {
                        session_id: session.id().clone(),
                        message_id: message_id.clone(),
                        rating: MessageFeedbackRating::Positive,
                        note: None,
                        category: None,
                        if_version: None,
                    })
                    .await
                    .unwrap(),
                MessageFeedbackResult::rejected(MessageFeedbackPutFailure::TargetNotFound {
                    session_id: session.id().clone(),
                    message_id,
                })
            );
        }
        fixture.finish().await;
    }

    #[tokio::test]
    async fn different_messages_keep_independent_versions_and_first_creation_order() {
        let fixture = Fixture::new(64.0).await;
        let (session, messages) = cold_session("independent-items");
        fixture.persistence.persist(&session);
        let make = |message_id, rating| MessageFeedbackPutRequest {
            session_id: session.id().clone(),
            message_id,
            rating,
            note: None,
            category: None,
            if_version: None,
        };
        let first_future = fixture.service.put(make(
            messages.first.clone(),
            MessageFeedbackRating::Positive,
        ));
        let second_future = fixture.service.put(make(
            messages.second.clone(),
            MessageFeedbackRating::Negative,
        ));
        let first = item(first_future.await.unwrap());
        let second = item(second_future.await.unwrap());
        let updated = item(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    session_id: session.id().clone(),
                    message_id: messages.first,
                    rating: MessageFeedbackRating::Negative,
                    note: Some("changed".into()),
                    category: None,
                    if_version: Some(first.version),
                })
                .await
                .unwrap(),
        );
        assert_eq!(
            fixture
                .service
                .list(MessageFeedbackListRequest {
                    session_id: session.id().clone(),
                })
                .await
                .unwrap(),
            MessageFeedbackResult::success(MessageFeedbackListValue {
                items: vec![updated, second]
            })
        );
        fixture.finish().await;
    }

    #[tokio::test]
    async fn live_ownership_wins_a_race_with_a_cold_catalog_miss() {
        let fixture = Fixture::new(64.0).await;
        let id = SessionId::from("live-race");
        fixture
            .persistence
            .block_stat
            .store(true, Ordering::Release);
        let listed = fixture.service.list(MessageFeedbackListRequest {
            session_id: id.clone(),
        });
        fixture.persistence.stat_started.notified().await;
        let session = fixture
            .ctx
            .get::<Sessions>()
            .unwrap()
            .create(&fixture.ctx, Some(id), CreateSessionOptions::default())
            .unwrap();
        fixture.persistence.stat_release.notify_one();
        assert_eq!(
            listed.await.unwrap(),
            MessageFeedbackResult::success(MessageFeedbackListValue { items: vec![] })
        );
        drop(session);
        fixture.finish().await;
    }

    #[tokio::test]
    async fn create_update_noop_category_and_clock_rollback_preserve_contract() {
        let fixture = Fixture::new(64.0).await;
        let (session, messages) = cold_session("updates");
        fixture.persistence.persist(&session);
        *lock(&fixture.service.0.now) = Arc::new(|| 1_700_000_001_000);
        let created = item(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    session_id: session.id().clone(),
                    message_id: messages.first.clone(),
                    rating: MessageFeedbackRating::Positive,
                    note: Some("  exact prose  ".into()),
                    category: Some(FeedbackCategory::TaskResult),
                    if_version: None,
                })
                .await
                .unwrap(),
        );
        *lock(&fixture.service.0.now) = Arc::new(|| 1_700_000_000_000);
        let updated = item(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    session_id: session.id().clone(),
                    message_id: messages.first.clone(),
                    rating: MessageFeedbackRating::Negative,
                    note: None,
                    category: Some(FeedbackCategory::Other),
                    if_version: Some(created.version.clone()),
                })
                .await
                .unwrap(),
        );
        assert_eq!(updated.created_at, created.created_at);
        assert_eq!(updated.updated_at, created.updated_at);
        assert_ne!(updated.version, created.version);
        let before = fixture
            .persistence
            .append_calls
            .load(AtomicOrdering::Acquire);
        let retry = item(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    session_id: session.id().clone(),
                    message_id: messages.first,
                    rating: MessageFeedbackRating::Negative,
                    note: None,
                    category: Some(FeedbackCategory::Other),
                    if_version: Some(updated.version.clone()),
                })
                .await
                .unwrap(),
        );
        assert_eq!(retry, updated);
        assert_eq!(
            fixture
                .persistence
                .append_calls
                .load(AtomicOrdering::Acquire),
            before
        );
        fixture.finish().await;
    }

    #[tokio::test]
    async fn note_validation_uses_complete_utf8_bytes_without_persistence() {
        let fixture = Fixture::new(4.0).await;
        let (session, messages) = cold_session("notes");
        fixture.persistence.persist(&session);
        let before = fixture.persistence.stat_calls.load(AtomicOrdering::Acquire);
        let base = MessageFeedbackPutRequest {
            session_id: session.id().clone(),
            message_id: messages.first.clone(),
            rating: MessageFeedbackRating::Positive,
            note: Some(" \n\t ".into()),
            category: None,
            if_version: None,
        };
        assert_eq!(
            fixture.service.put(base.clone()).await.unwrap(),
            MessageFeedbackResult::rejected(MessageFeedbackPutFailure::NoteBlank)
        );
        assert_eq!(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    note: Some("ééé".into()),
                    ..base.clone()
                })
                .await
                .unwrap(),
            MessageFeedbackResult::rejected(MessageFeedbackPutFailure::NoteTooLarge {
                max_bytes: 4,
                actual_bytes: 6,
            })
        );
        assert_eq!(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    note: Some("\u{feff}".into()),
                    ..base.clone()
                })
                .await
                .unwrap(),
            MessageFeedbackResult::rejected(MessageFeedbackPutFailure::NoteBlank)
        );
        assert_eq!(
            fixture.persistence.stat_calls.load(AtomicOrdering::Acquire),
            before
        );
        let c1_control = item(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    message_id: messages.second,
                    note: Some("\u{85}".into()),
                    ..base.clone()
                })
                .await
                .unwrap(),
        );
        assert_eq!(c1_control.note.as_deref(), Some("\u{85}"));
        item(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    note: Some("😀".into()),
                    ..base
                })
                .await
                .unwrap(),
        );
        fixture.finish().await;
    }

    #[tokio::test]
    async fn concurrent_creates_and_aba_are_guarded_by_versions() {
        let fixture = Fixture::new(64.0).await;
        let (session, messages) = cold_session("concurrency");
        fixture.persistence.persist(&session);
        let request = MessageFeedbackPutRequest {
            session_id: session.id().clone(),
            message_id: messages.first.clone(),
            rating: MessageFeedbackRating::Positive,
            note: None,
            category: None,
            if_version: None,
        };
        let first_future = fixture.service.put(request.clone());
        let second_future = fixture.service.put(request.clone());
        let first = item(first_future.await.unwrap());
        assert_eq!(
            second_future.await.unwrap(),
            MessageFeedbackResult::rejected(MessageFeedbackPutFailure::VersionConflict {
                current: Some(first.clone()),
            })
        );
        let second = item(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    rating: MessageFeedbackRating::Negative,
                    if_version: Some(first.version.clone()),
                    ..request.clone()
                })
                .await
                .unwrap(),
        );
        let current = item(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    if_version: Some(second.version),
                    ..request.clone()
                })
                .await
                .unwrap(),
        );
        assert_eq!(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    if_version: Some(first.version),
                    ..request
                })
                .await
                .unwrap(),
            MessageFeedbackResult::rejected(MessageFeedbackPutFailure::VersionConflict {
                current: Some(current),
            })
        );
        fixture.finish().await;
    }

    #[tokio::test]
    async fn delete_is_idempotent_and_prevents_delete_recreate_aba() {
        let fixture = Fixture::new(64.0).await;
        let (session, messages) = cold_session("delete-aba");
        fixture.persistence.persist(&session);
        let created = item(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    session_id: session.id().clone(),
                    message_id: messages.first.clone(),
                    rating: MessageFeedbackRating::Positive,
                    note: None,
                    category: None,
                    if_version: None,
                })
                .await
                .unwrap(),
        );
        let delete = MessageFeedbackDeleteRequest {
            session_id: session.id().clone(),
            message_id: messages.first.clone(),
            if_version: created.version.clone(),
        };
        let absent = MessageFeedbackResult::success(MessageFeedbackDeleteValue {
            absent: LiteralTrue,
        });
        assert_eq!(
            fixture.service.delete(delete.clone()).await.unwrap(),
            absent
        );
        assert_eq!(
            fixture.service.delete(delete.clone()).await.unwrap(),
            absent
        );
        let recreated = item(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    session_id: session.id().clone(),
                    message_id: messages.first,
                    rating: MessageFeedbackRating::Negative,
                    note: None,
                    category: None,
                    if_version: None,
                })
                .await
                .unwrap(),
        );
        assert_eq!(
            fixture.service.delete(delete).await.unwrap(),
            MessageFeedbackResult::rejected(MessageFeedbackDeleteFailure::VersionConflict {
                current: Some(recreated),
            })
        );
        fixture.finish().await;
    }

    #[tokio::test]
    async fn cold_history_contains_only_material_mutations_and_observer_failures_are_warnings() {
        let fixture = Fixture::new(64.0).await;
        let (session, messages) = cold_session("cold-log");
        fixture.persistence.persist(&session);
        let prefix = fixture.persistence.events(session.id()).len();
        let observed = Arc::new(AtomicUsize::new(0));
        let count = observed.clone();
        let _listener = fixture
            .ctx
            .on_parallel::<FeedbackCommitted, _>(move |_| {
                count.fetch_add(1, AtomicOrdering::AcqRel);
                async { Err("observer failed".into()) }
            })
            .unwrap();
        let created = item(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    session_id: session.id().clone(),
                    message_id: messages.first.clone(),
                    rating: MessageFeedbackRating::Positive,
                    note: None,
                    category: None,
                    if_version: None,
                })
                .await
                .unwrap(),
        );
        item(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    session_id: session.id().clone(),
                    message_id: messages.first.clone(),
                    rating: MessageFeedbackRating::Positive,
                    note: None,
                    category: None,
                    if_version: Some(created.version.clone()),
                })
                .await
                .unwrap(),
        );
        fixture
            .service
            .delete(MessageFeedbackDeleteRequest {
                session_id: session.id().clone(),
                message_id: messages.first,
                if_version: created.version,
            })
            .await
            .unwrap();
        let events = fixture.persistence.events(session.id());
        let kinds: Vec<_> = events[prefix..]
            .iter()
            .map(|event| event.event_type())
            .collect();
        assert_eq!(kinds, ["feedback/message-put", "feedback/message-delete"]);
        assert_eq!(observed.load(AtomicOrdering::Acquire), 2);
        assert_eq!(
            *lock(&fixture.persistence.opens),
            [
                SessionAccess::Write,
                SessionAccess::Write,
                SessionAccess::Write
            ]
        );
        assert_eq!(
            fixture
                .persistence
                .close_calls
                .load(AtomicOrdering::Acquire),
            3
        );
        assert!(
            fixture
                .ctx
                .get::<Sessions>()
                .unwrap()
                .get(session.id())
                .is_none()
        );
        assert!(fixture.ctx.logger().buffer().iter().any(|record| {
            record
                .message
                .contains("message-feedback: committed feedback observer failed")
        }));
        fixture.finish().await;
    }

    #[tokio::test]
    async fn live_flush_failure_retries_without_a_duplicate_event() {
        let fixture = Fixture::new(64.0).await;
        let sessions = fixture.ctx.get::<Sessions>().unwrap();
        let session = sessions
            .create(
                &fixture.ctx,
                Some(SessionId::from("live")),
                CreateSessionOptions::default(),
            )
            .unwrap();
        let messages = append_messages(&session);
        let fail = Arc::new(AtomicBool::new(true));
        let failure = fail.clone();
        let persistence = fixture.persistence.clone();
        let _flush = SessionStore::on_flush(&fixture.ctx, move |session| {
            let failure = failure.clone();
            let persistence = persistence.clone();
            async move {
                if failure.load(Ordering::Acquire) {
                    return Err("disk unavailable".into());
                }
                persistence.persist(&session);
                Ok(())
            }
        })
        .unwrap();
        let request = MessageFeedbackPutRequest {
            session_id: session.id().clone(),
            message_id: messages.first,
            rating: MessageFeedbackRating::Positive,
            note: None,
            category: None,
            if_version: None,
        };
        assert_eq!(
            fixture
                .service
                .put(request.clone())
                .await
                .unwrap_err()
                .to_string(),
            "disk unavailable"
        );
        let listed = fixture
            .service
            .list(MessageFeedbackListRequest {
                session_id: session.id().clone(),
            })
            .await
            .unwrap();
        let current = match listed {
            MessageFeedbackResult::Success(MessageFeedbackSuccess { value, .. }) => {
                value.items[0].clone()
            }
            other => panic!("unexpected list: {other:?}"),
        };
        fail.store(false, Ordering::Release);
        item(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    if_version: Some(current.version),
                    ..request
                })
                .await
                .unwrap(),
        );
        assert_eq!(
            session
                .snapshot_events(None, None)
                .iter()
                .filter(|event| event.event_type() == "feedback/message-put")
                .count(),
            1
        );
        fixture.finish().await;
    }

    #[tokio::test]
    async fn live_writes_require_an_owner_and_verify_the_exact_durable_tail() {
        let fixture = Fixture::new(64.0).await;
        let sessions = fixture.ctx.get::<Sessions>().unwrap();
        let unowned = sessions
            .create(
                &fixture.ctx,
                Some(SessionId::from("unowned")),
                CreateSessionOptions::default(),
            )
            .unwrap();
        let unowned_messages = append_messages(&unowned);
        assert!(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    session_id: unowned.id().clone(),
                    message_id: unowned_messages.first,
                    rating: MessageFeedbackRating::Positive,
                    note: None,
                    category: None,
                    if_version: None,
                })
                .await
                .unwrap_err()
                .to_string()
                .contains("no durability listener participated")
        );

        let persistence = fixture.persistence.clone();
        let _flush = SessionStore::on_flush(&fixture.ctx, move |session| {
            let persistence = persistence.clone();
            async move {
                persistence.persist(&session);
                let mut stored = lock(&persistence.stored);
                let stored = stored.get_mut(session.id()).unwrap();
                match session.id().as_str() {
                    "missing-tail" => {
                        stored.events.pop();
                    }
                    "different-tail" => {
                        let last = stored.events.last_mut().unwrap();
                        let mut value = last.as_value().clone();
                        value["time"] = json!(0);
                        *last = Arc::new(SessionEvent::from_value(value));
                    }
                    "different-lifecycle" => stored.header.created_at = 0,
                    _ => unreachable!(),
                }
                Ok(())
            }
        })
        .unwrap();
        for id in ["missing-tail", "different-tail", "different-lifecycle"] {
            let mismatched = sessions
                .create(
                    &fixture.ctx,
                    Some(SessionId::from(id)),
                    CreateSessionOptions::default(),
                )
                .unwrap();
            let messages = append_messages(&mismatched);
            assert!(
                fixture
                    .service
                    .put(MessageFeedbackPutRequest {
                        session_id: mismatched.id().clone(),
                        message_id: messages.first,
                        rating: MessageFeedbackRating::Positive,
                        note: None,
                        category: None,
                        if_version: None,
                    })
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("feedback prefix is not durable")
            );
        }
        fixture.finish().await;
    }

    #[tokio::test]
    async fn live_noop_verifies_the_flushed_checkpoint_before_concurrent_appends() {
        let fixture = Fixture::new(64.0).await;
        let sessions = fixture.ctx.get::<Sessions>().unwrap();
        let session = sessions
            .create(
                &fixture.ctx,
                Some(SessionId::from("checkpoint-prefix")),
                CreateSessionOptions::default(),
            )
            .unwrap();
        let persistence = fixture.persistence.clone();
        let _flush = SessionStore::on_flush(&fixture.ctx, move |session| {
            let persistence = persistence.clone();
            async move {
                persistence.persist(&session);
                Ok(())
            }
        })
        .unwrap();
        fixture
            .persistence
            .block_read
            .store(true, Ordering::Release);
        let deletion = fixture.service.delete(MessageFeedbackDeleteRequest {
            session_id: session.id().clone(),
            message_id: MessageId::from("absent"),
            if_version: version(),
        });
        fixture.persistence.read_started.notified().await;
        session
            .append("turn/start", json!({"turn":2}), None)
            .unwrap();
        fixture.persistence.read_release.notify_one();
        assert_eq!(
            deletion.await.unwrap(),
            MessageFeedbackResult::success(MessageFeedbackDeleteValue {
                absent: LiteralTrue,
            })
        );
        assert_eq!(fixture.persistence.events(session.id()).len(), 0);
        assert_eq!(session.snapshot_events(None, None).len(), 1);
        fixture.finish().await;
    }

    #[tokio::test]
    async fn inherited_parent_feedback_is_not_owned_by_a_fork() {
        let fixture = Fixture::new(64.0).await;
        let (parent, messages) = cold_session("parent");
        fixture.persistence.persist(&parent);
        let parent_item = item(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    session_id: parent.id().clone(),
                    message_id: messages.first.clone(),
                    rating: MessageFeedbackRating::Positive,
                    note: None,
                    category: None,
                    if_version: None,
                })
                .await
                .unwrap(),
        );
        let seed = fixture.persistence.events(parent.id());
        let child_id = SessionId::from("child");
        let child = Session::create(
            child_id.clone(),
            Some(&seed),
            Some(json!({
                "version": hoh_session::SESSION_FORMAT_VERSION,
                "id": child_id,
                "createdAt": 2,
                "isSeeded": true,
                "parentSession": parent.id(),
            })),
            Some(SessionLogOffset::new(seed.len() as u64).unwrap()),
            None,
        )
        .unwrap();
        fixture.persistence.persist(&child);
        assert_eq!(
            fixture
                .service
                .list(MessageFeedbackListRequest {
                    session_id: child.id().clone(),
                })
                .await
                .unwrap(),
            MessageFeedbackResult::success(MessageFeedbackListValue { items: vec![] })
        );
        let child_item = item(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    session_id: child.id().clone(),
                    message_id: messages.first.clone(),
                    rating: MessageFeedbackRating::Negative,
                    note: None,
                    category: None,
                    if_version: None,
                })
                .await
                .unwrap(),
        );
        fixture
            .service
            .delete(MessageFeedbackDeleteRequest {
                session_id: parent.id().clone(),
                message_id: messages.first,
                if_version: parent_item.version,
            })
            .await
            .unwrap();
        assert_eq!(
            fixture
                .service
                .list(MessageFeedbackListRequest {
                    session_id: child.id().clone(),
                })
                .await
                .unwrap(),
            MessageFeedbackResult::success(MessageFeedbackListValue {
                items: vec![child_item]
            })
        );
        fixture.finish().await;
    }

    #[tokio::test]
    async fn replacing_a_lifecycle_without_feedback_starts_clean() {
        let fixture = Fixture::new(64.0).await;
        let (session, messages) = cold_session("replacement");
        let clean_events = session.snapshot_events(None, None).to_vec();
        fixture.persistence.persist(&session);
        let old = item(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    session_id: session.id().clone(),
                    message_id: messages.first.clone(),
                    rating: MessageFeedbackRating::Positive,
                    note: None,
                    category: None,
                    if_version: None,
                })
                .await
                .unwrap(),
        );
        let mut header: SessionHeader =
            serde_json::from_value((*session.header()).clone()).unwrap();
        header.created_at += 1;
        lock(&fixture.persistence.stored).insert(
            session.id().clone(),
            Stored {
                header,
                events: clean_events,
            },
        );
        assert_eq!(
            fixture
                .service
                .list(MessageFeedbackListRequest {
                    session_id: session.id().clone(),
                })
                .await
                .unwrap(),
            MessageFeedbackResult::success(MessageFeedbackListValue { items: vec![] })
        );
        let recreated = item(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    session_id: session.id().clone(),
                    message_id: messages.first,
                    rating: MessageFeedbackRating::Negative,
                    note: None,
                    category: None,
                    if_version: None,
                })
                .await
                .unwrap(),
        );
        assert_ne!(recreated.version, old.version);
        fixture.finish().await;
    }

    #[tokio::test]
    async fn close_drains_two_admitted_operations_and_rejects_late_admission() {
        let fixture = Fixture::new(64.0).await;
        let (session, messages) = cold_session("drain");
        fixture.persistence.persist(&session);
        fixture
            .persistence
            .block_read
            .store(true, Ordering::Release);
        let first = fixture.service.put(MessageFeedbackPutRequest {
            session_id: session.id().clone(),
            message_id: messages.first,
            rating: MessageFeedbackRating::Positive,
            note: None,
            category: None,
            if_version: None,
        });
        fixture.persistence.read_started.notified().await;
        let second = fixture.service.put(MessageFeedbackPutRequest {
            session_id: session.id().clone(),
            message_id: messages.second,
            rating: MessageFeedbackRating::Negative,
            note: None,
            category: None,
            if_version: None,
        });
        let service = fixture.service.clone();
        let closing = tokio::spawn(async move { service.close().await });
        tokio::task::yield_now().await;
        assert!(!closing.is_finished());
        assert_eq!(
            fixture
                .service
                .delete(MessageFeedbackDeleteRequest {
                    session_id: session.id().clone(),
                    message_id: MessageId::from("anything"),
                    if_version: version(),
                })
                .await
                .unwrap_err()
                .to_string(),
            "message-feedback: service is disposing"
        );
        fixture.persistence.read_release.notify_one();
        assert_eq!(
            item(first.await.unwrap()).rating,
            MessageFeedbackRating::Positive
        );
        assert_eq!(
            item(second.await.unwrap()).rating,
            MessageFeedbackRating::Negative
        );
        closing.await.unwrap();
        assert_eq!(
            fixture
                .persistence
                .events(session.id())
                .iter()
                .filter(|event| event.event_type() == "feedback/message-put")
                .count(),
            2
        );
        assert_eq!(
            fixture.persistence.read_calls.load(AtomicOrdering::Acquire),
            2
        );
        assert_eq!(
            fixture
                .persistence
                .append_calls
                .load(AtomicOrdering::Acquire),
            2
        );
        assert_eq!(
            fixture
                .persistence
                .close_calls
                .load(AtomicOrdering::Acquire),
            2
        );
        fixture.finish().await;
    }

    #[tokio::test]
    async fn durable_items_preserve_extensions_and_js_number_semantics() {
        let fixture = Fixture::new(64.0).await;
        for (index, (created_at, updated_at, accepted_version)) in [
            ("1.0", "1", version().to_string()),
            ("1", "1.0", version().to_string()),
            ("1e0", "1", version().to_string()),
            ("1", "1e0", version().to_string()),
            ("1", "1", "ffffffff-ffff-ffff-ffff-ffffffffffff".into()),
        ]
        .into_iter()
        .enumerate()
        {
            let (session, messages) = cold_session(&format!("js-number-{index}"));
            let data: serde_json::Value = serde_json::from_str(&format!(
                r#"{{"sessionId":"{}","item":{{"messageId":"{}","rating":"positive","note":"\u0085","version":"{}","createdAt":{created_at},"updatedAt":{updated_at},"futureField":{{"x":1}}}}}}"#,
                session.id(),
                messages.first,
                accepted_version,
            ))
            .unwrap();
            session.append("feedback/message-put", data, None).unwrap();
            fixture.persistence.persist(&session);

            let result = fixture
                .service
                .list(MessageFeedbackListRequest {
                    session_id: session.id().clone(),
                })
                .await
                .unwrap();
            let MessageFeedbackResult::Success(MessageFeedbackSuccess { value, .. }) = result
            else {
                panic!("expected durable feedback to be accepted");
            };
            assert_eq!(value.items.len(), 1);
            assert_eq!(value.items[0].created_at, 1);
            assert_eq!(value.items[0].updated_at, 1);
            assert_eq!(value.items[0].version.as_str(), accepted_version);
            assert_eq!(value.items[0].note.as_deref(), Some("\u{85}"));
            assert_eq!(value.items[0].extensions["futureField"], json!({"x":1}));
            assert_eq!(
                serde_json::to_value(&value.items[0]).unwrap()["futureField"],
                json!({"x":1})
            );
        }
        fixture.finish().await;
    }

    #[test]
    fn publishes_all_remote_method_markers() {
        let source = include_str!("service.rs");
        for declaration in [
            concat!("/// Remote: messageFeedback.", "list\n    pub fn list("),
            concat!("/// Remote: messageFeedback.", "put\n    pub fn put("),
            concat!("/// Remote: messageFeedback.", "delete\n    pub fn delete("),
        ] {
            assert!(source.contains(declaration), "missing {declaration}");
        }
    }

    #[tokio::test]
    async fn malformed_persisted_feedback_and_js_unsafe_timestamps_are_rejected() {
        let fixture = Fixture::new(64.0).await;
        let item = |patch: serde_json::Value| {
            let mut item = json!({
                "messageId":"message",
                "rating":"positive",
                "version":version(),
                "createdAt":1,
                "updatedAt":1,
            });
            item.as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            item
        };
        let records = vec![
            ("feedback/message-put", serde_json::Value::Null),
            ("feedback/message-put", json!({"sessionId":"x","item":{}})),
            (
                "feedback/message-put",
                json!({"sessionId":"x","item":item(json!({"messageId":""}))}),
            ),
            (
                "feedback/message-put",
                json!({"sessionId":"x","item":item(json!({"rating":"neutral"}))}),
            ),
            (
                "feedback/message-put",
                json!({"sessionId":"x","item":item(json!({"version":"bad-token"}))}),
            ),
            (
                "feedback/message-put",
                json!({"sessionId":"x","item":item(json!({
                    "version":"ffffffff-ffff-ffff-ffff-fffffffffff0"
                }))}),
            ),
            (
                "feedback/message-put",
                json!({"sessionId":"x","item":item(json!({
                    "version":"00000000-0000-0000-0000-100000000000"
                }))}),
            ),
            (
                "feedback/message-put",
                json!({"sessionId":"x","item":item(json!({"note":"  \t"}))}),
            ),
            (
                "feedback/message-put",
                json!({"sessionId":"x","item":item(json!({"note":"\u{feff}"}))}),
            ),
            (
                "feedback/message-put",
                json!({"sessionId":"x","item":item(json!({"note":null}))}),
            ),
            (
                "feedback/message-put",
                json!({"sessionId":"x","item":item(json!({"category":null}))}),
            ),
            (
                "feedback/message-put",
                json!({"sessionId":"x","item":item(json!({"createdAt":-1}))}),
            ),
            (
                "feedback/message-put",
                json!({"sessionId":"x","item":item(json!({"updatedAt":0}))}),
            ),
            (
                "feedback/message-put",
                json!({"sessionId":"x","item":item(json!({"updatedAt":1.5}))}),
            ),
            (
                "feedback/message-delete",
                json!({"sessionId":"x","messageId":""}),
            ),
            (
                "feedback/message-delete",
                json!({"sessionId":12,"messageId":"message"}),
            ),
            (
                "feedback/message-put",
                json!({"sessionId":"x","item":item(json!({"category":"invalid"}))}),
            ),
            (
                "feedback/message-put",
                json!({"sessionId":"x","item":item(json!({"updatedAt":9_007_199_254_740_992_u64}))}),
            ),
        ];
        for (index, (event_type, data)) in records.into_iter().enumerate() {
            let (session, _) = cold_session(&format!("malformed-{index}"));
            session.append(event_type, data, None).unwrap();
            fixture.persistence.persist(&session);
            let closes = fixture
                .persistence
                .close_calls
                .load(AtomicOrdering::Acquire);
            assert!(
                fixture
                    .service
                    .list(MessageFeedbackListRequest {
                        session_id: session.id().clone(),
                    })
                    .await
                    .is_err()
            );
            assert_eq!(
                fixture
                    .persistence
                    .close_calls
                    .load(AtomicOrdering::Acquire),
                closes + 1,
                "malformed durable handle {index} was not closed"
            );
        }
        fixture.finish().await;
    }

    #[tokio::test]
    async fn close_error_overrides_an_earlier_cold_operation_error() {
        let fixture = Fixture::new(64.0).await;
        let (session, messages) = cold_session("close-order");
        fixture.persistence.persist(&session);
        *lock(&fixture.persistence.append_failure) = Some("append failed");
        *lock(&fixture.persistence.close_failure) = Some("close failed");
        assert_eq!(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    session_id: session.id().clone(),
                    message_id: messages.first,
                    rating: MessageFeedbackRating::Positive,
                    note: None,
                    category: None,
                    if_version: None,
                })
                .await
                .unwrap_err()
                .to_string(),
            "close failed"
        );
        fixture.finish().await;
    }

    #[tokio::test]
    async fn cold_read_and_flush_failures_pass_through_and_close_their_handles() {
        let fixture = Fixture::new(64.0).await;
        let (read_session, _) = cold_session("cold-read-failure");
        fixture.persistence.persist(&read_session);
        *lock(&fixture.persistence.read_failure) = Some("stored log checksum mismatch");
        assert_eq!(
            fixture
                .service
                .list(MessageFeedbackListRequest {
                    session_id: read_session.id().clone(),
                })
                .await
                .unwrap_err()
                .to_string(),
            "stored log checksum mismatch"
        );
        assert_eq!(
            fixture
                .persistence
                .close_calls
                .load(AtomicOrdering::Acquire),
            1
        );

        *lock(&fixture.persistence.read_failure) = None;
        let (flush_session, messages) = cold_session("cold-flush-failure");
        fixture.persistence.persist(&flush_session);
        *lock(&fixture.persistence.flush_failure) = Some("flush failed");
        assert_eq!(
            fixture
                .service
                .put(MessageFeedbackPutRequest {
                    session_id: flush_session.id().clone(),
                    message_id: messages.first,
                    rating: MessageFeedbackRating::Positive,
                    note: None,
                    category: None,
                    if_version: None,
                })
                .await
                .unwrap_err()
                .to_string(),
            "flush failed"
        );
        assert_eq!(
            fixture
                .persistence
                .close_calls
                .load(AtomicOrdering::Acquire),
            2
        );
        fixture.finish().await;
    }

    #[test]
    fn wire_literals_and_operation_specific_failure_unions_are_strict() {
        let listed: MessageFeedbackListResult =
            MessageFeedbackResult::success(MessageFeedbackListValue { items: Vec::new() });
        assert_eq!(
            serde_json::to_value(listed).unwrap(),
            json!({"ok":true,"value":{"items":[]}})
        );

        let deleted: MessageFeedbackDeleteResult =
            MessageFeedbackResult::success(MessageFeedbackDeleteValue {
                absent: LiteralTrue,
            });
        assert_eq!(
            serde_json::to_value(deleted).unwrap(),
            json!({"ok":true,"value":{"absent":true}})
        );

        for invalid in [
            json!({"ok":false,"value":{"items":[]}}),
            json!({"ok":true,"error":{"code":"session-not-found","sessionId":"x"}}),
            json!({"ok":false,"error":{"code":"note-blank"}}),
        ] {
            assert!(serde_json::from_value::<MessageFeedbackListResult>(invalid).is_err());
        }
        assert!(
            serde_json::from_value::<MessageFeedbackDeleteResult>(
                json!({"ok":true,"value":{"absent":false}})
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn invalid_configuration_is_rejected() {
        for value in [0.0, -1.0, 1.5, f64::INFINITY, 9_007_199_254_740_992.0] {
            assert_eq!(
                Config {
                    max_note_bytes: value
                }
                .resolve()
                .unwrap_err()
                .to_string(),
                "message-feedback: maxNoteBytes must be a positive safe integer"
            );
        }
    }

    #[tokio::test]
    async fn jsonl_composition_persists_feedback_across_restart() {
        let root = TempDir::new().unwrap();
        let session_id = SessionId::from("jsonl-feedback");
        let (saved, message_id, prefix_len) = {
            let kernel = Kernel::start();
            let ctx = kernel.root();
            ctx.plugin::<hoh_session::SessionPlugin>(json!({}))
                .unwrap()
                .settled()
                .await
                .unwrap();
            ctx.plugin::<JsonlSessionPersistencePlugin>(json!({
                "root":root.path().to_string_lossy(),
                "compression":"none"
            }))
            .unwrap()
            .settled()
            .await
            .unwrap();
            ctx.plugin::<MessageFeedbackPlugin>(json!({"maxNoteBytes":32}))
                .unwrap()
                .settled()
                .await
                .unwrap();
            let sessions = ctx.get::<Sessions>().unwrap();
            let session = sessions
                .create(
                    &ctx,
                    Some(session_id.clone()),
                    CreateSessionOptions::default(),
                )
                .unwrap();
            let writer = ctx
                .get::<SessionPersistenceService>()
                .unwrap()
                .create(
                    &serde_json::from_value((*session.header()).clone()).unwrap(),
                    None,
                )
                .await
                .unwrap();
            let messages = append_messages(&session);
            let service = ctx.get::<MessageFeedback>().unwrap();
            let item = item(
                service
                    .put(MessageFeedbackPutRequest {
                        session_id: session_id.clone(),
                        message_id: messages.first.clone(),
                        rating: MessageFeedbackRating::Positive,
                        note: Some("survives restart".into()),
                        category: None,
                        if_version: None,
                    })
                    .await
                    .unwrap(),
            );
            writer.close().await.unwrap();
            let reader = ctx
                .get::<SessionPersistenceService>()
                .unwrap()
                .open(&session_id, SessionAccess::Read, None)
                .await
                .unwrap();
            let prefix_len = reader.read(None, None, None).await.unwrap().events.len();
            reader.close().await.unwrap();
            kernel.shutdown().await;
            (item, messages.first, prefix_len)
        };

        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<hoh_session::SessionPlugin>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<JsonlSessionPersistencePlugin>(json!({
            "root":root.path().to_string_lossy(),
            "compression":"none"
        }))
        .unwrap()
        .settled()
        .await
        .unwrap();
        ctx.plugin::<MessageFeedbackPlugin>(json!({"maxNoteBytes":32}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let listed = ctx
            .get::<MessageFeedback>()
            .unwrap()
            .list(MessageFeedbackListRequest {
                session_id: session_id.clone(),
            })
            .await
            .unwrap();
        assert_eq!(
            listed,
            MessageFeedbackResult::success(MessageFeedbackListValue {
                items: vec![saved.clone()]
            })
        );
        assert!(!message_id.as_str().is_empty());
        let service = ctx.get::<MessageFeedback>().unwrap();
        let edited = item(
            service
                .put(MessageFeedbackPutRequest {
                    session_id: session_id.clone(),
                    message_id: message_id.clone(),
                    rating: MessageFeedbackRating::Negative,
                    note: Some("cold edit".into()),
                    category: None,
                    if_version: Some(saved.version),
                })
                .await
                .unwrap(),
        );
        service
            .delete(MessageFeedbackDeleteRequest {
                session_id: session_id.clone(),
                message_id,
                if_version: edited.version,
            })
            .await
            .unwrap();
        let reader = ctx
            .get::<SessionPersistenceService>()
            .unwrap()
            .open(&session_id, SessionAccess::Read, None)
            .await
            .unwrap();
        let events = reader.read(None, None, None).await.unwrap().events;
        reader.close().await.unwrap();
        assert_eq!(
            events[prefix_len..]
                .iter()
                .map(|event| event.event_type())
                .collect::<Vec<_>>(),
            ["feedback/message-put", "feedback/message-delete"]
        );
        assert!(ctx.get::<Sessions>().unwrap().get(&session_id).is_none());
        kernel.shutdown().await;
    }
}
