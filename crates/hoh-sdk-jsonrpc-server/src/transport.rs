//! Async host adapter over hoh-protocol's framing, classification and pending correlation.
//! hoh-protocol intentionally leaves async waiters/execution to the embedding host.
use crate::{
    error::{HohMethodNotFound, error},
    lock,
};
use futures_util::future::BoxFuture;
use hoh_error::{BoxError, SharedError};
use hoh_kernel::{Context, Undo};
use hoh_protocol::{
    framing::{self, JsonRpcId, RpcError},
    transport::{Incoming, PendingRequests},
};
use hoh_timeout::{AbortController, AbortSignal};
use hoh_util_values::{js_json_parse, js_json_stringify, js_trim};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt},
    sync::{Notify, oneshot, watch},
};

/// Longest wait, on unload, for requests that are still being answered. The source never
/// waits (the process just exits); a handler that cannot settle must not hang disposal.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
/// Total serialized output retained while the peer is not reading. The one frame currently being
/// written is included in both caps.
pub const OUTPUT_QUEUE_MAX_FRAMES: usize = 1024;
pub const OUTPUT_QUEUE_MAX_BYTES: usize = 4 * 1024 * 1024;
const STREAM_QUEUE_MAX_FRAMES: usize = 768;
const STREAM_QUEUE_MAX_BYTES: usize = 3 * 1024 * 1024;

pub trait JsonRpcTransportPeer: Send + Sync + 'static {
    fn request(
        &self,
        method: &str,
        params: Value,
        signal: Option<AbortSignal>,
    ) -> BoxFuture<'static, Result<Value, BoxError>>;
    fn notify(&self, method: &str, params: Option<Value>) -> Result<(), BoxError>;
}
/// Native host owns streams and process exit; no process::exit in a library.
pub type RequestHandler =
    Arc<dyn Fn(String, Value) -> BoxFuture<'static, Result<Value, BoxError>> + Send + Sync>;
pub type NotificationHandler = Arc<dyn Fn(String, Value) + Send + Sync>;
type AfterResponse = Arc<dyn Fn(&str, bool) + Send + Sync>;
#[derive(Debug, thiserror::Error)]
#[error("{}", .0.message)]
pub struct JsonRpcResponseError(pub RpcError);

enum Output {
    Frame(Vec<u8>),
    Flush(oneshot::Sender<Result<(), SharedError>>),
}
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct StreamKey {
    session_id: String,
    attempt_id: String,
}
#[derive(Clone, Debug)]
struct StreamChunkMeta {
    key: StreamKey,
    revision: u64,
    index: u64,
}
enum OutputClass {
    Critical,
    StreamChunk(StreamChunkMeta),
    Gap,
}
struct QueuedOutput {
    output: Output,
    bytes: usize,
    class: OutputClass,
}
impl QueuedOutput {
    fn is_stream_chunk(&self) -> bool {
        matches!(self.class, OutputClass::StreamChunk(_))
    }
}
#[derive(Clone, Copy)]
struct QueueLimits {
    frames: usize,
    bytes: usize,
    stream_frames: usize,
    stream_bytes: usize,
}
impl Default for QueueLimits {
    fn default() -> Self {
        Self {
            frames: OUTPUT_QUEUE_MAX_FRAMES,
            bytes: OUTPUT_QUEUE_MAX_BYTES,
            stream_frames: STREAM_QUEUE_MAX_FRAMES,
            stream_bytes: STREAM_QUEUE_MAX_BYTES,
        }
    }
}
struct OutputQueueState {
    queued: VecDeque<QueuedOutput>,
    in_flight: bool,
    frames: usize,
    bytes: usize,
    stream_frames: usize,
    stream_bytes: usize,
    gapped: HashSet<StreamKey>,
    gapped_bytes: usize,
    accepting: bool,
    stop_after_drain: bool,
    failure: Option<SharedError>,
}
struct OutputQueue {
    limits: QueueLimits,
    state: Mutex<OutputQueueState>,
    ready: Notify,
}
struct QueueAdmissionError {
    reason: SharedError,
    flushes: Vec<oneshot::Sender<Result<(), SharedError>>>,
    new_failure: bool,
}
impl OutputQueue {
    fn new(limits: QueueLimits) -> Self {
        Self {
            limits,
            state: Mutex::new(OutputQueueState {
                queued: VecDeque::new(),
                in_flight: false,
                frames: 0,
                bytes: 0,
                stream_frames: 0,
                stream_bytes: 0,
                gapped: HashSet::new(),
                gapped_bytes: 0,
                accepting: true,
                stop_after_drain: false,
                failure: None,
            }),
            ready: Notify::new(),
        }
    }

