//! Newline-delimited JSON-RPC 2.0 over caller-owned asynchronous byte streams.

use futures_util::{FutureExt, future::BoxFuture};
use hoh_error::{BoxError, SharedError};
use hoh_timeout::{AbortController, AbortSignal};
use hoh_util_values::{js_json_parse, js_json_stringify, js_number_string, js_trim};
use serde_json::{Map, Value, json};
use std::{
    collections::HashMap,
    future::Future,
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex, MutexGuard, Weak},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    runtime::Handle,
    sync::{Mutex as AsyncMutex, mpsc, oneshot},
};

pub type RequestHandler =
    Arc<dyn Fn(String, Value) -> BoxFuture<'static, Result<Value, BoxError>> + Send + Sync>;
pub type NotificationHandler = Arc<dyn Fn(String, Value) + Send + Sync>;

pub trait JsonRpcTransportPeer: Send + Sync + 'static {
    fn request(
        &self,
        method: &str,
        params: Value,
        signal: Option<AbortSignal>,
    ) -> BoxFuture<'static, Result<Value, BoxError>>;

    fn notify(&self, method: &str, params: Option<Value>) -> Result<(), BoxError>;
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct JsonRpcResponseError {
    pub code: Option<f64>,
    pub message: String,
    pub data: Option<Value>,
}

#[derive(Clone, Debug, thiserror::Error)]
#[error("{0}")]
struct TransportError(String);

fn error(message: impl Into<String>) -> BoxError {
    Box::new(TransportError(message.into()))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

enum PendingResult {
    Value(Value),
    ResponseError(JsonRpcResponseError),
    Failure(SharedError),
}

struct PendingRequest {
    waiter: oneshot::Sender<PendingResult>,
    signal: Option<AbortSignal>,
}

type WriteResult = Result<(), BoxError>;

enum Output {
    Frame {
        value: Value,
        written: Option<oneshot::Sender<WriteResult>>,
    },
    Flush(oneshot::Sender<WriteResult>),
}

#[derive(Default)]
struct State {
    started: bool,
    reader_stop: Option<AbortController>,
    failure: Option<SharedError>,
    request_handler: Option<RequestHandler>,
    notification_handler: Option<NotificationHandler>,
    pending: HashMap<String, PendingRequest>,
}

type Input = BufReader<Box<dyn AsyncRead + Unpin + Send>>;

struct Inner {
    state: Mutex<State>,
    input: Arc<AsyncMutex<Input>>,
    output: mpsc::UnboundedSender<Output>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(controller) = lock(&self.state).reader_stop.take() {
            controller.abort();
        }
    }
}

#[derive(Clone)]
pub struct JsonRpcLineTransport {
    inner: Arc<Inner>,
}

