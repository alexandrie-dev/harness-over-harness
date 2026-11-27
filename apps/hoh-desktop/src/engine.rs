//! Process supervision and replaceable, frame-level transport. No window has raw transport access.
use hoh_protocol::{
    framing,
    transport::{FrameReader, FrameWriter, Incoming},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::BufReader,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

/// Marker the window checks to tell "engine has no such method" from other failures.
pub const METHOD_UNAVAILABLE: &str = "method-unavailable";

/// The machine-readable code an engine error names, when it is a plain upper-case word. The engine puts it in front of
/// its text (`CODE: words`, the JSON-RPC error carries nothing else); `data.code` is the fallback. The error's own text
/// never leaves the host.
fn error_code(error: &hoh_protocol::framing::RpcError) -> Option<&str> {
    let plain = |code: &&str| {
        !code.is_empty()
            && code.len() <= 40
            && code.bytes().all(|b| b.is_ascii_uppercase() || b == b'_')
    };
    error
        .message
        .split_once(": ")
        .map(|(code, _)| code)
        .filter(plain)
        .or_else(|| {
            error
                .data
                .as_ref()
                .and_then(|data| data["code"].as_str())
                .filter(plain)
        })
}

/// Runs on the engine thread as an outgoing request gets its id, before its frame is written.
pub type OnSend = Box<dyn FnOnce(u64, &str) + Send>;
pub type Reply = mpsc::Sender<Result<Value, String>>;
pub type Sink = Arc<dyn Fn(HostEvent) + Send + Sync>;
pub(crate) type ReadyHandler = Box<dyn Fn(Instant) -> bool + Send>;
#[derive(Clone)]
pub enum HostEvent {
    Notification(Value),
    Status(Value),
    Request {
        generation: u64,
        id: Value,
        method: String,
        params: Value,
    },
}
pub enum Message {
    Request {
        method: String,
        params: Value,
        reply: Reply,
        on_send: Option<OnSend>,
    },
    Respond {
        generation: u64,
        frame: Value,
    },
    Incoming(u64, Incoming),
    Disconnected(u64),
    Restart,
    Stop,
}
/// Future transports supply frames and lifecycle, without changing bridge/approval code.
pub trait Transport: Send {
    fn send(&mut self, frame: Value) -> Result<(), String>;
    fn exited(&mut self) -> bool;
    fn disconnected(&self) -> bool {
        false
    }
    fn close(&mut self);
}
pub trait Connector: Send + Sync + 'static {
    fn connect(
        &self,
        generation: u64,
        inbox: mpsc::Sender<Message>,
    ) -> Result<Box<dyn Transport>, String>;
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessConfig {
    pub path: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra child environment (e.g. the engine's private data folder).
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// Where the engine's stderr goes (tests only; production discards it, it can hold secrets).
    #[serde(default)]
    pub stderr_file: Option<PathBuf>,
    pub cwd: PathBuf,
    pub provider: String,
    pub model: String,
}
impl Connector for ProcessConfig {
    fn connect(
        &self,
        generation: u64,
        inbox: mpsc::Sender<Message>,
    ) -> Result<Box<dyn Transport>, String> {
        let mut command = Command::new(&self.path);
        command
            .args(&self.args)
            .envs(
                self.env
                    .iter()
                    .filter(|(k, _)| !k.starts_with("TELEGRAM_"))
                    .map(|(k, v)| (k, v)),
            )
            .current_dir(&self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(match &self.stderr_file {
                Some(path) => std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .map(Stdio::from)
                    .map_err(|_| "Не удалось открыть журнал ошибок Помощника".to_owned())?,
                None => Stdio::null(),
            });
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("TELEGRAM_") {
                command.env_remove(name);
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command
            .spawn()
            .map_err(|error| spawn_problem(error.kind()))?;
        let stdout = child.stdout.take().ok_or("Нет связи с Помощником")?;
        let stdin = child.stdin.take().ok_or("Нет связи с Помощником")?;
        let (tx, rx) = mpsc::sync_channel::<Value>(64);
        let errors = inbox.clone();
        let disconnected = Arc::new(AtomicBool::new(false));
        let writer_disconnected = disconnected.clone();
        let writer = thread::spawn(move || {
            let mut writer = FrameWriter::new(stdin);
            while let Ok(frame) = rx.recv() {
                if writer.write(&frame).is_err() {
                    break;
                }
            }
            writer_disconnected.store(true, Ordering::Release);
            let _ = errors.send(Message::Disconnected(generation));
        });
        let reader_disconnected = disconnected.clone();
        let reader = thread::spawn(move || {
            let mut reader =
                FrameReader::with_line_limit(BufReader::new(stdout), Some(16 * 1024 * 1024));
            while let Ok(Some(frame)) = reader.read() {
                if inbox.send(Message::Incoming(generation, frame)).is_err() {
                    return;
                }
            }
            reader_disconnected.store(true, Ordering::Release);
            let _ = inbox.send(Message::Disconnected(generation));
        });
        Ok(Box::new(StdioTransport {
            child,
            tx: Some(tx),
            threads: vec![reader, writer],
            disconnected,
            closed: false,
        }))
    }
}
struct StdioTransport {
    child: Child,
    tx: Option<mpsc::SyncSender<Value>>,
    threads: Vec<thread::JoinHandle<()>>,
    disconnected: Arc<AtomicBool>,
    closed: bool,
}
impl Transport for StdioTransport {
    fn send(&mut self, frame: Value) -> Result<(), String> {
        self.tx
            .as_ref()
            .ok_or("Помощник остановлен")?
            .try_send(frame)
            .map_err(|_| "Связь с Помощником недоступна".into())
    }
    fn exited(&mut self) -> bool {
        #[cfg(unix)]
        {
            // Observe without reaping: reserve the PID until close kills its process group.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.child.id(),
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            result < 0 || unsafe { info.si_pid() } != 0
        }
        #[cfg(not(unix))]
        {
            !matches!(self.child.try_wait(), Ok(None))
        }
    }
    fn disconnected(&self) -> bool {
        self.disconnected.load(Ordering::Acquire)
    }
    fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        // The child is not reaped yet, so its process-group id cannot have been reused.
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.kill();
        self.tx.take();
        let _ = self.child.wait();
        for handle in self.threads.drain(..) {
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
    }
}
impl Drop for StdioTransport {
    fn drop(&mut self) {
        self.close();
    }
}

pub struct Engine {
    tx: mpsc::Sender<Message>,
    initialize: Arc<std::sync::Mutex<Value>>,
    join: std::sync::Mutex<Option<thread::JoinHandle<()>>>,
}
impl Engine {
    pub fn start(connector: Arc<dyn Connector>, initialize: Value, sink: Sink) -> Arc<Self> {
        Self::start_with_ready_handler(connector, initialize, sink, None)
    }

    pub(crate) fn start_with_ready_handler(
        connector: Arc<dyn Connector>,
        initialize: Value,
        sink: Sink,
        ready_handler: Option<ReadyHandler>,
    ) -> Arc<Self> {
        let (tx, rx) = mpsc::channel();
        let inbox = tx.clone();
        let initialize = Arc::new(std::sync::Mutex::new(initialize));
        let shared = initialize.clone();
        let join =
            thread::spawn(move || supervise(connector, shared, sink, inbox, rx, ready_handler));
        Arc::new(Self {
            tx,
            initialize,
            join: std::sync::Mutex::new(Some(join)),
        })
    }
    pub fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        self.request_with(method, params, None)
    }
    /// Like `request`; `on_send` learns the generation and JSON-RPC id of this very request.
    pub fn request_with(
        &self,
        method: &str,
        params: Value,
        on_send: Option<OnSend>,
    ) -> Result<Value, String> {
        let (reply, rx) = mpsc::channel();
        self.tx
            .send(Message::Request {
                method: method.into(),
                params,
                reply,
                on_send,
            })
            .map_err(|_| "Помощник остановлен")?;
        rx.recv_timeout(Duration::from_secs(310))
            .map_err(|_| "Помощник не ответил".to_owned())?
    }
    pub fn respond(&self, generation: u64, frame: Value) -> Result<(), String> {
        self.tx
            .send(Message::Respond { generation, frame })
            .map_err(|_| "Помощник остановлен".into())
    }
    /// Starts the engine again; the session folder of the new run is `cwd`. Calls in flight end
    /// with an error and are not repeated.
    pub fn restart_in(&self, cwd: &std::path::Path) -> Result<(), String> {
        self.initialize.lock().unwrap_or_else(|p| p.into_inner())["cwd"] = json!(cwd);
        self.tx
            .send(Message::Restart)
            .map_err(|_| "Помощник остановлен".into())
    }
    pub fn stop(&self) {
        let _ = self.tx.send(Message::Stop);
        if let Some(join) = self.join.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = join.join();
        }
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}
struct Pending {
    method: String,
    reply: Reply,
    started: Instant,
}
/// Why the engine could not be started, in plain words (no paths: the text reaches the window and stderr).
pub fn spawn_problem(kind: std::io::ErrorKind) -> String {
    match kind {
        std::io::ErrorKind::NotFound => {
            "файл Помощника не найден: программа повреждена или установлена не полностью".to_owned()
        }
        std::io::ErrorKind::PermissionDenied => "нет прав на запуск Помощника".to_owned(),
        _ => "Помощник не запускается".to_owned(),
    }
}
/// How long the engine gets, after `shutdown`, to write what it keeps (the session log is written
/// in batches up to 200 ms behind) and exit; the wait ends as soon as it has exited. Killing it
/// earlier loses the end of the conversation: a restart under load found no session to resume. An
/// engine that ignores `shutdown` is still killed once this has passed.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
/// After this many failed starts in a row the window is told the assistant is not starting, with the reason.
pub const START_FAILURES_BEFORE_REPORT: u32 = 4;
fn supervise(
    connector: Arc<dyn Connector>,
    initialize: Arc<std::sync::Mutex<Value>>,
    sink: Sink,
    inbox: mpsc::Sender<Message>,
    rx: mpsc::Receiver<Message>,
    mut ready_handler: Option<ReadyHandler>,
) {
    let mut generation = 0;
    let mut next = 0_u64;
    let mut pending = HashMap::<String, Pending>::new();
    let mut stopping = false;
    let mut failures = 0_u32;
    while !stopping {
        generation += 1;
        let mut transport = match connector.connect(generation, inbox.clone()) {
            Ok(t) => {
                failures = 0;
                t
            }
            Err(reason) => {
                failures += 1;
                if failures <= START_FAILURES_BEFORE_REPORT || failures.is_multiple_of(20) {
                    eprintln!(
                        "hoh-desktop: the engine did not start (attempt {failures}): {reason}"
                    );
                }
                let status = if failures >= START_FAILURES_BEFORE_REPORT {
                    json!({"state":"failed","message":format!("Помощник не запускается: {reason}. Закройте HoH и откройте снова; если не помогает, переустановите программу.")})
                } else {
                    json!({"state":"unavailable","message":"Помощник пока недоступен. Подключаюсь снова…"})
                };
                sink(HostEvent::Status(status));
                if drain_backoff(&rx) {
                    break;
                }
                continue;
            }
        };
        let initialize_id = format!("native-init-{generation}");
        if transport
            .send(framing::request(
                json!(initialize_id),
                "initialize",
                initialize.lock().unwrap_or_else(|p| p.into_inner()).clone(),
            ))
            .is_err()
        {
            transport.close();
            sink(HostEvent::Status(
                json!({"state":"unavailable","message":"Помощник пока недоступен. Подключаюсь снова…"}),
            ));
            if drain_backoff(&rx) {
                break;
            }
            continue;
        }
        let mut ready = false;
        let mut ready_at = None;
        let started = Instant::now();
        sink(HostEvent::Status(
            json!({"state":"starting","message":"Подключаю Помощника…"}),
        ));
        loop {
            let event = rx.recv_timeout(Duration::from_millis(50));
            match event {
                Ok(Message::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = transport.send(framing::request(
                        json!("native-shutdown"),
                        "shutdown",
                        json!({}),
                    ));
                    let until = Instant::now() + SHUTDOWN_GRACE;
                    while !transport.exited() && Instant::now() < until {
                        thread::sleep(Duration::from_millis(10));
                    }
                    stopping = true;
                    break;
                }
                Ok(Message::Request {
                    method,
                    params,
                    reply,
                    on_send,
                }) => {
                    if !ready {
                        let _ = reply.send(Err(crate::errors::CONNECTING.into()));
                        continue;
                    }
                    if pending.len() >= 64 {
                        let _ = reply.send(Err("Помощник занят. Повторите запрос позже".into()));
                        continue;
                    }
                    next += 1;
                    let id = format!("native-{generation}-{next}");
                    if let Some(on_send) = on_send {
                        on_send(generation, &id);
                    }
                    if transport
                        .send(framing::request(json!(id), &method, params))
                        .is_err()
                    {
                        let _ = reply.send(Err("Помощник недоступен".into()));
                        break;
                    }
                    pending.insert(
                        format!("s:{id}"),
                        Pending {
                            method,
                            reply,
                            started: Instant::now(),
                        },
                    );
                }
                Ok(Message::Respond {
                    generation: g,
                    frame,
                }) if g == generation && ready => {
                    if transport.send(frame).is_err() {
                        break;
                    }
                }
                Ok(Message::Incoming(g, incoming)) if g == generation => match incoming {
                    Incoming::Response { id, result, error } => {
                        if id.key() == format!("s:{initialize_id}") && !ready {
                            if error.is_some()
                                || hoh_protocol::validate_result("initialize", &result).is_err()
                                || result["hoh"]["protocolVersion"] != 1
                                || result["hoh"]["approvals"] != "host"
                            {
                                break;
                            }
                            ready = true;
                            ready_at = Some(Instant::now());
                            sink(HostEvent::Status(
                                json!({"state":"ready","message":"Помощник готов"}),
                            ));
                        } else if let Some(p) = pending.remove(&id.key()) {
                            // Error text and arbitrary error data can contain secrets; never expose them.
                            let answer = if let Some(error) = error {
                                // Only the "no such method" code is told apart, so the window can
                                // pick its fallback; error text and data can hold secrets.
                                Err(if error.code == Some(-32601) {
                                    METHOD_UNAVAILABLE.into()
                                } else if error.code == Some(-32021) {
                                    crate::errors::FOLDER_CHANGED.into()
                                } else if error_code(&error) == Some("TRUST_REVOKE_BUSY") {
                                    crate::errors::TRUST_REVOKE_BUSY.into()
                                } else if p.method == "session/delete"
                                    && error_code(&error) == Some("CHAT_DELETE_BUSY")
                                {
                                    crate::errors::CHAT_DELETE_BUSY.into()
                                } else if error_code(&error) == Some("EXECUTOR_FOLDER_FIXED") {
                                    // A closed list of codes may pass on (no text): the window explains this one.
                                    crate::errors::FOLDER_FIXED.into()
                                } else {
                                    crate::errors::REFUSED.into()
                                })
                            } else {
                                hoh_protocol::validate_result(&p.method, &result)
                                    .map(|()| result)
                                    .map_err(|_| "Не удалось прочитать ответ Помощника".into())
                            };
                            let _ = p.reply.send(answer);
                        }
                    }
                    Incoming::Notification { method, params } if ready => {
                        let frame = json!({"method":method,"params":params});
                        if let Ok(typed) =
                            serde_json::from_value::<hoh_protocol::Notification>(frame)
                            && let Ok(frame) = serde_json::to_value(typed)
                        {
                            sink(HostEvent::Notification(frame));
                        }
                    }
                    Incoming::Request { id, method, params } if ready => {
                        sink(HostEvent::Request {
                            generation,
                            id: serde_json::to_value(id).expect("RPC id"),
                            method,
                            params,
                        });
                    }
                    _ => {}
                },
                Ok(Message::Disconnected(g)) if g == generation => break,
                Ok(Message::Restart) => {
                    // Let the engine flush what it keeps (settings) before it is replaced.
                    let _ = transport.send(framing::request(
                        json!("native-restart"),
                        "shutdown",
                        json!({}),
                    ));
                    let until = Instant::now() + SHUTDOWN_GRACE;
                    while !transport.exited() && Instant::now() < until {
                        thread::sleep(Duration::from_millis(10));
                    }
                    break;
                }
                _ => {}
            }
            if transport.exited() || (!ready && started.elapsed() > Duration::from_secs(10)) {
                break;
            }
            if let Some(handler) = &ready_handler
                && let Some(ready_at) = ready_at
                && !transport.disconnected()
                && handler(ready_at)
            {
                ready_handler = None;
            }
            pending.retain(|_, p| {
                if p.started.elapsed() <= Duration::from_secs(310) {
                    return true;
                }
                let _ = p.reply.send(Err("Время ожидания Помощника истекло".into()));
                false
            });
        }
        // Do not replay requests after a crash: their effects may already have happened.
        for (_, p) in pending.drain() {
            let _ = p
                .reply
                .send(Err(crate::errors::CONNECTION_INTERRUPTED.into()));
        }
        transport.close();
        sink(HostEvent::Status(
            json!({"state":if stopping {"stopped"} else {"restarting"},"message":if stopping {"Помощник остановлен"} else {"Связь с Помощником прервалась. Подключаюсь снова…"}}),
        ));
        if !stopping {
            stopping = drain_backoff(&rx);
        }
    }
}
fn drain_backoff(rx: &mpsc::Receiver<Message>) -> bool {
    let until = Instant::now() + Duration::from_millis(500);
    while let Some(wait) = until.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(wait) {
            Ok(Message::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return true,
            Ok(Message::Request { reply, .. }) => {
                let _ = reply.send(Err(crate::errors::RECONNECTING.into()));
            }
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod telegram_environment_tests {
    use super::*;

    #[test]
    fn engine_process_does_not_receive_telegram_environment() {
        let directory = std::env::temp_dir().join(format!("hoh-engine-env-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let output = directory.join("env");
        let config = ProcessConfig {
            path: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "env > \"$1\"".into(),
                "sh".into(),
                output.display().to_string(),
            ],
            env: vec![
                ("TELEGRAM_U4_EXPLICIT".into(), "fixture".into()),
                ("HOH_U4_ALLOWED".into(), "yes".into()),
            ],
            stderr_file: None,
            cwd: directory.clone(),
            provider: "mock".into(),
            model: "mock".into(),
        };
        let (sender, _receiver) = mpsc::channel();
        let mut process = config.connect(0, sender).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !process.exited() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        process.close();
        let environment = std::fs::read_to_string(output).unwrap();
        assert!(environment.lines().any(|line| line == "HOH_U4_ALLOWED=yes"));
        assert!(
            !environment
                .lines()
                .any(|line| line.starts_with("TELEGRAM_"))
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
