//! Owned handles, ordered mutation continuations, and routed live write batching.
use crate::{JsonlError, Result, error::error, io::lock, lease::SessionWriteLease};
use futures_util::future::BoxFuture;
use hoh_error::{BoxError, SharedError};
use hoh_session::{
    MAX_SAFE_INTEGER, SessionEvent, SessionHeader, SessionId, SessionLogOffset,
    SessionSeedEventState,
};
use hoh_session_persistence::*;
use hoh_timeout::AbortSignal;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tokio::sync::{oneshot, watch};

pub const LIVE_WRITE_BATCH_MAX_DELAY_MS: u64 = 200;
pub trait JsonlHandleStorage: Send + Sync + 'static {
    fn persist_batch<'a>(
        &'a self,
        header: &'a SessionHeader,
        events: &'a [Arc<SessionEvent>],
        materialized: bool,
        cut: SessionLogOffset,
    ) -> BoxFuture<'a, Result<()>>;
    fn persist_header<'a>(
        &'a self,
        header: &'a SessionHeader,
        cut: SessionLogOffset,
    ) -> BoxFuture<'a, Result<()>>;
    fn truncate_torn_tail<'a>(
        &'a self,
        header: &'a SessionHeader,
        offset: u64,
    ) -> BoxFuture<'a, Result<()>>;
    fn resolve_current_log<'a>(
        &'a self,
        id: &'a SessionId,
        signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<Option<String>>>;
    fn read_stored_log<'a>(
        &'a self,
        path: &'a str,
        id: &'a SessionId,
        signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<SessionHandleReadResult>>;
    fn has_pending_session(&self, id: &SessionId) -> bool;
    fn acquire_write_lease<'a>(
        &'a self,
        header: &'a SessionHeader,
    ) -> BoxFuture<'a, Result<Arc<SessionWriteLease>>>;
    fn release_handle(&self, handle: &JsonlSessionHandle, materialized: bool);
}
#[derive(Clone, Default)]
pub struct StorageHandleState {
    pub cursor: u64,
    pub materialized: bool,
    pub torn_truncate_to: Option<u64>,
    pub recovered_tail: Option<Vec<Arc<SessionEvent>>>,
    pub inherited_event_count: SessionLogOffset,
    pub primed: Option<SessionHandleReadResult>,
}
type SharedResult = std::result::Result<(), SharedError>;
type Completion = watch::Receiver<Option<SharedResult>>;
#[derive(Clone)]
pub struct JsonlSessionHandle(Arc<Handle>);
struct Handle {
    storage: Arc<dyn JsonlHandleStorage>,
    id: SessionId,
    header: SessionHeader,
    access: SessionAccess,
    cut: SessionLogOffset,
    state: Mutex<StorageHandleState>,
    lease: Mutex<Option<Arc<SessionWriteLease>>>,
    chain: Mutex<watch::Receiver<bool>>,
    closing: AtomicBool,
    close_result: Mutex<Option<Completion>>,
    observed: AtomicU64,
    buffer: Mutex<Vec<Arc<SessionEvent>>>,
    timer: Mutex<Option<tokio::task::JoinHandle<()>>>,
    drain_paused: AtomicBool,
    drain_result: Mutex<Option<Completion>>,
}
fn done_queue() -> watch::Receiver<bool> {
    watch::channel(true).1
}
fn ready_failure(failure: BoxError) -> BoxFuture<'static, Result<()>> {
    Box::pin(async move { Err(failure) })
}
async fn wait_completion(mut receiver: Completion) -> Result<()> {
    loop {
        let result = receiver.borrow().clone();
        if let Some(result) = result {
            return result.map_err(|e| Box::new(e) as BoxError);
        }
        if receiver.changed().await.is_err() {
            return Err(error("JSONL handle operation ended without a result"));
        }
    }
}
struct QueueDone(watch::Sender<bool>);
impl Drop for QueueDone {
    fn drop(&mut self) {
        let _ = self.0.send(true);
    }
}
impl JsonlSessionHandle {
    pub fn new(
        storage: Arc<dyn JsonlHandleStorage>,
        id: SessionId,
        header: SessionHeader,
        access: SessionAccess,
        state: StorageHandleState,
        lease: Option<Arc<SessionWriteLease>>,
    ) -> Self {
        Self(Arc::new(Handle {
            storage,
            id,
            header,
            access,
            cut: state.inherited_event_count,
            state: Mutex::new(state),
            lease: Mutex::new(lease),
            chain: Mutex::new(done_queue()),
            closing: AtomicBool::new(false),
            close_result: Mutex::new(None),
            observed: AtomicU64::new(0),
            buffer: Mutex::new(vec![]),
            timer: Mutex::new(None),
            drain_paused: AtomicBool::new(false),
            drain_result: Mutex::new(None),
        }))
    }
    pub(crate) fn same_handle(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    fn assert_open(&self, operation: &str) -> Result<()> {
        if self.0.closing.load(Ordering::Acquire) {
            return Err(Box::new(SessionHandleClosedError::new(
                self.0.id.clone(),
                operation,
            )));
        }
        Ok(())
    }
    // Native Promise.then continuations become handle-owned tasks. Close joins their queue;
    // dropping a caller future cannot cancel a write between bytes and fsync/rollback.
    fn enqueue(
        &self,
        operation: impl std::future::Future<Output = Result<()>> + Send + 'static,
    ) -> BoxFuture<'static, Result<()>> {
        let (done_sender, done_receiver) = watch::channel(false);
        let previous = {
            let mut chain = lock(&self.0.chain);
            std::mem::replace(&mut *chain, done_receiver)
        };
        let (send, receive) = oneshot::channel();
        let guard = QueueDone(done_sender);
        tokio::spawn(async move {
            let mut previous = previous;
            loop {
                let done = *previous.borrow();
                if done {
                    break;
                }
                if previous.changed().await.is_err() {
                    break;
                }
            }
            let result = operation.await;
            let _ = send.send(result);
            drop(guard);
        });
        Box::pin(async move { receive.await.map_err(|e| Box::new(e) as BoxError)? })
    }
    fn run(
        &self,
        operation: &'static str,
        body: impl std::future::Future<Output = Result<()>> + Send + 'static,
    ) -> BoxFuture<'static, Result<()>> {
        if let Err(failure) = self.assert_open(operation) {
            return ready_failure(failure);
        }
        let handle = self.clone();
        self.enqueue(async move {
            handle.assert_open(operation)?;
            body.await
        })
    }
    async fn wait_chain(&self) {
        let mut receiver = lock(&self.0.chain).clone();
        loop {
            let done = *receiver.borrow();
            if done {
                return;
            }
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }
    async fn ensure_lease(&self) -> Result<()> {
        if lock(&self.0.lease).is_some() {
            return Ok(());
        }
        let lease = self.0.storage.acquire_write_lease(&self.0.header).await?;
        *lock(&self.0.lease) = Some(lease);
        Ok(())
    }
    async fn persist_contiguous(&self, batch: Vec<Arc<SessionEvent>>) -> Result<()> {
        if self.0.access != SessionAccess::Write {
            return Err(Box::new(SessionReadOnlyError::new(
                self.0.id.clone(),
                "append",
            )));
        }
        if batch.is_empty() {
            return Ok(());
        }
        self.ensure_lease().await?;
        let state = lock(&self.0.state).clone();
        let plain = batch.iter().map(|e| (**e).clone()).collect::<Vec<_>>();
        assert_contiguous(&self.0.id, &plain, state.cursor)?;
        if let Some(offset) = state.torn_truncate_to {
            self.0
                .storage
                .truncate_torn_tail(&self.0.header, offset)
                .await?;
            lock(&self.0.state).torn_truncate_to = None;
        }
        if let Some(tail) = state.recovered_tail {
            if !tail.is_empty() {
                self.0
                    .storage
                    .persist_batch(&self.0.header, &tail, state.materialized, self.0.cut)
                    .await?;
            }
            lock(&self.0.state).recovered_tail = None;
        }
        self.0
            .storage
            .persist_batch(&self.0.header, &batch, state.materialized, self.0.cut)
            .await?;
        let mut current = lock(&self.0.state);
        current.materialized = true;
        current.cursor += batch.len() as u64;
        current.primed = None;
        self.0.observed.store(current.cursor, Ordering::Release);
        Ok(())
    }
    pub fn enqueue_live(&self, event: &SessionEvent, report: Arc<dyn Fn(BoxError) + Send + Sync>) {
        lock(&self.0.buffer).push(Arc::new(event.clone()));
        let mut timer = lock(&self.0.timer);
        if timer.is_some() || self.0.drain_paused.load(Ordering::Acquire) {
            return;
        }
        let handle = self.clone();
        *timer = Some(tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(
                LIVE_WRITE_BATCH_MAX_DELAY_MS,
            ))
            .await;
            // Remove our JoinHandle before drain_live clears any still-pending timer.
            lock(&handle.0.timer).take();
            if let Err(failure) = handle.drain_live().await {
                report(failure);
            }
        }));
    }
    pub fn drain_live(&self) -> BoxFuture<'static, Result<()>> {
        let mut state = lock(&self.0.drain_result);
        if let Some(receiver) = state.as_ref()
            && receiver.borrow().is_none()
        {
            return Box::pin(wait_completion(receiver.clone()));
        }
        if let Some(timer) = lock(&self.0.timer).take() {
            timer.abort();
        }
        self.0.drain_paused.store(false, Ordering::Release);
        let (sender, receiver) = watch::channel(None);
        *state = Some(receiver.clone());
        let handle = self.clone();
        tokio::spawn(async move {
            let result = handle.drain_buffered().await.map_err(SharedError::new);
            let _ = sender.send(Some(result));
        });
        Box::pin(wait_completion(receiver))
    }
    async fn drain_buffered(&self) -> Result<()> {
        loop {
            if lock(&self.0.buffer).is_empty() {
                return Ok(());
            }
            let handle = self.clone();
            self.enqueue(async move {
                let batch = std::mem::take(&mut *lock(&handle.0.buffer));
                let plain = batch.iter().map(|e| (**e).clone()).collect::<Vec<_>>();
                let result = match materialize_append_batch(&plain) {
                    Ok(batch) => {
                        handle
                            .persist_contiguous(batch.into_iter().map(Arc::new).collect())
                            .await
                    }
                    Err(e) => Err(e),
                };
                if let Err(failure) = result {
                    let mut buffer = lock(&handle.0.buffer);
                    let newer = std::mem::take(&mut *buffer);
                    *buffer = batch;
                    buffer.extend(newer);
                    handle.0.drain_paused.store(true, Ordering::Release);
                    return Err(failure);
                }
                Ok(())
            })
            .await?;
        }
    }
    async fn read_current(
        &self,
        path: &str,
        offset: u64,
        length: u64,
        signal: Option<&AbortSignal>,
    ) -> Result<SessionHandleReadResult> {
        let source = self
            .0
            .storage
            .read_stored_log(path, &self.0.id, signal)
            .await?;
        let previous = self.0.observed.load(Ordering::Acquire);
        if (source.events.len() as u64) < previous {
            return Err(error(format!(
                "session \"{}\": stored log shrank below a previously observed prefix ({} < {previous})",
                self.0.id,
                source.events.len()
            )));
        }
        self.0
            .observed
            .fetch_max(source.events.len() as u64, Ordering::AcqRel);
        Ok(slice(source, offset, length))
    }
    pub fn close_handle(&self) -> BoxFuture<'static, Result<()>> {
        let mut result = lock(&self.0.close_result);
        if let Some(receiver) = result.as_ref() {
            return Box::pin(wait_completion(receiver.clone()));
        }
        self.0.closing.store(true, Ordering::Release);
        let (sender, receiver) = watch::channel(None);
        *result = Some(receiver.clone());
        let handle = self.clone();
        tokio::spawn(async move {
            let mut errors = vec![];
            loop {
                if let Err(failure) = handle.drain_live().await {
                    errors.push(failure);
                    break;
                }
                handle.wait_chain().await;
                if lock(&handle.0.buffer).is_empty() {
                    break;
                }
            }
            handle.wait_chain().await;
            let lease = lock(&handle.0.lease).take();
            if let Some(lease) = lease
                && let Err(failure) = lease.release().await
            {
                errors.push(failure);
            }
            let materialized = lock(&handle.0.state).materialized;
            handle.0.storage.release_handle(&handle, materialized);
            let result = if errors.len() > 1 {
                Err(Box::new(JsonlError::Aggregate {
                    message: format!(
                        "session \"{}\": close failed to drain and to release its write lock",
                        handle.0.id
                    ),
                    errors,
                }) as BoxError)
            } else if let Some(error) = errors.pop() {
                Err(error)
            } else {
                Ok(())
            };
            let _ = sender.send(Some(result.map_err(SharedError::new)));
        });
        Box::pin(wait_completion(receiver))
    }
}
fn slice(mut source: SessionHandleReadResult, offset: u64, length: u64) -> SessionHandleReadResult {
    let start = offset.min(source.events.len() as u64) as usize;
    let end = offset
        .saturating_add(length)
        .min(source.events.len() as u64) as usize;
    source.events = source.events[start..end.max(start)].to_vec();
    source
}
impl SessionHandle for JsonlSessionHandle {
    fn id(&self) -> &SessionId {
        &self.0.id
    }
    fn header(&self) -> &SessionHeader {
        &self.0.header
    }
    fn inherited_event_count(&self) -> SessionLogOffset {
        self.0.cut
    }
    fn access(&self) -> SessionAccess {
        self.0.access
    }
    fn read<'a>(
        &'a self,
        offset: Option<u64>,
        length: Option<u64>,
        options: Option<&'a SessionHandleReadOptions>,
    ) -> BoxFuture<'a, Result<SessionHandleReadResult>> {
        let admission = self.assert_open("read");
        let handle = self.clone();
        let signal = options.and_then(|o| o.signal.clone());
        Box::pin(async move {
            admission?;
            let offset = offset.unwrap_or(0);
            let length = length.unwrap_or(MAX_SAFE_INTEGER);
            for (value, label) in [(offset, "offset"), (length, "length")] {
                if value > MAX_SAFE_INTEGER {
                    return Err(Box::new(hoh_session::SessionError::TypeError(format!(
                        "read {label} must be a non-negative safe integer, got {value}"
                    ))) as BoxError);
                }
            }
            if let Some(signal) = &signal {
                signal.throw_if_aborted()?;
            }
            let state = lock(&handle.0.state).clone();
            if let Some(primed) = state.primed {
                let path = if handle.0.access == SessionAccess::Read {
                    handle
                        .0
                        .storage
                        .resolve_current_log(&handle.0.id, signal.as_ref())
                        .await?
                } else {
                    None
                };
                if let Some(path) = path {
                    lock(&handle.0.state).primed = None;
                    return handle
                        .read_current(&path, offset, length, signal.as_ref())
                        .await;
                }
                handle
                    .0
                    .observed
                    .fetch_max(primed.events.len() as u64, Ordering::AcqRel);
                return Ok(slice(primed, offset, length));
            }
            if handle.0.access == SessionAccess::Write && !state.materialized {
                return Ok(SessionHandleReadResult {
                    events: vec![],
                    event_state: SessionSeedEventState::Detached,
                });
            }
            if let Some(path) = handle
                .0
                .storage
                .resolve_current_log(&handle.0.id, signal.as_ref())
                .await?
            {
                return handle
                    .read_current(&path, offset, length, signal.as_ref())
                    .await;
            }
            if handle.0.storage.has_pending_session(&handle.0.id) {
                return Ok(SessionHandleReadResult {
                    events: vec![],
                    event_state: SessionSeedEventState::Detached,
                });
            }
            Err(Box::new(SessionPersistenceNotFoundError::new(handle.0.id.clone())) as BoxError)
        })
    }
    fn append<'a>(
        &'a self,
        events: &'a [Arc<SessionEvent>],
        options: Option<&'a SessionHandleAppendOptions>,
    ) -> BoxFuture<'a, Result<()>> {
        if let Err(failure) = self.assert_open("append") {
            return ready_failure(failure);
        }
        let plain = events.iter().map(|e| (**e).clone()).collect::<Vec<_>>();
        let batch = match materialize_append_batch(&plain) {
            Ok(batch) => batch.into_iter().map(Arc::new).collect(),
            Err(e) => return ready_failure(e),
        };
        let signal = options.and_then(|o| o.signal.clone());
        let handle = self.clone();
        self.run("append", async move {
            if let Some(signal) = signal {
                signal.throw_if_aborted()?;
            }
            handle.persist_contiguous(batch).await
        })
    }
    fn flush<'a>(
        &'a self,
        options: Option<&'a SessionHandleFlushOptions>,
    ) -> BoxFuture<'a, Result<()>> {
        let signal = options.and_then(|o| o.signal.clone());
        let handle = self.clone();
        self.run("flush", async move {
            if let Some(signal) = signal {
                signal.throw_if_aborted()?;
            }
            if handle.0.access != SessionAccess::Write {
                return Err(
                    Box::new(SessionReadOnlyError::new(handle.0.id.clone(), "flush")) as BoxError,
                );
            }
            if lock(&handle.0.state).materialized {
                return Ok(());
            }
            handle.ensure_lease().await?;
            handle
                .0
                .storage
                .persist_header(&handle.0.header, handle.0.cut)
                .await?;
            lock(&handle.0.state).materialized = true;
            Ok(())
        })
    }
    fn close(&self) -> BoxFuture<'_, Result<()>> {
        self.close_handle()
    }
}