impl JsonRpcLineTransport {
    /// Prepare both caller-owned streams. Construction requires a Tokio runtime;
    /// input is not consumed until [`Self::start`] is called.
    pub fn new<R, W>(input: R, mut output: W) -> Result<Self, BoxError>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let runtime = Handle::try_current()
            .map_err(|_| error("JSON-RPC transport requires a Tokio runtime"))?;
        let (tx, mut rx) = mpsc::unbounded_channel();
        runtime.spawn(async move {
            while let Some(message) = rx.recv().await {
                match message {
                    Output::Frame { value, written } => {
                        let mut bytes = js_json_stringify(&value).into_bytes();
                        bytes.push(b'\n');
                        let result = async {
                            output.write_all(&bytes).await?;
                            output.flush().await
                        }
                        .await
                        .map_err(|cause| Box::new(cause) as BoxError);
                        if let Some(written) = written {
                            let _ = written.send(result);
                        }
                    }
                    Output::Flush(done) => {
                        let result = output
                            .flush()
                            .await
                            .map_err(|cause| Box::new(cause) as BoxError);
                        let _ = done.send(result);
                    }
                }
            }
        });
        Ok(Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State::default()),
                input: Arc::new(AsyncMutex::new(BufReader::new(Box::new(input)))),
                output: tx,
            }),
        })
    }

    /// Begin consuming input. Repeated calls are no-ops. A prior close keeps its
    /// permanent outbound failure but, as in the source, does not prevent this
    /// first attachment of the reader.
    pub fn start(&self) -> Result<(), BoxError> {
        let runtime = Handle::try_current()
            .map_err(|_| error("JSON-RPC transport requires a Tokio runtime"))?;
        let (stop, input) = {
            let mut state = lock(&self.inner.state);
            if state.started {
                return Ok(());
            }
            state.started = true;
            let controller = AbortController::new();
            let stop = controller.signal();
            state.reader_stop = Some(controller);
            (stop, self.inner.input.clone())
        };
        let inner = Arc::downgrade(&self.inner);
        runtime.spawn(async move {
            let mut input = input.lock().await;
            let mut line = Vec::new();
            loop {
                line.clear();
                let read = tokio::select! {
                    biased;
                    _ = stop.cancelled() => break,
                    read = input.read_until(b'\n', &mut line) => read,
                };
                match read {
                    Err(cause) => {
                        if let Some(inner) = inner.upgrade() {
                            Self::fail_inner(&inner, SharedError::new(cause));
                        }
                        break;
                    }
                    Ok(0) => {
                        if let Some(inner) = inner.upgrade() {
                            Self::fail_inner(
                                &inner,
                                SharedError::new(TransportError(
                                    "JSON-RPC input closed".to_owned(),
                                )),
                            );
                        }
                        break;
                    }
                    Ok(_) if line.last() != Some(&b'\n') => {
                        if let Some(inner) = inner.upgrade() {
                            Self::fail_inner(
                                &inner,
                                SharedError::new(TransportError(
                                    "JSON-RPC input closed".to_owned(),
                                )),
                            );
                        }
                        break;
                    }
                    Ok(_) => {}
                }
                let text = String::from_utf8_lossy(&line);
                let trimmed = js_trim(&text);
                if trimmed.is_empty() {
                    continue;
                }
                let Ok(value) = js_json_parse(trimmed) else {
                    continue;
                };
                let Some(inner) = inner.upgrade() else {
                    break;
                };
                Self::receive(&inner, value);
            }
        });
        Ok(())
    }

    pub fn on_request<F, Fut>(&self, handler: F)
    where
        F: Fn(String, Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value, BoxError>> + Send + 'static,
    {
        lock(&self.inner.state).request_handler = Some(Arc::new(move |method, params| {
            Box::pin(handler(method, params))
        }));
    }

    pub fn on_notification<F>(&self, handler: F)
    where
        F: Fn(String, Value) + Send + Sync + 'static,
    {
        lock(&self.inner.state).notification_handler = Some(Arc::new(handler));
    }

    pub fn close(&self) {
        if let Some(controller) = lock(&self.inner.state).reader_stop.take() {
            controller.abort();
        }
        Self::fail_inner(
            &self.inner,
            SharedError::new(TransportError("JSON-RPC transport closed".to_owned())),
        );
    }

    pub async fn flush(&self) -> Result<(), BoxError> {
        let (tx, rx) = oneshot::channel();
        self.inner
            .output
            .send(Output::Flush(tx))
            .map_err(|_| error("JSON-RPC transport closed"))?;
        rx.await.map_err(|_| error("JSON-RPC transport closed"))?
    }

    /// Test-parity diagnostic for the source suite's direct `pending.size` checks.
    #[doc(hidden)]
    pub fn pending_len(&self) -> usize {
        lock(&self.inner.state).pending.len()
    }

    fn write_frame(
        &self,
        value: Value,
        acknowledge: bool,
    ) -> Result<Option<oneshot::Receiver<WriteResult>>, BoxError> {
        let (written, receiver) = if acknowledge {
            let (tx, rx) = oneshot::channel();
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        self.inner
            .output
            .send(Output::Frame { value, written })
            .map_err(|_| error("JSON-RPC transport closed"))?;
        Ok(receiver)
    }

    fn fail_inner(inner: &Arc<Inner>, reason: SharedError) {
        let pending = {
            let mut state = lock(&inner.state);
            if state.failure.is_none() {
                state.failure = Some(reason.clone());
            }
            state
                .pending
                .drain()
                .map(|(_, pending)| {
                    let failure = pending
                        .signal
                        .as_ref()
                        .and_then(AbortSignal::reason)
                        .unwrap_or_else(|| reason.clone());
                    (pending.waiter, failure)
                })
                .collect::<Vec<_>>()
        };
        for (waiter, failure) in pending {
            let _ = waiter.send(PendingResult::Failure(failure));
        }
    }

    fn receive(inner: &Arc<Inner>, value: Value) {
        let Some(frame) = value.as_object() else {
            return;
        };
        let id = frame.get("id").and_then(json_rpc_id);
        let method = frame.get("method").and_then(Value::as_str);
        match (id, method) {
            (Some((id, _)), Some(method)) => {
                let params = object_params(frame.get("params"));
                let method = method.to_owned();
                let handler = lock(&inner.state).request_handler.clone();
                let Some(handler) = handler else {
                    let response = json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": format!("method not found: {method}") }
                    });
                    let transport = Self {
                        inner: inner.clone(),
                    };
                    let _ = transport.write_frame(response, false);
                    return;
                };
                // Calling the owner closure is deliberately synchronous. This matches
                // JavaScript execution up to the handler's first await and preserves
                // line-order effects relative to following notifications.
                let response =
                    std::panic::catch_unwind(AssertUnwindSafe(|| handler(method, params)));
                let response = match response {
                    Ok(response) => response,
                    Err(cause) => {
                        let response = json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": { "code": -32603, "message": panic_message(&cause) }
                        });
                        let transport = Self {
                            inner: inner.clone(),
                        };
                        let _ = transport.write_frame(response, false);
                        return;
                    }
                };
                let weak = Arc::downgrade(inner);
                tokio::spawn(async move {
                    let Some(inner) = weak.upgrade() else {
                        return;
                    };
                    let response = match AssertUnwindSafe(response).catch_unwind().await {
                        Ok(result) => match result {
                            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                            Err(cause) => json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": { "code": -32603, "message": cause.to_string() }
                            }),
                        },
                        Err(cause) => json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": { "code": -32603, "message": panic_message(&cause) }
                        }),
                    };
                    let transport = Self { inner };
                    let _ = transport.write_frame(response, false);
                });
            }
            (Some((_, key)), None) => {
                let pending = lock(&inner.state).pending.remove(&key);
                let Some(pending) = pending else {
                    return;
                };
                let result = pending
                    .signal
                    .as_ref()
                    .and_then(AbortSignal::reason)
                    .map_or_else(
                        || {
                            response_error(frame).map_or_else(
                                || {
                                    PendingResult::Value(
                                        frame.get("result").cloned().unwrap_or(Value::Null),
                                    )
                                },
                                PendingResult::ResponseError,
                            )
                        },
                        PendingResult::Failure,
                    );
                let _ = pending.waiter.send(result);
            }
            (None, Some(method)) => {
                let handler = lock(&inner.state).notification_handler.clone();
                if let Some(handler) = handler {
                    handler(method.to_owned(), object_params(frame.get("params")));
                }
            }
            (None, None) => {}
        }
    }
}

