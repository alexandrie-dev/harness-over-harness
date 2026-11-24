//! A harness that runs as a separate program speaking the contract itself: JSON-RPC 2.0 over
//! stdio, one process per HoH session. This is how a harness package from the package host
//! (`docs/packages.md`) plugs in, and how the tests drive the stub harness. Knows no concrete CLI.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures_util::future::BoxFuture;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::contract::{
    AccessMode, AuthState, CONTRACT_VERSION, Discovery, EndReason, EventParams, HarnessEvent,
    HarnessFailure, HarnessPolicy, InitializeParams, InitializeResult, PeerInfo, PromptParams,
    RespondParams, SessionParams, StartParams, StartResult, VersionStatus, code, method,
};
use crate::harness::{ExternalHarness, HarnessResult, HarnessSession, Started};
use crate::redact::redact;
use crate::supervisor::{Frame, ProcessSpec, StderrSink, Supervisor, SupervisorLimits};

/// How long the end of a session waits for the last stderr lines.
const STDERR_DRAIN: Duration = Duration::from_millis(500);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// How to run a contract-speaking harness program.
#[derive(Clone)]
pub struct StdioHarnessConfig {
    pub spec: ProcessSpec,
    pub limits: SupervisorLimits,
    /// Longest wait for an answer to one request.
    pub request_timeout: Duration,
    /// Events waiting for the session driver.
    pub event_queue: usize,
    /// Receives each stderr line, masked.
    pub on_stderr: Option<StderrSink>,
    /// Added after `spec.args` when [`AccessMode::Full`] widens a native session (not a read-only
    /// one; [`AccessMode::widens`]): the program's own switch for running without questions.
    /// `None`: the program has no such launch and full access is refused.
    pub full_access_args: Option<Vec<String>>,
}

impl StdioHarnessConfig {
    pub fn new(spec: ProcessSpec) -> Self {
        Self {
            spec,
            limits: SupervisorLimits::default(),
            request_timeout: Duration::from_secs(30),
            event_queue: 256,
            on_stderr: None,
            full_access_args: None,
        }
    }
}

pub struct StdioHarness {
    config: Arc<StdioHarnessConfig>,
}

impl StdioHarness {
    pub fn new(config: StdioHarnessConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }
}

impl ExternalHarness for StdioHarness {
    /// The arguments added for the mode: the program's own switches are its business.
    fn launch_flags(&self, policy: &HarnessPolicy, access: AccessMode) -> Vec<String> {
        if access.widens(policy) {
            self.config.full_access_args.clone().unwrap_or_default()
        } else {
            Vec::new()
        }
    }