    fn closed(state: &OutputQueueState) -> QueueAdmissionError {
        QueueAdmissionError {
            reason: state
                .failure
                .clone()
                .unwrap_or_else(|| SharedError::new(error("JSON-RPC transport closed"))),
            flushes: Vec::new(),
            new_failure: false,
        }
    }

    fn fits(&self, state: &OutputQueueState, frames: usize, bytes: usize) -> bool {
        state.frames.saturating_add(frames) <= self.limits.frames
            && state.bytes.saturating_add(bytes) <= self.limits.bytes
    }

    fn push(state: &mut OutputQueueState, output: QueuedOutput) {
        state.frames += 1;
        state.bytes += output.bytes;
        if output.is_stream_chunk() {
            state.stream_frames += 1;
            state.stream_bytes += output.bytes;
        }
        state.queued.push_back(output);
    }

    fn remove_accounting(state: &mut OutputQueueState, output: &QueuedOutput) {
        state.frames -= 1;
        state.bytes -= output.bytes;
        if output.is_stream_chunk() {
            state.stream_frames -= 1;
            state.stream_bytes -= output.bytes;
        }
    }

    fn gap_output(meta: &StreamChunkMeta) -> QueuedOutput {
        let value = framing::notification(
            "session.assistant-stream",
            serde_json::json!({
                "sessionId": meta.key.session_id,
                "frame": {
                    "type": "gap",
                    "attemptId": meta.key.attempt_id,
                    "revision": meta.revision,
                    "index": meta.index,
                }
            }),
        );
        let mut bytes = js_json_stringify(&value).into_bytes();
        bytes.push(b'\n');
        QueuedOutput {
            bytes: bytes.len(),
            output: Output::Frame(bytes),
            class: OutputClass::Gap,
        }
    }

    fn gap_key_bytes(key: &StreamKey) -> usize {
        std::mem::size_of::<StreamKey>() + key.session_id.len() + key.attempt_id.len()
    }

    fn remove_gap(state: &mut OutputQueueState, key: &StreamKey) {
        if state.gapped.remove(key) {
            state.frames -= 1;
            let bytes = Self::gap_key_bytes(key);
            state.bytes -= bytes;
            state.gapped_bytes -= bytes;
        }
    }

    /// Replace every still-queued chunk of one attempt with one gap at the earliest chunk's FIFO
    /// position. A chunk already being written is deliberately left alone; the gap follows it.
    fn gap_stream(&self, state: &mut OutputQueueState, observed: &StreamChunkMeta) -> bool {
        if state.gapped.contains(&observed.key) {
            return true;
        }
        let earliest = state
            .queued
            .iter()
            .filter_map(|output| match &output.class {
                OutputClass::StreamChunk(meta) if meta.key == observed.key => Some(meta),
                _ => None,
            })
            .min_by_key(|meta| meta.index)
            .cloned()
            .unwrap_or_else(|| observed.clone());
        let gap = Self::gap_output(&earliest);
        let removed = state
            .queued
            .iter()
            .filter(|output| {
                matches!(&output.class, OutputClass::StreamChunk(meta) if meta.key == observed.key)
            })
            .collect::<Vec<_>>();
        let removed_frames = removed.len();
        let removed_bytes = removed.iter().map(|output| output.bytes).sum::<usize>();
        let key_bytes = Self::gap_key_bytes(&observed.key);
        let frames = state.frames - removed_frames + 2;
        let bytes = state.bytes - removed_bytes + gap.bytes + key_bytes;
        if frames > self.limits.frames || bytes > self.limits.bytes {
            return false;
        }
        let mut replacement = Some(gap);
        let mut rebuilt = VecDeque::with_capacity(state.queued.len() + 1);
        let mut inserted = false;
        while let Some(output) = state.queued.pop_front() {
            if matches!(&output.class, OutputClass::StreamChunk(meta) if meta.key == observed.key) {
                Self::remove_accounting(state, &output);
                if !inserted {
                    let gap = replacement.take().expect("one gap replacement");
                    state.frames += 1;
                    state.bytes += gap.bytes;
                    rebuilt.push_back(gap);
                    inserted = true;
                }
            } else {
                rebuilt.push_back(output);
            }
        }
        if !inserted {
            let gap = replacement.take().expect("one appended gap");
            state.frames += 1;
            state.bytes += gap.bytes;
            rebuilt.push_back(gap);
        }
        state.queued = rebuilt;
        state.gapped.insert(observed.key.clone());
        state.frames += 1;
        state.bytes += key_bytes;
        state.gapped_bytes += key_bytes;
        true
    }