impl JsonRpcTransportPeer for JsonRpcLineTransport {
    fn request(
        &self,
        method: &str,
        params: Value,
        signal: Option<AbortSignal>,
    ) -> BoxFuture<'static, Result<Value, BoxError>> {
        let id = format!(
            "req_{}",
            hoh_util_crypto::random_uuid().as_str().replace('-', "")
        );
        let key = format!("s:{id}");
        let (tx, rx) = oneshot::channel();
        let admitted = (|| {
            let mut state = lock(&self.inner.state);
            if let Some(failure) = &state.failure {
                return Err(Box::new(failure.clone()) as BoxError);
            }
            if let Some(signal) = &signal {
                signal
                    .throw_if_aborted()
                    .map_err(|reason| Box::new(reason) as BoxError)?;
            }
            state.pending.insert(
                key.clone(),
                PendingRequest {
                    waiter: tx,
                    signal: signal.clone(),
                },
            );
            drop(state);
            match self.write_frame(
                json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
                true,
            ) {
                Ok(Some(written)) => Ok(written),
                Ok(None) => unreachable!("acknowledged frame has a receiver"),
                Err(cause) => {
                    lock(&self.inner.state).pending.remove(&key);
                    Err(cause)
                }
            }
        })();
        struct Abandon {
            inner: Weak<Inner>,
            key: String,
            abort_done: Option<oneshot::Sender<()>>,
        }
        impl Drop for Abandon {
            fn drop(&mut self) {
                if let Some(done) = self.abort_done.take() {
                    let _ = done.send(());
                }
                if let Some(inner) = self.inner.upgrade() {
                    lock(&inner.state).pending.remove(&self.key);
                }
            }
        }
        let abort_done = if admitted.is_ok() {
            signal.clone().map(|signal| {
                let (done, mut done_rx) = oneshot::channel();
                let inner = Arc::downgrade(&self.inner);
                let key = key.clone();
                tokio::spawn(async move {
                    tokio::select! {
                        biased;
                        _ = &mut done_rx => {}
                        reason = signal.cancelled() => {
                            if let Some(inner) = inner.upgrade()
                                && let Some(pending) = lock(&inner.state).pending.remove(&key)
                            {
                                let _ = pending.waiter.send(PendingResult::Failure(reason));
                            }
                        }
                    }
                });
                done
            })
        } else {
            None
        };
        let guard = Abandon {
            inner: Arc::downgrade(&self.inner),
            key,
            abort_done,
        };
        Box::pin(async move {
            let _guard = guard;
            let written = admitted?;
            let mut rx = rx;
            let early = if let Some(signal) = &signal {
                tokio::select! {
                    biased;
                    result = &mut rx => Some(result),
                    reason = signal.cancelled() => return Err(Box::new(reason) as BoxError),
                    result = written => {
                        result.map_err(|_| error("JSON-RPC transport closed"))??;
                        None
                    },
                }
            } else {
                tokio::select! {
                    biased;
                    result = &mut rx => Some(result),
                    result = written => {
                        result.map_err(|_| error("JSON-RPC transport closed"))??;
                        None
                    },
                }
            };
            let result = if let Some(result) = early {
                result
            } else if let Some(signal) = signal {
                tokio::select! {
                    biased;
                    result = rx => result,
                    reason = signal.cancelled() => return Err(Box::new(reason) as BoxError),
                }
            } else {
                rx.await
            };
            pending_result(result)
        })
    }

    fn notify(&self, method: &str, params: Option<Value>) -> Result<(), BoxError> {
        let value = match params {
            Some(params) => json!({ "jsonrpc": "2.0", "method": method, "params": params }),
            None => json!({ "jsonrpc": "2.0", "method": method }),
        };
        self.write_frame(value, false).map(|_| ())
    }
}

