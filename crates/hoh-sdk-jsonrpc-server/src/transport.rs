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
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot, watch},
};

/// Longest wait, on unload, for requests that are still being answered. The source never
/// waits (the process just exits); a handler that cannot settle must not hang disposal.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

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
    Frame(Value),
    Flush(oneshot::Sender<Result<(), SharedError>>),
    /// Everything queued before this has been written; the writer ends.
    Stop,
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
    output: mpsc::UnboundedSender<Output>,
    /// Incoming requests whose handler has not answered yet.
    in_flight: watch::Sender<usize>,
    /// Set by `close()`: the owner's teardown has finished producing frames.
    closed: watch::Sender<bool>,
    /// `true` once the writer was told to stop; writes after that are errors, not silent drops.
    /// Locked only around the channel send, never together with `state`'s own work.
    stopped: Mutex<bool>,
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
        let (tx, mut rx) = mpsc::unbounded_channel();
        let transport = Self {
            inner: Arc::new(Inner {
                reader_shutdown: AbortController::new(),
                state: Mutex::new(State::default()),
                output: tx,
                in_flight: watch::channel(0).0,
                closed: watch::channel(false).0,
                stopped: Mutex::new(false),
            }),
        };
        let sender = transport.clone();
        let finisher = transport.clone();
        // The writer is not cancelled by plugin unload: answers of requests that are still
        // running must reach the output, as in the source, where closing the transport only
        // stops reading. Unload waits for those requests, then stops the writer.
        ctx.effect("jsonrpc.write", move || {
            let writer = tokio::spawn(async move {
                while let Some(message) = rx.recv().await {
                    match message {
                        Output::Frame(value) => {
                            let mut bytes = js_json_stringify(&value).into_bytes();
                            bytes.push(b'\n');
                            if let Err(e) = output.write_all(&bytes).await {
                                sender.fail(SharedError::new(e));
                            }
                        }
                        Output::Flush(done) => {
                            let result = output.flush().await.map_err(SharedError::new);
                            let _ = done.send(result);
                        }
                        Output::Stop => break,
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
    fn write(&self, value: Value) -> Result<(), BoxError> {
        let stopped = lock(&self.inner.stopped);
        if *stopped {
            return Err(error("JSON-RPC transport closed"));
        }
        self.inner
            .output
            .send(Output::Frame(value))
            .map_err(|_| error("JSON-RPC transport closed"))
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
            .send(Output::Flush(tx))
            .map_err(|_| error("JSON-RPC transport closed"))?;
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
        let mut stopped = lock(&self.inner.stopped);
        *stopped = true;
        let _ = self.inner.output.send(Output::Stop);
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
        let value = match params {
            Some(params) => framing::notification(method, params),
            None => serde_json::json!({"jsonrpc":"2.0", "method":method}),
        };
        self.write(value)
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
        let admitted = (|| {
            let mut state = lock(&self.inner.state);
            if let Some(e) = &state.failure {
                return Err(Box::new(e.clone()) as BoxError);
            }
            if let Some(signal) = &signal {
                signal.throw_if_aborted()?;
            }
            state.pending.register(&id, method).map_err(error)?;
            state.waiters.insert(key.clone(), tx);
            self.write(framing::request(serde_json::to_value(&id)?, method, params))?;
            Ok(())
        })();
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