    fn first_queued_stream(state: &OutputQueueState) -> Option<StreamChunkMeta> {
        state.queued.iter().find_map(|output| match &output.class {
            OutputClass::StreamChunk(meta) => Some(meta.clone()),
            _ => None,
        })
    }

    fn fail_locked(state: &mut OutputQueueState, reason: SharedError) -> QueueAdmissionError {
        if let Some(existing) = &state.failure {
            return QueueAdmissionError {
                reason: existing.clone(),
                flushes: Vec::new(),
                new_failure: false,
            };
        }
        state.accepting = false;
        state.stop_after_drain = true;
        state.failure = Some(reason.clone());
        let mut flushes = Vec::new();
        while let Some(output) = state.queued.pop_front() {
            Self::remove_accounting(state, &output);
            if let Output::Flush(done) = output.output {
                flushes.push(done);
            }
        }
        state.frames -= state.gapped.len();
        state.bytes -= state.gapped_bytes;
        state.gapped.clear();
        state.gapped_bytes = 0;
        QueueAdmissionError {
            reason,
            flushes,
            new_failure: true,
        }
    }

    fn overflow(state: &mut OutputQueueState) -> QueueAdmissionError {
        Self::fail_locked(
            state,
            SharedError::new(error(format!(
                "JSON-RPC output queue overflow (limit: {} frames / {} bytes)",
                OUTPUT_QUEUE_MAX_FRAMES, OUTPUT_QUEUE_MAX_BYTES
            ))),
        )
    }

    fn enqueue_frame(
        &self,
        bytes: Vec<u8>,
        class: OutputClass,
        end: Option<&StreamKey>,
    ) -> Result<(), QueueAdmissionError> {
        let mut state = lock(&self.state);
        if !state.accepting {
            return Err(Self::closed(&state));
        }
        let size = bytes.len();
        if let OutputClass::StreamChunk(meta) = &class {
            if state.gapped.contains(&meta.key) {
                return Ok(());
            }
            if self.fits(&state, 1, size)
                && state.stream_frames < self.limits.stream_frames
                && state.stream_bytes.saturating_add(size) <= self.limits.stream_bytes
            {
                Self::push(
                    &mut state,
                    QueuedOutput {
                        output: Output::Frame(bytes),
                        bytes: size,
                        class,
                    },
                );
                drop(state);
                self.ready.notify_one();
                return Ok(());
            }
            if self.gap_stream(&mut state, meta) {
                drop(state);
                self.ready.notify_one();
                return Ok(());
            }
            let failure = Self::overflow(&mut state);
            drop(state);
            self.ready.notify_waiters();
            return Err(failure);
        }
        if let Some(end) = end {
            Self::remove_gap(&mut state, end);
        }
        while !self.fits(&state, 1, size) {
            let Some(meta) = Self::first_queued_stream(&state) else {
                let failure = Self::overflow(&mut state);
                drop(state);
                self.ready.notify_waiters();
                return Err(failure);
            };
            if !self.gap_stream(&mut state, &meta) {
                let failure = Self::overflow(&mut state);
                drop(state);
                self.ready.notify_waiters();
                return Err(failure);
            }
        }
        Self::push(
            &mut state,
            QueuedOutput {
                output: Output::Frame(bytes),
                bytes: size,
                class,
            },
        );
        drop(state);
        self.ready.notify_one();
        Ok(())
    }

    fn enqueue_flush(
        &self,
        done: oneshot::Sender<Result<(), SharedError>>,
    ) -> Result<(), QueueAdmissionError> {
        let mut state = lock(&self.state);
        if !state.accepting {
            return Err(Self::closed(&state));
        }
        while !self.fits(&state, 1, 0) {
            let Some(meta) = Self::first_queued_stream(&state) else {
                let failure = Self::overflow(&mut state);
                drop(state);
                self.ready.notify_waiters();
                return Err(failure);
            };
            if !self.gap_stream(&mut state, &meta) {
                let failure = Self::overflow(&mut state);
                drop(state);
                self.ready.notify_waiters();
                return Err(failure);
            }
        }
        Self::push(
            &mut state,
            QueuedOutput {
                output: Output::Flush(done),
                bytes: 0,
                class: OutputClass::Critical,
            },
        );
        drop(state);
        self.ready.notify_one();
        Ok(())
    }