    fn discover(&self) -> BoxFuture<'static, HarnessResult<Discovery>> {
        let found = self.discover_with_status();
        Box::pin(async move { Ok(found.await?.0) })
    }

    /// The program's own `versionStatus` next to its discovery (`unknown` when it gives none).
    fn discover_with_status(
        &self,
    ) -> BoxFuture<'static, HarnessResult<(Discovery, VersionStatus)>> {
        let config = self.config.clone();
        Box::pin(async move {
            let connection =
                match Connection::open(&config, config.spec.cwd.clone(), None, &[]).await {
                    Ok(connection) => connection,
                    Err(failure) if failure.code == code::NOT_INSTALLED => {
                        let discovery = Discovery {
                            installed: false,
                            version: None,
                            auth: AuthState::Unknown,
                            models: Vec::new(),
                            detail: Some(failure.message),
                            failure: None,
                        };
                        return Ok((discovery, VersionStatus::Unknown));
                    }
                    Err(failure) => return Err(failure),
                };
            let answer = connection
                .link
                .request::<_, Value>(method::DISCOVER, &json!({}))
                .await;
            connection.link.shutdown(Duration::from_secs(2)).await;
            let answer = answer?;
            let status = answer
                .get("versionStatus")
                .and_then(|status| serde_json::from_value(status.clone()).ok())
                .unwrap_or(VersionStatus::Unknown);
            let discovery = serde_json::from_value(answer).map_err(|error| {
                HarnessFailure::new(
                    code::PROTOCOL,
                    redact(&format!("invalid answer to {}: {error}", method::DISCOVER)),
                )
            })?;
            Ok((discovery, status))
        })
    }

    fn start_or_resume(&self, params: StartParams) -> BoxFuture<'static, HarnessResult<Started>> {
        self.start_with_access(params, AccessMode::Confirm)
    }

    /// Full access runs the program with [`StdioHarnessConfig::full_access_args`]; the start
    /// request names the mode (`access`) either way.
    fn start_with_access(
        &self,
        params: StartParams,
        access: AccessMode,
    ) -> BoxFuture<'static, HarnessResult<Started>> {
        let config = self.config.clone();
        Box::pin(async move {
            let extra = match (access.widens(&params.policy), &config.full_access_args) {
                (false, _) => Vec::new(),
                (true, Some(args)) => args.clone(),
                (true, None) => {
                    return Err(HarnessFailure::new(
                        code::POLICY_UNAVAILABLE,
                        "this harness program has no full-access launch",
                    ));
                }
            };
            let connection = Connection::open(
                &config,
                Some(params.cwd.clone().into()),
                params.cwd_identity,
                &extra,
            )
            .await?;
            let mut request = encode(&params)?;
            // The identity guards the local process spawn; it is not part of the peer protocol.
            if let Some(object) = request.as_object_mut() {
                object.remove("cwdIdentity");
            }
            request["access"] = json!(access);
            let result = match connection
                .link
                .request::<_, StartResult>(method::START, &request)
                .await
            {
                Ok(result) => result,
                Err(failure) => {
                    connection.link.shutdown(Duration::from_secs(2)).await;
                    return Err(failure);
                }
            };
            let events = connection.take_events()?;
            Ok(Started {
                result,
                session: Arc::new(StdioSession {
                    link: connection.link,
                    session_id: params.session_id,
                }),
                events,
            })
        })
    }
}

/// One running harness process and its JSON-RPC state; cheap to clone.
#[derive(Clone)]
struct Link {
    supervisor: Arc<Supervisor>,
    /// Shared by every clone: the last one gone kills the process (see [`Owner`]).
    _owner: Arc<Owner>,
    rpc: Arc<Rpc>,
    timeout: Duration,
}

/// Kills the process group when the last link to it is dropped without a stop: a start the
/// caller gave up on ("Stop" during `initialize` or `harness/start`) must not leave the CLI
/// waiting on its stdin. The stdout pump holds the supervisor itself, not this.
struct Owner(Arc<Supervisor>);

impl Drop for Owner {
    fn drop(&mut self) {
        self.0.abandon();
    }
}

/// A link plus the events of its process, until the session driver takes them.
struct Connection {
    link: Link,
    events: Mutex<Option<mpsc::Receiver<HarnessEvent>>>,
}

type Pending = HashMap<u64, oneshot::Sender<Result<Value, HarnessFailure>>>;

struct Rpc {
    next_id: AtomicU64,
    pending: Mutex<Pending>,
    /// Set once the stdout pump ended: every later request fails with it.
    closed: Mutex<Option<HarnessFailure>>,
    terminating: AtomicBool,
}

