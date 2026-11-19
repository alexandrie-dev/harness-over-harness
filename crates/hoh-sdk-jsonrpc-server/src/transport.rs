//! Async host adapter over hoh-protocol's framing, classification and pending correlation.
//! hoh-protocol intentionally leaves async waiters/execution to the embedding host.
use crate::{error::error, lock};
use futures_util::future::BoxFuture;
use hoh_error::{BoxError, SharedError};
use hoh_kernel::Context;
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
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot},
};

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
}
#[derive(Default)]
struct State {
    pending: PendingRequests,
    waiters: HashMap<String, oneshot::Sender<Result<Value, SharedError>>>,
    failure: Option<SharedError>,
    request_handler: Option<RequestHandler>,
    notification_handler: Option<NotificationHandler>,
    after_response: Option<AfterResponse>,
}
struct Inner {
    reader_shutdown: AbortController,
    state: Mutex<State>,
    output: mpsc::UnboundedSender<Output>,
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
            }),
        };
        let sender = transport.clone();
        ctx.spawn("jsonrpc.write", move |token| async move {
            loop {
                let message =
                    tokio::select! { biased; _ = token.cancelled() => break, m = rx.recv() => m };
                let Some(message) = message else {
                    break;
                };
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
                }
            }
            Ok(())
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
                    reader.receive(&owner, frame)?;
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
        self.inner
            .output
            .send(Output::Frame(value))
            .map_err(|_| error("JSON-RPC transport closed"))
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
    pub fn close(&self) {
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
    fn receive(&self, ctx: &Context, frame: Incoming) -> Result<(), BoxError> {
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
                ctx.spawn("jsonrpc.request", move |token| async move {
                    let id = serde_json::to_value(id)?;
                    let (response, succeeded) = tokio::select! {
                        biased;
                        _ = token.cancelled() => return Ok(()),
                        response = async {
                            match handler {
                                None => (framing::error(id, -32601, format!("method not found: {method}")), false),
                                Some(handler) => match handler(method.clone(), params).await {
                                    Ok(result) => (framing::success(id, result), true),
                                    Err(e) => (framing::error(id, -32603, e.to_string()), false),
                                },
                            }
                        } => response,
                    };
                    this.write(response)?;
                    let callback = lock(&this.inner.state).after_response.clone();
                    if let Some(callback) = callback { callback(&method, succeeded); }
                    Ok(())
                })?;
            }
        }
        Ok(())
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