fn pending_result(
    result: Result<PendingResult, oneshot::error::RecvError>,
) -> Result<Value, BoxError> {
    match result.map_err(|_| error("JSON-RPC transport closed"))? {
        PendingResult::Value(value) => Ok(value),
        PendingResult::ResponseError(cause) => Err(Box::new(cause) as BoxError),
        PendingResult::Failure(cause) => Err(Box::new(cause) as BoxError),
    }
}

fn panic_message(cause: &Box<dyn std::any::Any + Send>) -> String {
    cause
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| cause.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "request handler panicked".to_owned())
}

fn object_params(value: Option<&Value>) -> Value {
    value
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}))
}

fn json_rpc_id(value: &Value) -> Option<(Value, String)> {
    match value {
        Value::String(id) => Some((value.clone(), format!("s:{id}"))),
        Value::Number(number) => Some((
            value.clone(),
            format!("n:{}", js_number_string(number.as_f64()?)),
        )),
        _ => None,
    }
}

fn response_error(frame: &Map<String, Value>) -> Option<JsonRpcResponseError> {
    let error = frame.get("error")?;
    if !matches!(error, Value::Object(_) | Value::Array(_)) {
        return None;
    }
    let code = error.get("code").and_then(Value::as_f64);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("JSON-RPC error")
        .to_owned();
    let data = error.get("data").cloned();
    Some(JsonRpcResponseError {
        code,
        message,
        data,
    })
}