impl Connection {
    /// Start the program in `cwd` and negotiate the contract version.
    async fn open(
        config: &StdioHarnessConfig,
        cwd: Option<std::path::PathBuf>,
        cwd_identity: Option<[u64; 2]>,
        extra_args: &[String],
    ) -> HarnessResult<Connection> {
        let mut spec = config.spec.clone();
        spec.cwd = cwd;
        spec.cwd_identity = cwd_identity;
        spec.args.extend_from_slice(extra_args);
        let supervisor = Arc::new(Supervisor::spawn(
            &spec,
            config.limits,
            config.on_stderr.clone(),
        )?);
        let frames = supervisor
            .take_frames()
            .ok_or_else(|| HarnessFailure::new(code::IO, "harness frames are taken"))?;
        let rpc = Arc::new(Rpc {
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            closed: Mutex::new(None),
            terminating: AtomicBool::new(false),
        });
        let (events_tx, events) = mpsc::channel(config.event_queue.max(1));
        tokio::spawn(pump(frames, supervisor.clone(), rpc.clone(), events_tx));
        let connection = Connection {
            link: Link {
                _owner: Arc::new(Owner(supervisor.clone())),
                supervisor,
                rpc,
                timeout: config.request_timeout,
            },
            events: Mutex::new(Some(events)),
        };
        let initialized = connection
            .link
            .request::<_, InitializeResult>(
                method::INITIALIZE,
                &InitializeParams {
                    protocol_versions: vec![CONTRACT_VERSION],
                    host: PeerInfo {
                        name: "hoh".to_owned(),
                        version: Some(env!("CARGO_PKG_VERSION").to_owned()),
                    },
                },
            )
            .await
            .and_then(|result| {
                if result.protocol_version == CONTRACT_VERSION {
                    Ok(())
                } else {
                    Err(HarnessFailure::new(
                        code::UNSUPPORTED_VERSION,
                        format!(
                            "harness chose contract version {}, HoH speaks {CONTRACT_VERSION}",
                            result.protocol_version
                        ),
                    ))
                }
            });
        if let Err(failure) = initialized {
            connection.link.shutdown(Duration::from_secs(1)).await;
            return Err(failure);
        }
        Ok(connection)
    }

    fn take_events(&self) -> HarnessResult<mpsc::Receiver<HarnessEvent>> {
        lock(&self.events)
            .take()
            .ok_or_else(|| HarnessFailure::new(code::IO, "harness events are taken"))
    }
}

impl Link {
    async fn request<P: Serialize, R: DeserializeOwned>(
        &self,
        name: &str,
        params: &P,
    ) -> HarnessResult<R> {
        let value = self.request_value(name, encode(params)?).await?;
        serde_json::from_value(value).map_err(|error| {
            // serde quotes an unknown variant: the text is masked like the harness's own.
            HarnessFailure::new(
                code::PROTOCOL,
                redact(&format!("invalid answer to {name}: {error}")),
            )
        })
    }

    async fn request_value(&self, name: &str, params: Value) -> HarnessResult<Value> {
        let id = self.rpc.next_id.fetch_add(1, Ordering::Relaxed);
        let (answer_tx, answer) = oneshot::channel();
        {
            if let Some(failure) = lock(&self.rpc.closed).clone() {
                return Err(failure);
            }
            lock(&self.rpc.pending).insert(id, answer_tx);
        }
        let message = json!({"jsonrpc": "2.0", "id": id, "method": name, "params": params});
        // The write itself is confirmed; a process that does not read its stdin gets the same
        // time as one that does not answer.
        let sent = match tokio::time::timeout(self.timeout, self.supervisor.send(&message)).await {
            Ok(sent) => sent,
            Err(_) => Err(HarnessFailure::new(
                code::CLOSED,
                format!("harness did not read {name} in time"),
            )),
        };
        if let Err(failure) = sent {
            lock(&self.rpc.pending).remove(&id);
            return Err(failure);
        }
        match tokio::time::timeout(self.timeout, answer).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(lock(&self.rpc.closed)
                .clone()
                .unwrap_or_else(|| HarnessFailure::new(code::CLOSED, "harness closed"))),
            Err(_) => {
                lock(&self.rpc.pending).remove(&id);
                Err(HarnessFailure::new(
                    code::CLOSED,
                    format!("harness did not answer {name} in time"),
                ))
            }
        }
    }

    /// A graceful `harness/stop` within `grace`, then the process group is terminated.
    async fn shutdown(&self, grace: Duration) {
        self.rpc.terminating.store(true, Ordering::SeqCst);
        if self.supervisor.exit_status().is_none() && lock(&self.rpc.closed).is_none() {
            let stop = self.request_value(method::STOP, json!({}));
            let _ = tokio::time::timeout(grace, stop).await;
        }
        self.terminate(grace).await;
    }

    async fn terminate(&self, grace: Duration) {
        self.rpc.terminating.store(true, Ordering::SeqCst);
        self.supervisor.terminate(grace).await;
    }
}