    async fn recv(&self) -> Option<QueuedOutput> {
        loop {
            let notified = self.ready.notified();
            {
                let mut state = lock(&self.state);
                if !state.in_flight
                    && let Some(output) = state.queued.pop_front()
                {
                    state.in_flight = true;
                    return Some(output);
                }
                if !state.in_flight && state.stop_after_drain {
                    return None;
                }
            }
            notified.await;
        }
    }

    fn complete(&self, bytes: usize, stream_chunk: bool) {
        let mut state = lock(&self.state);
        state.frames -= 1;
        state.bytes -= bytes;
        if stream_chunk {
            state.stream_frames -= 1;
            state.stream_bytes -= bytes;
        }
        state.in_flight = false;
        drop(state);
        self.ready.notify_waiters();
    }

    fn finish(&self) {
        let mut state = lock(&self.state);
        state.accepting = false;
        state.stop_after_drain = true;
        drop(state);
        self.ready.notify_waiters();
    }

    fn abort(&self, reason: SharedError) -> Option<QueueAdmissionError> {
        let mut state = lock(&self.state);
        if state.failure.is_some() {
            return None;
        }
        let failure = Self::fail_locked(&mut state, reason);
        drop(state);
        self.ready.notify_waiters();
        Some(failure)
    }
}
#[derive(Default)]
struct State {
    pending: PendingRequests,
    waiters: HashMap<String, oneshot::Sender<Result<Value, SharedError>>>,
    failure: Option<SharedError>,
    request_handler: Option<RequestHandler>,
    notification_handler: Option<NotificationHandler>,
    after_response: Option<AfterResponse>,
    /// The owner promises to call [`JsonRpcLineTransport::close`] during unload; the writer then
    /// outlives everything the owner's own teardown still sends.
    hold_writer: bool,
}
struct Inner {
    reader_shutdown: AbortController,
    state: Mutex<State>,
    output: OutputQueue,
    /// Incoming requests whose handler has not answered yet.
    in_flight: watch::Sender<usize>,
    /// Set by `close()`: the owner's teardown has finished producing frames.
    closed: watch::Sender<bool>,
}
/// One unanswered incoming request; counted from admission until its task ends.
struct InFlight(Arc<Inner>);
impl InFlight {
    fn enter(inner: &Arc<Inner>) -> Self {
        inner.in_flight.send_modify(|count| *count += 1);
        Self(inner.clone())
    }
}
impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.in_flight.send_modify(|count| *count -= 1);
    }
}
#[derive(Clone)]
pub struct JsonRpcLineTransport {
    inner: Arc<Inner>,
}
impl JsonRpcLineTransport {
    /// Attach caller-owned output. Input starts separately so hosts can install handlers first.
    pub fn prepare<W>(ctx: &Context, mut output: W) -> Result<Self, BoxError>
    where
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let transport = Self {
            inner: Arc::new(Inner {
                reader_shutdown: AbortController::new(),
                state: Mutex::new(State::default()),
                output: OutputQueue::new(QueueLimits::default()),
                in_flight: watch::channel(0).0,
                closed: watch::channel(false).0,
            }),
        };
        let sender = transport.clone();
        let finisher = transport.clone();
        // The writer is not cancelled by plugin unload: answers of requests that are still
        // running must reach the output, as in the source, where closing the transport only
        // stops reading. Unload waits for those requests, then stops the writer.
        ctx.effect("jsonrpc.write", move || {
            let writer = tokio::spawn(async move {
                while let Some(message) = sender.inner.output.recv().await {
                    let bytes = message.bytes;
                    let stream_chunk = message.is_stream_chunk();
                    let failure = match message.output {
                        Output::Frame(bytes) => {
                            output.write_all(&bytes).await.err().map(SharedError::new)
                        }
                        Output::Flush(done) => {
                            let failure = output.flush().await.err().map(SharedError::new);
                            let _ = done.send(match &failure {
                                Some(reason) => Err(reason.clone()),
                                None => Ok(()),
                            });
                            failure
                        }
                    };
                    sender.inner.output.complete(bytes, stream_chunk);
                    if let Some(reason) = failure {
                        sender.output_failure(reason);
                        break;
                    }
                }
            });
            Undo::future(move || async move {
                finisher.finish().await;
                writer.await.map_err(|e| Box::new(e) as BoxError)
            })
        })?;
        Ok(transport)
    }

    /// Start consuming input after every request/notification handler is installed.
    pub fn start_reader<R>(&self, ctx: &Context, mut input: R) -> Result<(), BoxError>
    where
        R: AsyncBufRead + Unpin + Send + 'static,
    {
        let reader = self.clone();
        let owner = ctx.clone();
        ctx.spawn("jsonrpc.read", move |token| async move {
            let mut line = Vec::new();
            loop {
                line.clear();
                let stopped = reader.inner.reader_shutdown.signal();
                let read = tokio::select! { biased; _ = token.cancelled() => break, _ = stopped.cancelled() => break, read = input.read_until(b'\n', &mut line) => read };
                match read {
                    Err(e) => { reader.fail(SharedError::new(e)); break; }
                    Ok(0) => { reader.fail(SharedError::new(error("JSON-RPC input closed"))); break; }
                    Ok(_) if line.last() != Some(&b'\n') => { reader.fail(SharedError::new(error("JSON-RPC input closed"))); break; }
                    Ok(_) => {}
                }
                let text = String::from_utf8_lossy(&line);
                if let Ok(value) = js_json_parse(js_trim(&text)) && let Some(frame) = hoh_protocol::transport::classify(value) {
                    reader.receive(&owner, frame);
                }
            }
            Ok(())
        })?;
        Ok(())
    }

    /// Convenience for hosts that have no setup between transport construction and input start.
    pub fn start<R, W>(ctx: &Context, input: R, output: W) -> Result<Self, BoxError>
    where
        R: AsyncBufRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let transport = Self::prepare(ctx, output)?;
        transport.start_reader(ctx, input)?;
        Ok(transport)
    }
    pub fn on_request(&self, handler: RequestHandler) {
        lock(&self.inner.state).request_handler = Some(handler);
    }
    pub fn on_notification(&self, handler: NotificationHandler) {
        lock(&self.inner.state).notification_handler = Some(handler);
    }
    pub fn after_response(&self, callback: AfterResponse) {
        lock(&self.inner.state).after_response = Some(callback);
    }
    fn classify_stream(method: &str, params: Option<&Value>) -> (OutputClass, Option<StreamKey>) {
        if method != "session.assistant-stream" {
            return (OutputClass::Critical, None);
        }
        let Some(params) = params else {
            return (OutputClass::Critical, None);
        };
        let frame = &params["frame"];
        let Some(session_id) = params["sessionId"].as_str() else {
            return (OutputClass::Critical, None);
        };
        let Some(attempt_id) = frame["attemptId"].as_str() else {
            return (OutputClass::Critical, None);
        };
        let key = StreamKey {
            session_id: session_id.to_owned(),
            attempt_id: attempt_id.to_owned(),
        };
        if frame["type"] == "chunk"
            && let (Some(revision), Some(index)) =
                (frame["revision"].as_u64(), frame["index"].as_u64())
        {
            return (
                OutputClass::StreamChunk(StreamChunkMeta {
                    key,
                    revision,
                    index,
                }),
                None,
            );
        }
        let end = (frame["type"] == "end").then_some(key);
        (OutputClass::Critical, end)
    }

    fn queue_error(&self, failure: QueueAdmissionError) -> BoxError {
        for done in failure.flushes {
            let _ = done.send(Err(failure.reason.clone()));
        }
        if failure.new_failure {
            self.inner.reader_shutdown.abort();
            self.fail(failure.reason.clone());
        }
        Box::new(failure.reason)
    }

    fn output_failure(&self, reason: SharedError) {
        if let Some(failure) = self.inner.output.abort(reason) {
            let _ = self.queue_error(failure);
        }
    }

    fn write_classified(
        &self,
        value: Value,
        class: OutputClass,
        end: Option<&StreamKey>,
    ) -> Result<(), BoxError> {
        let mut bytes = js_json_stringify(&value).into_bytes();
        bytes.push(b'\n');
        self.inner
            .output
            .enqueue_frame(bytes, class, end)
            .map_err(|failure| self.queue_error(failure))
    }

    fn write(&self, value: Value) -> Result<(), BoxError> {
        self.write_classified(value, OutputClass::Critical, None)
    }
    /// For an owner that calls [`close`](Self::close) from its own unload (the SDK server does,
    /// after `server.shutdown()`): the writer is stopped only after that, so frames produced by
    /// the teardown (`user-question.cancelled`, ...) still leave. Without it the writer stops as
    /// soon as the requests in flight are answered.
    pub fn hold_writer_until_closed(&self) {
        lock(&self.inner.state).hold_writer = true;
    }
    pub async fn flush(&self) -> Result<(), BoxError> {
        let (tx, rx) = oneshot::channel();
        self.inner
            .output
            .enqueue_flush(tx)
            .map_err(|failure| self.queue_error(failure))?;
        rx.await
            .map_err(|_| error("JSON-RPC transport closed"))?
            .map_err(Into::into)
    }
    /// Wait (bounded) for requests still being answered (and, when the owner asked for it, for
    /// its `close()`), then stop the writer after everything queued so far. Called by the unload
    /// of the output effect; input has stopped by then.
    async fn finish(&self) {
        let hold = lock(&self.inner.state).hold_writer;
        let mut closed = self.inner.closed.subscribe();
        let mut idle = self.inner.in_flight.subscribe();
        // On timeout nothing more can be said to the peer; the writer ends either way.
        let _ = tokio::time::timeout(DRAIN_TIMEOUT, async {
            if hold {
                let _ = closed.wait_for(|closed| *closed).await;
            }
            let _ = idle.wait_for(|count| *count == 0).await;
        })
        .await;
        self.inner.output.finish();
    }
    pub fn close(&self) {
        self.inner.closed.send_replace(true);
        self.inner.reader_shutdown.abort();
        self.fail(SharedError::new(error("JSON-RPC transport closed")));
    }
    fn fail(&self, reason: SharedError) {
        let waiters = {
            let mut state = lock(&self.inner.state);
            if state.failure.is_none() {
                state.failure = Some(reason.clone());
            }
            state.pending.close(&reason.to_string());
            state
                .waiters
                .drain()
                .map(|(_, waiter)| waiter)
                .collect::<Vec<_>>()
        };
        for waiter in waiters {
            let _ = waiter.send(Err(reason.clone()));
        }
    }
    fn receive(&self, _ctx: &Context, frame: Incoming) {
        match frame {
            Incoming::Response { ref id, .. } => {
                let key = id.key();
                let pair = {
                    let mut state = lock(&self.inner.state);
                    state.pending.receive(frame).and_then(|(_, result)| {
                        state.waiters.remove(&key).map(|waiter| (waiter, result))
                    })
                };
                if let Some((waiter, result)) = pair {
                    let _ =
                        waiter.send(result.map_err(|e| SharedError::new(JsonRpcResponseError(e))));
                }
            }
            Incoming::Notification { method, params } => {
                let handler = lock(&self.inner.state).notification_handler.clone();
                if let Some(handler) = handler {
                    handler(method, params);
                }
            }
            Incoming::Request { id, method, params } => {
                let handler = lock(&self.inner.state).request_handler.clone();
                let this = self.clone();
                // A finished request leaves nothing behind (no per-request effect), and an
                // unload does not cancel it: its answer is still written.
                let in_flight = InFlight::enter(&self.inner);
                tokio::spawn(async move {
                    let _in_flight = in_flight;
                    let Ok(id) = serde_json::to_value(id) else {
                        return;
                    };
                    let (response, succeeded) = match handler {
                        None => (
                            framing::error(id, -32601, format!("method not found: {method}")),
                            false,
                        ),
                        Some(handler) => match handler(method.clone(), params).await {
                            Ok(result) => (framing::success(id, result), true),
                            Err(e) => {
                                let code = if e.downcast_ref::<HohMethodNotFound>().is_some() {
                                    -32601
                                } else {
                                    -32603
                                };
                                (framing::error(id, code, e.to_string()), false)
                            }
                        },
                    };
                    // After the writer has stopped (process exit) there is nobody to answer.
                    if this.write(response).is_err() {
                        return;
                    }
                    let callback = lock(&this.inner.state).after_response.clone();
                    if let Some(callback) = callback {
                        callback(&method, succeeded);
                    }
                });
            }
        }
    }
}
impl JsonRpcTransportPeer for JsonRpcLineTransport {
    fn notify(&self, method: &str, params: Option<Value>) -> Result<(), BoxError> {
        let (class, end) = Self::classify_stream(method, params.as_ref());
        let value = match params {
            Some(params) => framing::notification(method, params),
            None => serde_json::json!({"jsonrpc":"2.0", "method":method}),
        };
        self.write_classified(value, class, end.as_ref())
    }
    fn request(
        &self,
        method: &str,
        params: Value,
        signal: Option<AbortSignal>,
    ) -> BoxFuture<'static, Result<Value, BoxError>> {
        let id = JsonRpcId::String(format!(
            "req_{}",
            hoh_util_crypto::random_uuid().as_str().replace('-', "")
        ));
        let key = id.key();
        let (tx, rx) = oneshot::channel();
        let registered = (|| {
            let mut state = lock(&self.inner.state);
            if let Some(e) = &state.failure {
                return Err(Box::new(e.clone()) as BoxError);
            }
            if let Some(signal) = &signal {
                signal.throw_if_aborted()?;
            }
            state.pending.register(&id, method).map_err(error)?;
            state.waiters.insert(key.clone(), tx);
            Ok(())
        })();
        let admitted = registered.and_then(|()| {
            self.write(framing::request(serde_json::to_value(&id)?, method, params))
        });
        struct Abandon {
            inner: Arc<Inner>,
            key: String,
            id: JsonRpcId,
        }
        impl Drop for Abandon {
            fn drop(&mut self) {
                let mut state = lock(&self.inner.state);
                state.pending.abandon(&self.id);
                state.waiters.remove(&self.key);
            }
        }
        let guard = Abandon {
            inner: self.inner.clone(),
            key,
            id,
        };
        Box::pin(async move {
            let _guard = guard;
            admitted?;
            let result = if let Some(signal) = signal {
                tokio::select! { biased; reason = signal.cancelled() => return Err(reason.into()), result = rx => result }
            } else {
                rx.await
            };
            result
                .map_err(|_| error("JSON-RPC transport closed"))?
                .map_err(Into::into)
        })
    }
}