#[derive(Clone)]
pub struct PendingSession {
    pub header: SessionHeader,
    pub revision: SessionPersistenceRevision,
    pub inherited_event_count: SessionLogOffset,
}
#[derive(Default)]
struct TrackerState {
    open: Vec<Arc<JsonlSessionHandle>>,
    writers: indexmap::IndexMap<SessionId, Option<Arc<JsonlSessionHandle>>>,
    pending: indexmap::IndexMap<SessionId, PendingSession>,
    counter: u64,
}
pub struct JsonlBackendTracker {
    name: String,
    state: Mutex<TrackerState>,
}
impl JsonlBackendTracker {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            state: Mutex::new(TrackerState::default()),
        }
    }
    pub fn register_created(&self, header: SessionHeader, cut: SessionLogOffset) -> Result<()> {
        let mut state = lock(&self.state);
        if state.writers.contains_key(&header.id) {
            return Err(Box::new(SessionAlreadyExistsError::new(header.id)));
        }
        state.counter += 1;
        let revision =
            SessionPersistenceRevision::from(format!("memory:{}:{}", self.name, state.counter));
        state.writers.insert(header.id.clone(), None);
        state.pending.insert(
            header.id.clone(),
            PendingSession {
                header,
                revision,
                inherited_event_count: cut,
            },
        );
        Ok(())
    }
    pub fn claim_write(&self, id: &SessionId) -> Result<()> {
        let mut state = lock(&self.state);
        if state.writers.contains_key(id) {
            return Err(Box::new(SessionAlreadyOwnedError::new(id.clone())));
        }
        state.writers.insert(id.clone(), None);
        Ok(())
    }
    pub fn release_claim(&self, id: &SessionId) {
        lock(&self.state).writers.shift_remove(id);
    }
    pub fn pending_of(&self, id: &SessionId) -> Option<PendingSession> {
        lock(&self.state).pending.get(id).cloned()
    }
    pub fn has_pending(&self, id: &SessionId) -> bool {
        lock(&self.state).pending.contains_key(id)
    }
    pub fn pending_entries(&self) -> Vec<(SessionId, PendingSession)> {
        lock(&self.state)
            .pending
            .iter()
            .map(|(id, value)| (id.clone(), value.clone()))
            .collect()
    }
    pub fn materialized(&self, id: &SessionId) {
        lock(&self.state).pending.shift_remove(id);
    }
    pub fn adopt(&self, handle: JsonlSessionHandle) -> Arc<JsonlSessionHandle> {
        let handle = Arc::new(handle);
        let mut state = lock(&self.state);
        state.open.push(handle.clone());
        if handle.access() == SessionAccess::Write {
            state
                .writers
                .insert(handle.id().clone(), Some(handle.clone()));
        }
        handle
    }
    pub fn release(&self, handle: &JsonlSessionHandle, materialized: bool) {
        let mut state = lock(&self.state);
        state.open.retain(|entry| !entry.same_handle(handle));
        if handle.access() == SessionAccess::Write {
            state.writers.shift_remove(handle.id());
            if !materialized {
                state.pending.shift_remove(handle.id());
            }
        }
    }
    pub fn writer(&self, id: &SessionId) -> Option<Arc<JsonlSessionHandle>> {
        lock(&self.state).writers.get(id).cloned().flatten()
    }
    pub async fn flush_all(&self) -> Result<()> {
        let writers = lock(&self.state)
            .writers
            .values()
            .filter_map(Clone::clone)
            .collect::<Vec<_>>();
        let mut errors = vec![];
        for writer in writers {
            let result = async {
                writer.drain_live().await?;
                writer.flush(None).await
            }
            .await;
            if let Err(failure) = result
                && !failure.is::<SessionHandleClosedError>()
            {
                errors.push(failure);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(Box::new(JsonlError::Aggregate {
                message: format!("{} flush failed", self.name),
                errors,
            }))
        }
    }
    pub async fn close_all(&self) -> Result<()> {
        let handles = lock(&self.state).open.clone();
        let mut errors = vec![];
        for handle in handles {
            if let Err(failure) = handle.close().await {
                errors.push(failure);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(Box::new(JsonlError::Aggregate {
                message: format!("{} dispose failed", self.name),
                errors,
            }))
        }
    }
}