/// Route stdout frames: answers to their requests, events to the driver, anything else ends the
/// session. The last event is always [`HarnessEvent::SessionEnded`].
async fn pump(
    mut frames: mpsc::Receiver<Frame>,
    supervisor: Arc<Supervisor>,
    rpc: Arc<Rpc>,
    events: mpsc::Sender<HarnessEvent>,
) {
    let (reason, message) = loop {
        let Some(frame) = frames.recv().await else {
            break (EndReason::StreamClosed, None);
        };
        match frame {
            Frame::Message(value) => match route(&value, &rpc) {
                Routed::Event(event) => {
                    // A full queue holds stdout back; a gone driver drops the event.
                    let _ = events.send(*event).await;
                }
                Routed::Request(id) => {
                    let reply = json!({"jsonrpc": "2.0", "id": id, "error": {
                        "code": -32601, "message": "the host serves no requests"}});
                    // Written beside the pump: waiting here for a process that does not read its
                    // stdin would stop stdout too.
                    let supervisor = supervisor.clone();
                    tokio::spawn(async move {
                        let _ = supervisor.send(&reply).await;
                    });
                }
                Routed::Done => {}
                Routed::Invalid(why) => break (EndReason::ProtocolError, Some(why)),
            },
            Frame::Violation(why) => break (EndReason::ProtocolError, Some(why)),
            Frame::Closed { truncated: true } => {
                break (
                    EndReason::StreamClosed,
                    Some("stdout ended in the middle of a message".to_owned()),
                );
            }
            Frame::Closed { truncated: false } => break (EndReason::Exited, None),
        }
    };
    drop(frames);
    let closed = HarnessFailure::new(code::CLOSED, "harness session ended");
    *lock(&rpc.closed) = Some(closed.clone());
    for (_, answer) in lock(&rpc.pending).drain() {
        let _ = answer.send(Err(closed.clone()));
    }
    let (reason, exit_code) = if rpc.terminating.load(Ordering::SeqCst) {
        (
            EndReason::Terminated,
            supervisor.exit_status().and_then(|e| e.code),
        )
    } else if reason != EndReason::Exited {
        // A broken stream or a contract violation: the process can no longer be talked to.
        rpc.terminating.store(true, Ordering::SeqCst);
        let exit = supervisor.terminate(Duration::from_secs(1)).await;
        (reason, exit.code)
    } else {
        match supervisor.wait_timeout(Duration::from_millis(500)).await {
            Some(exit) => {
                // Sweep what the harness left running in its process group.
                supervisor.terminate(Duration::ZERO).await;
                let reason = if exit.success() {
                    EndReason::Exited
                } else {
                    EndReason::Crashed
                };
                (reason, exit.code)
            }
            None => {
                // stdout is closed but the process lives on.
                rpc.terminating.store(true, Ordering::SeqCst);
                let exit = supervisor.terminate(Duration::from_secs(1)).await;
                (EndReason::StreamClosed, exit.code)
            }
        }
    };
    supervisor.stderr_drained(STDERR_DRAIN).await;
    let tail = supervisor.stderr_tail();
    let message = match (message, tail.is_empty()) {
        (Some(why), true) => Some(why),
        (Some(why), false) => Some(format!("{why}\n{tail}")),
        (None, false) if reason != EndReason::Terminated => Some(tail),
        (None, _) => None,
    };
    let _ = events
        .send(HarnessEvent::SessionEnded {
            reason,
            message,
            exit_code: exit_code.map(i64::from),
        })
        .await;
}