#[cfg(test)]
mod bounded_queue_tests {
    use super::*;

    fn meta(session: &str, attempt: &str, index: u64) -> StreamChunkMeta {
        StreamChunkMeta {
            key: StreamKey {
                session_id: session.into(),
                attempt_id: attempt.into(),
            },
            revision: 7,
            index,
        }
    }

    fn frame(queue: &OutputQueue, bytes: usize, meta: StreamChunkMeta) {
        queue
            .enqueue_frame(vec![b'x'; bytes], OutputClass::StreamChunk(meta), None)
            .unwrap_or_else(|failure| panic!("unexpected queue failure: {}", failure.reason));
    }

    #[test]
    fn coalesces_one_attempt_at_its_earliest_chunk_without_reordering_other_attempts() {
        let queue = OutputQueue::new(QueueLimits {
            frames: 8,
            bytes: 4096,
            stream_frames: 3,
            stream_bytes: 4096,
        });
        frame(&queue, 32, meta("s", "a", 0));
        frame(&queue, 32, meta("s", "b", 0));
        frame(&queue, 32, meta("s", "a", 1));
        frame(&queue, 32, meta("s", "a", 2));

        let state = lock(&queue.state);
        assert!(state.frames <= queue.limits.frames);
        assert!(state.bytes <= queue.limits.bytes);
        assert_eq!(state.stream_frames, 1);
        let mut outputs = state.queued.iter();
        let gap = outputs.next().expect("gap");
        let Output::Frame(bytes) = &gap.output else {
            panic!("gap must be a frame")
        };
        let value: Value = serde_json::from_slice(bytes.strip_suffix(b"\n").unwrap()).unwrap();
        assert_eq!(value["params"]["frame"]["type"], "gap");
        assert_eq!(value["params"]["frame"]["attemptId"], "a");
        assert_eq!(value["params"]["frame"]["index"], 0);
        assert!(matches!(
            outputs.next().map(|output| &output.class),
            Some(OutputClass::StreamChunk(meta)) if meta.key.attempt_id == "b"
        ));
        assert!(outputs.next().is_none());
    }