enum Routed {
    Event(Box<HarnessEvent>),
    Request(Value),
    Done,
    Invalid(String),
}

fn route(value: &Value, rpc: &Rpc) -> Routed {
    let id = value.get("id");
    let method_name = value.get("method").and_then(Value::as_str);
    match (id, method_name) {
        (Some(id), Some(_)) => Routed::Request(id.clone()),
        (None, Some(name)) if name == method::EVENT => {
            match serde_json::from_value::<EventParams>(value["params"].clone()) {
                Ok(params) => Routed::Event(Box::new(params.event)),
                Err(error) => {
                    Routed::Invalid(redact(&format!("invalid {name} notification: {error}")))
                }
            }
        }
        (None, Some(name)) => Routed::Invalid(redact(&format!("unknown notification {name}"))),
        (Some(id), None) => {
            let Some(id) = id.as_u64() else {
                return Routed::Invalid("answer with a foreign id".to_owned());
            };
            let Some(answer) = lock(&rpc.pending).remove(&id) else {
                // An answer that came after its request timed out.
                return Routed::Done;
            };
            let result = if let Some(error) = value.get("error") {
                Err(rpc_failure(error))
            } else {
                Ok(value.get("result").cloned().unwrap_or(Value::Null))
            };
            let _ = answer.send(result);
            Routed::Done
        }
        (None, None) => Routed::Invalid("a message without id and method".to_owned()),
    }
}

/// A JSON-RPC error object as a failure: `data.code` carries the contract's code.
fn encode<P: Serialize>(params: &P) -> HarnessResult<Value> {
    serde_json::to_value(params)
        .map_err(|error| HarnessFailure::new(code::PROTOCOL, error.to_string()))
}

/// The failure an error answer describes; its text is masked (an upstream error may quote a key).
fn rpc_failure(error: &Value) -> HarnessFailure {
    let message = redact(error["message"].as_str().unwrap_or("harness error"));
    let code = error["data"]["code"]
        .as_str()
        .unwrap_or(code::HARNESS)
        .to_owned();
    HarnessFailure { code, message }
}

struct StdioSession {
    link: Link,
    session_id: String,
}

impl StdioSession {
    fn call<P: Serialize>(
        &self,
        name: &'static str,
        params: P,
    ) -> BoxFuture<'static, HarnessResult<()>> {
        let link = self.link.clone();
        let params = encode(&params);
        Box::pin(async move { link.request_value(name, params?).await.map(|_| ()) })
    }
}

impl HarnessSession for StdioSession {
    fn prompt(&self, params: PromptParams) -> BoxFuture<'static, HarnessResult<()>> {
        self.call(method::PROMPT, params)
    }

    fn interrupt(&self) -> BoxFuture<'static, HarnessResult<()>> {
        let params = SessionParams {
            session_id: self.session_id.clone(),
        };
        self.call(method::INTERRUPT, params)
    }

    fn respond(&self, params: RespondParams) -> BoxFuture<'static, HarnessResult<()>> {
        self.call(method::RESPOND, params)
    }

    fn terminate(&self, grace: Duration) -> BoxFuture<'static, ()> {
        let link = self.link.clone();
        let stop = self.call(
            method::STOP,
            SessionParams {
                session_id: self.session_id.clone(),
            },
        );
        Box::pin(async move {
            // Marked first: the exit that follows a graceful stop is not a crash.
            let first = !link.rpc.terminating.swap(true, Ordering::SeqCst);
            if first && link.supervisor.exit_status().is_none() && lock(&link.rpc.closed).is_none()
            {
                let _ = tokio::time::timeout(grace, stop).await;
            }
            link.terminate(grace).await;
        })
    }

    fn diagnostics(&self) -> String {
        self.link.supervisor.stderr_tail()
    }
}