    #[test]
    fn critical_only_overflow_fails_closed_and_releases_flush_waiters() {
        let queue = OutputQueue::new(QueueLimits {
            frames: 2,
            bytes: 64,
            stream_frames: 1,
            stream_bytes: 32,
        });
        let (done, waiting) = oneshot::channel();
        queue
            .enqueue_flush(done)
            .unwrap_or_else(|failure| panic!("unexpected queue failure: {}", failure.reason));
        queue
            .enqueue_frame(vec![b'x'; 32], OutputClass::Critical, None)
            .unwrap_or_else(|failure| panic!("unexpected queue failure: {}", failure.reason));
        let failure = queue
            .enqueue_frame(vec![b'y'], OutputClass::Critical, None)
            .expect_err("critical-only queue must fail closed");
        assert!(failure.new_failure);
        assert_eq!(failure.flushes.len(), 1);
        for done in failure.flushes {
            let _ = done.send(Err(failure.reason.clone()));
        }
        assert!(waiting.blocking_recv().unwrap().is_err());
        let state = lock(&queue.state);
        assert!(!state.accepting);
        assert!(state.stop_after_drain);
        assert_eq!(state.frames, 0);
        assert_eq!(state.bytes, 0);
    }

    #[tokio::test]
    async fn finish_drains_admitted_fifo_and_atomically_rejects_later_producers() {
        let queue = OutputQueue::new(QueueLimits {
            frames: 2,
            bytes: 64,
            stream_frames: 1,
            stream_bytes: 32,
        });
        queue
            .enqueue_frame(vec![b'x'], OutputClass::Critical, None)
            .unwrap_or_else(|failure| panic!("unexpected queue failure: {}", failure.reason));
        queue.finish();
        assert!(
            queue
                .enqueue_frame(vec![b'y'], OutputClass::Critical, None)
                .is_err()
        );
        let output = queue.recv().await.expect("admitted frame");
        queue.complete(output.bytes, output.is_stream_chunk());
        assert!(queue.recv().await.is_none());
    }

    #[tokio::test]
    async fn in_flight_frames_remain_inside_total_and_stream_caps() {
        let total_bytes = OutputQueue::new(QueueLimits {
            frames: 2,
            bytes: 10,
            stream_frames: 1,
            stream_bytes: 5,
        });
        total_bytes
            .enqueue_frame(vec![b'x'; 6], OutputClass::Critical, None)
            .unwrap_or_else(|failure| panic!("unexpected queue failure: {}", failure.reason));
        let writing = total_bytes.recv().await.expect("in-flight frame");
        assert!(
            total_bytes
                .enqueue_frame(vec![b'y'; 5], OutputClass::Critical, None)
                .is_err(),
            "popped writer frame escaped byte accounting"
        );
        total_bytes.complete(writing.bytes, writing.is_stream_chunk());

        let total_frames = OutputQueue::new(QueueLimits {
            frames: 1,
            bytes: 4096,
            stream_frames: 1,
            stream_bytes: 4096,
        });
        total_frames
            .enqueue_frame(vec![b'x'], OutputClass::Critical, None)
            .unwrap_or_else(|failure| panic!("unexpected queue failure: {}", failure.reason));
        let writing = total_frames.recv().await.expect("in-flight frame");
        assert!(
            total_frames
                .enqueue_frame(vec![b'y'], OutputClass::Critical, None)
                .is_err(),
            "popped writer frame escaped frame accounting"
        );
        total_frames.complete(writing.bytes, writing.is_stream_chunk());

        let stream = OutputQueue::new(QueueLimits {
            frames: 8,
            bytes: 4096,
            stream_frames: 8,
            stream_bytes: 48,
        });
        frame(&stream, 32, meta("s", "a", 0));
        let writing = stream.recv().await.expect("in-flight stream chunk");
        frame(&stream, 32, meta("s", "a", 1));
        {
            let state = lock(&stream.state);
            assert!(state.gapped.contains(&meta("s", "a", 0).key));
            assert_eq!(state.stream_frames, 1, "in-flight chunk was not counted");
            assert_eq!(state.stream_bytes, 32, "in-flight bytes were not counted");
        }
        stream.complete(writing.bytes, writing.is_stream_chunk());
    }

    #[test]
    fn admitted_end_releases_gap_accounting_and_the_attempt_key() {
        let queue = OutputQueue::new(QueueLimits {
            frames: 8,
            bytes: 4096,
            stream_frames: 1,
            stream_bytes: 4096,
        });
        let first = meta("s", "a", 0);
        frame(&queue, 32, first.clone());
        frame(&queue, 32, meta("s", "a", 1));
        let gap_bytes = {
            let state = lock(&queue.state);
            assert!(state.gapped.contains(&first.key));
            assert!(state.gapped_bytes > 0);
            state.queued.front().expect("gap marker").bytes
        };

        queue
            .enqueue_frame(vec![b'e'], OutputClass::Critical, Some(&first.key))
            .unwrap_or_else(|failure| panic!("unexpected queue failure: {}", failure.reason));
        {
            let state = lock(&queue.state);
            assert!(state.gapped.is_empty());
            assert_eq!(state.gapped_bytes, 0);
            assert_eq!(state.frames, 2);
            assert_eq!(state.bytes, gap_bytes + 1);
        }

        frame(&queue, 32, meta("s", "a", 2));
        let state = lock(&queue.state);
        assert_eq!(state.stream_frames, 1, "attempt key stayed suppressed");
    }
}
