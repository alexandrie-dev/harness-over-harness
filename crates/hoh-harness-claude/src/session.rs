//! One running `claude` process speaking the stream-json control protocol, seen as a
//! [`HarnessSession`]: frames in, [`HarnessEvent`]s out, permission requests held until HoH answers.
//!
//! Adapted from T3 Code `ClaudeAdapterV2.ts` (see PORT_NOTES.md): T3 reaches this protocol through
//! the Agent SDK's callbacks (`canUseTool`, `interrupt`, stream events); here the CLI's own JSON
//! lines are read and written directly.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures_util::future::BoxFuture;
use hoh_external_harness::{
    ApprovalDecision, EndReason, FileChange, FileChangeKind, Frame, HarnessAction, HarnessEvent,
    HarnessFailure, HarnessQuestion, HarnessResponse, HarnessSession, ItemOutcome, PromptParams,
    QuestionAnswer, QuestionOption, RespondParams, Supervisor, code,
};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::version;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// What a held `can_use_tool` request is, until HoH answers it.
#[derive(Clone)]
enum Pending {
    Approval {
        input: Value,
        tool_use_id: Option<String>,
        /// The `toolUseID` of the answer: always a string, as in T3 (the request id if the CLI
        /// sent none).
        native_id: String,
    },
    Question {
        input: Value,
        questions: Vec<String>,
        native_id: String,
    },
}

/// Claude Code's refusal of `bypassPermissions` as root (T3 `CLAUDE_ROOT_REFUSAL`).
const ROOT_REFUSAL: &str =
    "--dangerously-skip-permissions cannot be used with root/sudo privileges for security reasons";

const PROCESS_ENDED: &str = "the process ended";

type HostReply = oneshot::Sender<Result<Value, String>>;

#[derive(Default)]
struct State {
    /// Requests this side sent to the CLI (`initialize`, `interrupt`), by request id.
    host: HashMap<String, HostReply>,
    /// Requests the CLI sent here and still waits for, by the CLI's request id.
    pending: HashMap<String, Pending>,
    started: HashSet<String>,
    /// `tool_use` ids of subagents: their steps are not the turn's own (T3 nests them).
    subagent_tools: HashSet<String>,
    declined: HashSet<String>,
    turn: Option<String>,
    interrupting: bool,
    message_id: Option<String>,
    /// The native id of the session; the CLI may assign a new one when it resumes.
    native: String,
    /// Why the stream was cut off (a line that is not a contract message), for the window.
    violation: Option<String>,
    /// Content blocks of each assistant message already consumed (the CLI sends one `assistant`
    /// line per block, all with the same message id).
    consumed: HashMap<String, usize>,
    /// The session ended and released its open requests: nothing is held open any more.
    released: bool,
    /// The CLI called the last assistant message `authentication_failed`.
    auth_failed: bool,
    /// The uuid the running turn's prompt was sent with.
    prompt_uuid: Option<String>,
    /// This CLI has named a prompt of this session by its uuid (an echo in a `result`, or a
    /// `command_lifecycle`): T3 `promptEchoMode` other than `unknown`.
    echoes: bool,
}

struct Inner {
    sup: Supervisor,
    events: Mutex<Option<mpsc::Sender<HarnessEvent>>>,
    state: Mutex<State>,
    cwd: PathBuf,
    next_request: AtomicU64,
    terminated: AtomicBool,
    interrupt_timeout: Duration,
    /// `CLAUDE_CONFIG_DIR` of the process, named when the login has expired.
    config_dir: Option<String>,
    bypass: bool,
    /// The sleeping watchers of unanswered interrupts: they do not outlive the session.
    timers: Mutex<Vec<tokio::task::AbortHandle>>,
}

pub(crate) struct ClaudeSession {
    inner: Arc<Inner>,
}

/// The last handle gone without a stop kills the process group: a start the caller gave up on
/// («Стоп» before `initialize` is answered) must not leave the CLI waiting on its stdin. The
/// reader holds `Inner`, not this, as the stdout pump of the stdio harness does (its `Owner`).
impl Drop for ClaudeSession {
    fn drop(&mut self) {
        self.inner.sup.abandon();
    }
}

/// What a session needs besides the process.
pub(crate) struct SessionSettings {
    pub event_queue: usize,
    pub init_timeout: Duration,
    pub interrupt_timeout: Duration,
    pub config_dir: Option<String>,
    /// The CLI runs in `bypassPermissions` (full access).
    pub bypass: bool,
}

/// What `initialize` answered.
pub(crate) struct InitInfo {
    pub response: Value,
}

impl ClaudeSession {
    /// Take over a spawned process: read its frames, send `initialize`, wait for the answer.
    ///
    /// A process that exits first, or an answer that is not a successful `control_response`,
    /// fails with a message for the window (`version` is the already-checked CLI version).
    pub async fn start(
        sup: Supervisor,
        native: String,
        cwd: PathBuf,
        version: &str,
        settings: SessionSettings,
    ) -> Result<(Arc<ClaudeSession>, mpsc::Receiver<HarnessEvent>, InitInfo), HarnessFailure> {
        let frames = sup
            .take_frames()
            .ok_or_else(|| HarnessFailure::new(code::CLOSED, "process frames are taken"))?;
        let SessionSettings {
            event_queue,
            init_timeout,
            interrupt_timeout,
            config_dir,
            bypass,
        } = settings;
        let (events_tx, events_rx) = mpsc::channel(event_queue.max(1));
        let inner = Arc::new(Inner {
            sup,
            events: Mutex::new(Some(events_tx)),
            state: Mutex::new(State {
                native,
                ..State::default()
            }),
            cwd,
            next_request: AtomicU64::new(1),
            terminated: AtomicBool::new(false),
            interrupt_timeout,
            config_dir,
            bypass,
            timers: Mutex::new(Vec::new()),
        });
        let (init_tx, init_rx) = oneshot::channel();
        let init_id = inner.request_id();
        lock(&inner.state).host.insert(init_id.clone(), init_tx);
        tokio::spawn(read_loop(inner.clone(), frames));
        let session = Arc::new(ClaudeSession { inner });
        let sent = session
            .inner
            .sup
            .send(&json!({
                "type": "control_request",
                "request_id": init_id,
                "request": { "subtype": "initialize" },
            }))
            .await;
        if let Err(error) = sent {
            return Err(session.failed_start(version, error.message).await);
        }
        let answer = tokio::select! {
            biased;
            answer = init_rx => Some(answer),
            _ = session.inner.sup.wait() => None,
            _ = tokio::time::sleep(init_timeout) => {
                session.inner.sup.terminate(Duration::from_millis(500)).await;
                return Err(HarnessFailure::new(
                    code::PROTOCOL,
                    version::protocol_message(version, "нет ответа на initialize"),
                ));
            }
        };
        match answer {
            Some(Ok(Ok(response))) => Ok((session, events_rx, InitInfo { response })),
            Some(Ok(Err(error))) if error == PROCESS_ENDED => {
                Err(session.failed_start(version, String::new()).await)
            }
            Some(Ok(Err(error))) => {
                session
                    .inner
                    .sup
                    .terminate(Duration::from_millis(500))
                    .await;
                Err(HarnessFailure::new(
                    code::PROTOCOL,
                    version::protocol_message(version, &format!("initialize: {error}")),
                ))
            }
            // The process ended (or the reader stopped) before it answered.
            _ => Err(session.failed_start(version, String::new()).await),
        }
    }

    /// The process did not come up: say why, from what it wrote.
    async fn failed_start(&self, version: &str, detail: String) -> HarnessFailure {
        self.inner
            .sup
            .stderr_drained(Duration::from_millis(500))
            .await;
        let tail = hoh_external_harness::redact::redact(&self.inner.sup.stderr_tail());
        let violation = lock(&self.inner.state).violation.clone();
        self.inner.sup.terminate(Duration::from_millis(500)).await;
        if tail.contains(ROOT_REFUSAL) {
            // T3 `ClaudeAdapterV2.ts:383-385`, `:765`: the CLI explains the refusal only on stderr.
            return HarnessFailure::new(
                code::HARNESS,
                format!(
                    "Claude Code exited before it started: {ROOT_REFUSAL}. Полный доступ (bypassPermissions) CLI не разрешает от root/sudo: запустите HoH под обычным пользователем или выберите режим «С подтверждением»."
                ),
            );
        }
        if tail.contains("No conversation found") {
            return HarnessFailure::new(
                code::SESSION_NOT_FOUND,
                "Сессия Claude Code не найдена: её нельзя продолжить.",
            );
        }
        let reason = match violation {
            Some(violation) => format!("{violation}; {}", tail.trim()),
            None if tail.trim().is_empty() => detail,
            None => tail,
        };
        HarnessFailure::new(
            code::PROTOCOL,
            version::protocol_message(
                version,
                &format!(
                    "процесс завершился до ответа на initialize: {}",
                    reason.trim()
                ),
            ),
        )
    }
}

impl ClaudeSession {
    /// End the process at once (a probe has nothing to finish).
    pub async fn terminate_now(&self) {
        self.inner.terminated.store(true, Ordering::SeqCst);
        abort_timers(&self.inner);
        self.inner.sup.terminate(Duration::from_millis(500)).await;
    }
}

impl Inner {
    fn request_id(&self) -> String {
        format!("hoh-{}", self.next_request.fetch_add(1, Ordering::SeqCst))
    }

    async fn emit(&self, event: HarnessEvent) {
        let sender = lock(&self.events).clone();
        if let Some(sender) = sender {
            let _ = sender.send(event).await;
        }
    }

    fn close_events(&self) {
        lock(&self.events).take();
    }

    async fn send(&self, message: Value) -> Result<(), HarnessFailure> {
        self.sup.send(&message).await
    }

    /// The answer of a human is not dropped silently: the failure to write it (`closed`,
    /// `io-error` of the supervisor) goes to the caller unchanged.
    async fn reply(&self, request_id: &str, response: Value) -> Result<(), HarnessFailure> {
        self.send(json!({
            "type": "control_response",
            "response": { "subtype": "success", "request_id": request_id, "response": response },
        }))
        .await
    }

    async fn reply_error(&self, request_id: &str, error: &str) {
        let _ = self
            .send(json!({
                "type": "control_response",
                "response": { "subtype": "error", "request_id": request_id, "error": error },
            }))
            .await;
    }
}

async fn read_loop(inner: Arc<Inner>, mut frames: mpsc::Receiver<Frame>) {
    while let Some(frame) = frames.recv().await {
        match frame {
            Frame::Message(message) => handle_message(&inner, &message).await,
            Frame::Violation(message) => {
                lock(&inner.state).violation = Some(message.clone());
                release_requests(&inner).await;
                inner
                    .emit(HarnessEvent::SessionEnded {
                        reason: EndReason::ProtocolError,
                        message: Some(message),
                        exit_code: None,
                    })
                    .await;
                inner.sup.terminate(Duration::from_millis(500)).await;
                break;
            }
            Frame::Closed { truncated } => {
                end_of_stream(&inner, truncated).await;
                break;
            }
        }
    }
    fail_host_requests(&inner);
    abort_timers(&inner);
    inner.close_events();
}

/// The watchers of unanswered interrupts end with the session, not after their sleep.
fn abort_timers(inner: &Inner) {
    for timer in lock(&inner.timers).drain(..) {
        timer.abort();
    }
}

fn fail_host_requests(inner: &Inner) {
    for (_, reply) in lock(&inner.state).host.drain() {
        let _ = reply.send(Err(PROCESS_ENDED.to_owned()));
    }
}

async fn end_of_stream(inner: &Inner, truncated: bool) {
    if inner.terminated.load(Ordering::SeqCst) {
        return;
    }
    let exit = inner.sup.wait_timeout(Duration::from_secs(2)).await;
    inner.sup.stderr_drained(Duration::from_millis(500)).await;
    let tail = hoh_external_harness::redact::redact(&inner.sup.stderr_tail());
    let event = match exit {
        Some(exit) if exit.success() && !truncated => HarnessEvent::SessionEnded {
            reason: EndReason::Exited,
            message: None,
            exit_code: Some(0),
        },
        Some(exit) => HarnessEvent::SessionEnded {
            reason: EndReason::Crashed,
            message: Some(crash_message(exit.signal, &tail)),
            exit_code: exit.code.map(i64::from),
        },
        None => HarnessEvent::SessionEnded {
            reason: EndReason::StreamClosed,
            message: Some(tail).filter(|tail| !tail.is_empty()),
            exit_code: None,
        },
    };
    release_requests(inner).await;
    inner.emit(event).await;
}

/// Tell the host that every request still open is gone with the process (T3
/// `ClaudeAdapterV2.ts:7713`).
///
/// The events are not allowed to hold the stop back: what a full queue does not take at once is
/// delivered by a task that gives up when the reader is gone or after a while.
async fn release_requests(inner: &Inner) {
    let open: Vec<String> = {
        let mut state = lock(&inner.state);
        state.released = true;
        state.pending.drain().map(|(id, _)| id).collect()
    };
    let Some(sender) = lock(&inner.events).clone() else {
        return;
    };
    let mut late = Vec::new();
    for request_id in open {
        if let Err(mpsc::error::TrySendError::Full(event)) =
            sender.try_send(HarnessEvent::RequestResolved { request_id })
        {
            late.push(event);
        }
    }
    if !late.is_empty() {
        tokio::spawn(async move {
            for event in late {
                if tokio::time::timeout(Duration::from_secs(30), sender.send(event))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
    }
}

/// The message of a crashed process: the signal that killed it (T3 `ClaudeAdapterV2.test.ts:22384`,
/// "Claude Code process terminated by signal SIGABRT"), then what it wrote to stderr.
fn crash_message(signal: Option<i32>, tail: &str) -> String {
    let tail = tail.trim();
    match (signal, tail.is_empty()) {
        (Some(signal), true) => format!(
            "Claude Code process terminated by signal {}",
            signal_name(signal)
        ),
        (Some(signal), false) => format!(
            "Claude Code process terminated by signal {}: {tail}",
            signal_name(signal)
        ),
        (None, true) => "Claude Code завершился с ошибкой".to_owned(),
        (None, false) => tail.to_owned(),
    }
}

fn signal_name(signal: i32) -> String {
    match signal {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        other => return format!("{other}"),
    }
    .to_owned()
}

async fn handle_message(inner: &Inner, message: &Value) {
    match text(message, "type") {
        Some("control_response") => handle_control_response(inner, message),
        Some("control_request") => handle_control_request(inner, message).await,
        Some("control_cancel_request") => {
            if let Some(id) = text(message, "request_id") {
                let known = lock(&inner.state).pending.remove(id).is_some();
                if known {
                    inner
                        .emit(HarnessEvent::RequestResolved {
                            request_id: id.to_owned(),
                        })
                        .await;
                }
            }
        }
        Some("system") => handle_system(inner, message).await,
        Some("stream_event") => handle_stream_event(inner, message).await,
        Some("assistant") => handle_assistant(inner, message).await,
        Some("user") => handle_user(inner, message).await,
        Some("result") => handle_result(inner, message).await,
        // The CLI names the prompt it took: it echoes prompts (T3 `claudePromptLifecycle`).
        Some("command_lifecycle") => {
            let mut state = lock(&inner.state);
            if let Some(uuid) = text(message, "command_uuid")
                && state.prompt_uuid.as_deref() == Some(uuid)
            {
                state.echoes = true;
            }
        }
        // keep_alive, rate_limit_event, auth_status and anything newer: not part of the contract.
        _ => {}
    }
}

fn handle_control_response(inner: &Inner, message: &Value) {
    let Some(response) = message.get("response") else {
        return;
    };
    let Some(id) = text(response, "request_id") else {
        return;
    };
    let Some(reply) = lock(&inner.state).host.remove(id) else {
        return;
    };
    let _ = reply.send(if text(response, "subtype") == Some("success") {
        Ok(response.get("response").cloned().unwrap_or(Value::Null))
    } else {
        Err(text(response, "error").unwrap_or("error").to_owned())
    });
}

async fn handle_system(inner: &Inner, message: &Value) {
    if text(message, "subtype") != Some("init") {
        return;
    }
    if let Some(id) = text(message, "session_id")
        && id != lock(&inner.state).native
    {
        lock(&inner.state).native = id.to_owned();
        inner
            .emit(HarnessEvent::NativeSession {
                native_session_id: id.into(),
            })
            .await;
    }
}

async fn handle_stream_event(inner: &Inner, message: &Value) {
    if message
        .get("parent_tool_use_id")
        .is_some_and(|v| !v.is_null())
    {
        return;
    }
    let Some(event) = message.get("event") else {
        return;
    };
    match text(event, "type") {
        Some("message_start") => {
            let id = event
                .get("message")
                .and_then(|m| text(m, "id"))
                .map(str::to_owned);
            lock(&inner.state).message_id = id;
        }
        Some("content_block_delta") => {
            let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);
            let Some(delta) = event.get("delta") else {
                return;
            };
            let message_id = block_id(inner, None, index as usize);
            match text(delta, "type") {
                Some("text_delta") => {
                    if let Some(text) = text(delta, "text") {
                        inner
                            .emit(HarnessEvent::TextDelta {
                                message_id,
                                text: text.to_owned(),
                            })
                            .await;
                    }
                }
                Some("thinking_delta") => {
                    if let Some(text) = text(delta, "thinking") {
                        inner
                            .emit(HarnessEvent::ReasoningDelta {
                                message_id,
                                text: text.to_owned(),
                            })
                            .await;
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
}

/// `<message id>:<block index>` — the same for the streamed deltas and the final block.
fn block_id(inner: &Inner, message_id: Option<&str>, index: usize) -> String {
    let current = lock(&inner.state).message_id.clone();
    let id = message_id.map(str::to_owned).or(current);
    format!("{}:{index}", id.as_deref().unwrap_or("message"))
}

async fn handle_assistant(inner: &Inner, message: &Value) {
    // T3 `ClaudeAdapterV2.ts:11034`: the CLI names an expired login on the message itself.
    if message
        .get("error")
        .or_else(|| message.get("message").and_then(|body| body.get("error")))
        .and_then(Value::as_str)
        == Some("authentication_failed")
    {
        lock(&inner.state).auth_failed = true;
    }
    let subagent = message
        .get("parent_tool_use_id")
        .is_some_and(|v| !v.is_null());
    let Some(body) = message.get("message") else {
        return;
    };
    let message_id = text(body, "id").unwrap_or("message").to_owned();
    let Some(blocks) = body.get("content").and_then(Value::as_array) else {
        return;
    };
    for block in blocks {
        let ordinal = {
            let mut state = lock(&inner.state);
            let slot = state.consumed.entry(message_id.clone()).or_insert(0);
            let ordinal = *slot;
            *slot += 1;
            ordinal
        };
        let id = format!("{message_id}:{ordinal}");
        match text(block, "type") {
            Some("text") if !subagent => {
                if let Some(text) = text(block, "text") {
                    inner
                        .emit(HarnessEvent::TextFinal {
                            message_id: id,
                            text: text.to_owned(),
                        })
                        .await;
                }
            }
            Some("thinking") if !subagent => {
                if let Some(text) = text(block, "thinking") {
                    inner
                        .emit(HarnessEvent::ReasoningFinal {
                            message_id: id,
                            text: text.to_owned(),
                        })
                        .await;
                }
            }
            Some("tool_use") => {
                if let (Some(tool_id), Some(name)) = (text(block, "id"), text(block, "name")) {
                    if subagent {
                        lock(&inner.state).subagent_tools.insert(tool_id.to_owned());
                    } else {
                        ensure_item_started(inner, tool_id, name, block.get("input")).await;
                    }
                }
            }
            _ => {}
        }
    }
}

async fn ensure_item_started(inner: &Inner, id: &str, name: &str, input: Option<&Value>) {
    let fresh = lock(&inner.state).started.insert(id.to_owned());
    if fresh {
        let action = action_for(name, input.unwrap_or(&Value::Null), &inner.cwd).await;
        inner
            .emit(HarnessEvent::ItemStarted {
                item_id: id.to_owned(),
                action,
            })
            .await;
    }
}

/// For package (б) (`approval: never` by a list), which is off: a command whose folder cannot be
/// told is given `/`, so that the slot asks. In mode (a) the slot asks about every command anyway,
/// and a made-up `/` would only mislead the window and the classifier (relative paths of the
/// command are read from it), so the shown folder is the true one: where the session started.
const STRICT_FOLDER: bool = false;

/// Characters that let a command line build, hide or chain a change of folder.
const SHELL_SYNTAX: &str = "$`\\;&|(){}<>\n\r";
/// Words that run, or can run, another command or change the folder.
const FOLDER_WORDS: [&str; 17] = [
    "cd", "pushd", "popd", "builtin", "command", "eval", "exec", "source", "env", "xargs", "sh",
    "bash", "zsh", "time", "nohup", "sudo", "chdir",
];

/// Options that move the working folder of a command (`make -C /etc`, `git -C x`, `tar -C /`);
/// a word equal to one, or starting with one (`--prefix=/x`, `-C/x`), makes the folder unknown.
const FOLDER_OPTIONS: [&str; 7] = [
    "--directory",
    "--prefix",
    "--git-dir",
    "--work-tree",
    "--cwd",
    "--chdir",
    "-chdir",
];

fn moves_folder(word: &str) -> bool {
    FOLDER_OPTIONS.iter().any(|option| word.starts_with(option))
        // `-C` alone or with a path; `-C3` (context lines of grep) is a number.
        || word
            .strip_prefix("-C")
            .is_some_and(|rest| rest.is_empty() || !rest.chars().all(|c| c.is_ascii_digit()))
}

/// The folder a command runs in. The contract has no "unknown", so a command whose folder cannot
/// be told is given the filesystem root: outside any session folder, so the slot asks (fail
/// closed). A allow-list, no shell parsing: the folder of the session only for a line without
/// any of [`SHELL_SYNTAX`] and without a word of [`FOLDER_WORDS`] or [`FOLDER_OPTIONS`] (`ls`, `cat a.txt`,
/// `rg foo src`); everything else, `cd x && make` included, is unknown.
pub(crate) fn command_cwd(command: &str, session: &std::path::Path) -> String {
    // The quotes of the shell are gone before it looks for a program: `c''d` is `cd`.
    let unquoted = command.replace(['\'', '"'], "");
    let words = || unquoted.split_whitespace();
    let plain = !command.contains(|c: char| SHELL_SYNTAX.contains(c))
        && !words().any(|word| {
            // `/bin/sh`, `/usr/bin/env`: the name of the program, not its path.
            let name = word.rsplit('/').next().unwrap_or(word);
            FOLDER_WORDS.contains(&name) || moves_folder(word)
        })
        // `.` is `source` only as the program; as an argument (`ls .`) it is a path.
        && words().next() != Some(".");
    if plain {
        session.to_string_lossy().into_owned()
    } else {
        "/".to_owned()
    }
}

/// What a Claude tool call is, in the contract's words.
pub(crate) async fn action_for(name: &str, input: &Value, cwd: &std::path::Path) -> HarnessAction {
    let field = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| input.get(*key).and_then(Value::as_str))
            .map(str::to_owned)
    };
    match name {
        "Bash" => {
            if let Some(command) = field(&["command"]) {
                let cwd = if STRICT_FOLDER {
                    command_cwd(&command, cwd)
                } else {
                    cwd.to_string_lossy().into_owned()
                };
                return HarnessAction::Command {
                    command,
                    cwd: Some(cwd),
                };
            }
        }
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
            if let Some(path) = field(&["file_path", "notebook_path", "path"]) {
                let exists = tokio::fs::try_exists(cwd.join(&path))
                    .await
                    .unwrap_or(false);
                let kind = if name == "Write" && !exists {
                    FileChangeKind::Add
                } else {
                    FileChangeKind::Update
                };
                return HarnessAction::FileChange {
                    changes: vec![FileChange {
                        path,
                        kind,
                        move_path: None,
                    }],
                };
            }
        }
        _ => {}
    }
    HarnessAction::Tool {
        name: name.to_owned(),
        input: input.clone(),
    }
}

async fn handle_user(inner: &Inner, message: &Value) {
    let Some(blocks) = message
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
    else {
        return;
    };
    for block in blocks {
        if text(block, "type") != Some("tool_result") {
            continue;
        }
        let Some(id) = text(block, "tool_use_id") else {
            continue;
        };
        let (known, declined) = {
            let mut state = lock(&inner.state);
            state.subagent_tools.remove(id);
            (state.started.remove(id), state.declined.remove(id))
        };
        if !known {
            continue;
        }
        let failed = block.get("is_error").and_then(Value::as_bool) == Some(true);
        let outcome = if declined {
            ItemOutcome::Declined
        } else if non_execution_kind(message, id)
            .is_some_and(|kind| matches!(kind, "cancelled" | "interrupted"))
        {
            // T3 `ClaudeAdapterV2.test.ts:8208`: the CLI says the tool never ran.
            ItemOutcome::Interrupted
        } else if failed {
            ItemOutcome::Failed
        } else {
            ItemOutcome::Completed
        };
        inner
            .emit(HarnessEvent::ItemCompleted {
                item_id: id.to_owned(),
                outcome,
                output: Some(tool_result_text(block.get("content"))),
                exit_code: None,
            })
            .await;
    }
}

/// `tool_result_meta: [{id, non_execution_kind}]` of a `user` line (T3 `ClaudeAdapterV2.ts:4327-4340`).
fn non_execution_kind<'a>(message: &'a Value, tool_use_id: &str) -> Option<&'a str> {
    message
        .get("tool_result_meta")?
        .as_array()?
        .iter()
        .find(|meta| text(meta, "id") == Some(tool_use_id))
        .and_then(|meta| text(meta, "non_execution_kind"))
}

fn tool_result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| text(block, "text"))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The prompt uuids a `result` names as the ones it answers (T3 `claudeEchoedPromptUuids`).
fn echoed_uuids(message: &Value) -> Vec<&str> {
    match message.get("user_message_uuids").and_then(Value::as_array) {
        Some(uuids) => uuids.iter().filter_map(Value::as_str).collect(),
        None => text(message, "user_message_uuid").into_iter().collect(),
    }
}

/// A `result` of a turn the CLI ran on its own (a background task's notification, a wake), not
/// of the prompt this session sent: it ends no turn here (T3 `isClaudeResultForOtherTurn`,
/// `isClaudeEmptyTaskNotificationSuccess`). Pilot 22.11: a CLI resumed after «Стоп» answered a
/// background command lost with the old process first; taken for the prompt's answer, it closed
/// the turn at once, the window went idle with no «Стоп» to press, and the real answer came
/// between turns. Without an echo, an origin other than `human` proves another turn only on a
/// CLI that echoes prompts (on another, the prompt may be folded into that turn).
fn of_another_turn(state: &mut State, message: &Value) -> bool {
    let echoed = echoed_uuids(message);
    if !echoed.is_empty() {
        state.echoes = true;
        return !state
            .prompt_uuid
            .as_deref()
            .is_some_and(|ours| echoed.contains(&ours));
    }
    let origin = message
        .get("origin")
        .and_then(|origin| text(origin, "kind"));
    let lifecycle = origin == Some("task-notification")
        && text(message, "subtype") == Some("success")
        && message.get("is_error").and_then(Value::as_bool) != Some(true)
        && message.get("num_turns").and_then(Value::as_u64) == Some(0)
        && text(message, "result").unwrap_or("").is_empty();
    lifecycle || (state.echoes && origin.is_some_and(|kind| kind != "human"))
}

async fn handle_result(inner: &Inner, message: &Value) {
    let (turn, interrupted, open, withdrawn, auth_failed) = {
        let mut state = lock(&inner.state);
        if state.turn.is_some() && of_another_turn(&mut state, message) {
            return;
        }
        state.prompt_uuid = None;
        let open: Vec<String> = state.started.drain().collect();
        state.declined.clear();
        state.subagent_tools.clear();
        state.consumed.clear();
        let withdrawn: Vec<String> = state.pending.drain().map(|(id, _)| id).collect();
        (
            state.turn.take(),
            std::mem::take(&mut state.interrupting),
            open,
            withdrawn,
            std::mem::take(&mut state.auth_failed),
        )
    };
    for request_id in withdrawn {
        inner
            .emit(HarnessEvent::RequestResolved { request_id })
            .await;
    }
    for item_id in open {
        inner
            .emit(HarnessEvent::ItemCompleted {
                item_id,
                outcome: ItemOutcome::Interrupted,
                output: None,
                exit_code: None,
            })
            .await;
    }
    let Some(turn_id) = turn else {
        return;
    };
    let failed = message.get("is_error").and_then(Value::as_bool) == Some(true)
        || text(message, "subtype").is_some_and(|subtype| subtype != "success");
    let event = if interrupted {
        HarnessEvent::TurnAborted { turn_id }
    } else if failed {
        let detail = text(message, "result")
            .or_else(|| text(message, "subtype"))
            .unwrap_or("error");
        // T3 `:12016-12042`: a 403 shows the CLI's own text (login cannot grant access); an
        // expired login is named instead of the terminal API error.
        let forbidden = message.get("api_error_status").and_then(Value::as_u64) == Some(403);
        let detail = if auth_failed && !forbidden {
            signed_out_message(&inner.cwd, inner.config_dir.as_deref())
        } else {
            detail.to_owned()
        };
        HarnessEvent::TurnFailed {
            turn_id,
            error: HarnessFailure::new(code::HARNESS, detail),
        }
    } else {
        HarnessEvent::TurnCompleted { turn_id }
    };
    inner.emit(event).await;
}

async fn handle_control_request(inner: &Inner, message: &Value) {
    let Some(request_id) = text(message, "request_id") else {
        return;
    };
    let Some(request) = message.get("request") else {
        return;
    };
    match text(request, "subtype") {
        Some("can_use_tool") => can_use_tool(inner, request_id, request).await,
        _ => {
            inner
                .reply_error(request_id, "unsupported control request")
                .await;
        }
    }
}

/// The login of Claude Code has expired (T3 `claudeSignedOutMessage`): says how to renew it and
/// where the CLI looks for it.
fn signed_out_message(cwd: &std::path::Path, config_dir: Option<&str>) -> String {
    let config = config_dir.map_or_else(String::new, |dir| format!("; папка настроек {dir}"));
    format!(
        "Вход в Claude Code истёк или не выполнен: run `claude auth login` в терминале и повторите \
(папка сессии {}{config}).",
        serde_json::Value::String(cwd.to_string_lossy().into_owned())
    )
}

/// `can_use_tool`: T3 `ClaudeAdapterV2.ts` `canUseToolEffect`. `AskUserQuestion` becomes a
/// question; everything else a permission request.
async fn can_use_tool(inner: &Inner, request_id: &str, request: &Value) {
    let name = text(request, "tool_name").unwrap_or("tool");
    let input = request.get("input").cloned().unwrap_or_else(|| json!({}));
    let native_id = text(request, "tool_use_id")
        .unwrap_or(request_id)
        .to_owned();
    if name == "AskUserQuestion" {
        let (questions, texts) = questions_from(&input);
        lock(&inner.state).pending.insert(
            request_id.to_owned(),
            Pending::Question {
                input,
                questions: texts,
                native_id,
            },
        );
        inner
            .emit(HarnessEvent::QuestionAsked {
                request_id: request_id.to_owned(),
                questions,
            })
            .await;
        return;
    }
    let tool_use_id = text(request, "tool_use_id").map(str::to_owned);
    let of_subagent = tool_use_id
        .as_deref()
        .is_some_and(|id| lock(&inner.state).subagent_tools.contains(id));
    if let Some(id) = tool_use_id.as_deref().filter(|_| !of_subagent) {
        ensure_item_started(inner, id, name, Some(&input)).await;
    }
    if inner.bypass && is_removal_safety_refusal(name, request) {
        // T3 `ClaudeAdapterV2.ts:12494-12510`: the CLI's own removal check refused it under
        // bypass; the model is told so, and the human is not asked to override a safety check.
        let safety = strip_terminal_escapes(text(request, "decision_reason").unwrap_or_default());
        let safety = safety.trim();
        let denial = json!({
            "behavior": "deny",
            "message": format!(
                "Claude Code's removal safety check denied this command; the user did not reject it. The command did not run. Do not bypass the check by splitting the removal, putting it in a script, or using another tool or shell. If the warning below supplies a safe rewrite, use it and let the same check evaluate it. Otherwise continue the rest of the task without the removal and tell the user what you needed to delete and why. Warning: {safety}"
            ),
            "toolUseID": native_id,
        });
        let _ = inner.reply(request_id, denial).await;
        return;
    }
    let reason = approval_reason(name, &input, request);
    let action = action_for(name, &input, &inner.cwd).await;
    lock(&inner.state).pending.insert(
        request_id.to_owned(),
        Pending::Approval {
            input,
            tool_use_id: tool_use_id.clone(),
            native_id,
        },
    );
    inner
        .emit(HarnessEvent::ApprovalRequested {
            request_id: request_id.to_owned(),
            item_id: tool_use_id.filter(|_| !of_subagent),
            action,
            reason,
        })
        .await;
}

/// Under bypass the CLI's removal warnings (T3 `:12494-12508`; the SDK omits the CLI's
/// discriminator, so the texts are matched): `Dangerous rm`/`rmdir …`, and the two `shell -c`
/// messages, unless an ask rule of the user matched.
fn is_removal_safety_refusal(name: &str, request: &Value) -> bool {
    static DANGEROUS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let dangerous = DANGEROUS.get_or_init(|| {
        regex::Regex::new(r"(?i)^Dangerous rm(?:dir)?\b").expect("a fixed pattern")
    });
    let reason = strip_terminal_escapes(text(request, "decision_reason").unwrap_or_default());
    let reason = reason.trim();
    name == "Bash"
        && request.get("matched_ask_rule").is_none_or(Value::is_null)
        && (dangerous.is_match(reason)
            || reason == "This shell -c script runs rm and could not be checked"
            || reason == "A permission rule denies a command in this shell -c script")
}

/// The prompt T3 shows (`ClaudeAdapterV2.ts:12511-12517`): the command itself, else the CLI's
/// title, description, decision reason, else a summary of the call.
pub(crate) fn approval_prompt(name: &str, input: &Value, request: &Value) -> String {
    // As in T3: the command if it is not blank (as it is, untrimmed); then, by `??`, the first of
    // title, description, decision reason that is present at all (an empty title is chosen).
    text(input, "command")
        .filter(|command| !command.trim().is_empty())
        .or_else(|| text(request, "title"))
        .or_else(|| text(request, "description"))
        .or_else(|| text(request, "decision_reason"))
        .map(str::to_owned)
        .unwrap_or_else(|| summarize_tool(name, input))
}

/// `stripVTControlCharacters` of Node (the ansi-regex pattern it uses): the CLI's prompt renderer
/// leaves terminal escapes in its texts (T3 `ClaudeAdapterV2.ts:4367`). Only what the pattern
/// matches goes (`ESC[31m`, `ESC]0;title BEL`, `ESC(B`, the C1 forms); a bare `\r` or BEL stays.
pub(crate) fn strip_terminal_escapes(value: &str) -> String {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let pattern = PATTERN.get_or_init(|| {
        regex::Regex::new(concat!(
            r"[\x{1B}\x{9B}][\[\]()#;?]*",
            r"(?:(?:(?:(?:;[-a-zA-Z0-9/#&.:=?%@~_]+)*|[a-zA-Z0-9]+(?:;[-a-zA-Z0-9/#&.:=?%@~_]*)*)?",
            r"(?:\x{07}|\x{1B}\x{5C}|\x{9C}))",
            r"|(?:(?:[0-9]{1,4}(?:;[0-9]{0,4})*)?[0-9A-PR-TZcf-ntqry=><~]))",
        ))
        .expect("the pattern of Node's stripVTControlCharacters")
    });
    pattern.replace_all(value, "").into_owned()
}

/// The first `limit` UTF-16 units of `value` (JS `slice(0, limit)`), cut at a character.
fn slice_utf16(value: &str, limit: usize) -> String {
    let mut units = 0;
    let mut out = String::new();
    for c in value.chars() {
        units += c.len_utf16();
        if units > limit {
            break;
        }
        out.push(c);
    }
    out
}

const SUMMARY_LIMIT: usize = 400;

/// `summarizeClaudeToolRequest`, T3 `ClaudeAdapterV2.ts:4120-4132`.
fn summarize_tool(name: &str, input: &Value) -> String {
    let first = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| text(input, key))
            .map(|value| slice_utf16(value, SUMMARY_LIMIT))
    };
    if let Some(command) = first(&["command", "cmd", "script"]) {
        return format!("{name}: {command}");
    }
    if let Some(path) = first(&["file_path", "path", "filename", "fileName"]) {
        return format!("{name}: {path}");
    }
    // Keys in the order the CLI sent them (`preserve_order`), written as `JSON.stringify` does.
    let serialized = hoh_util_values::js_json_stringify(input);
    if serialized.encode_utf16().count() <= SUMMARY_LIMIT {
        format!("{name}: {serialized}")
    } else {
        format!("{name}: {}...", slice_utf16(&serialized, SUMMARY_LIMIT - 3))
    }
}

/// The reason is the CLI's decision reason unless it only repeats the prompt
/// (T3 `claudeApprovalPresentation`, `ClaudeAdapterV2.ts:4360-4387`).
pub(crate) fn approval_reason(name: &str, input: &Value, request: &Value) -> Option<String> {
    let prompt = approval_prompt(name, input, request);
    // Escapes are stripped from the reason only, and the comparison is with the raw prompt.
    let reason = strip_terminal_escapes(text(request, "decision_reason").unwrap_or_default());
    let reason = reason.trim();
    // Shown at the same limit as the summary of a call (`summarize_tool`).
    (!reason.is_empty() && reason != prompt).then(|| slice_utf16(reason, SUMMARY_LIMIT))
}

/// The `AskUserQuestion` input (`{questions: [{question, header, options, multiSelect}]}`) as
/// contract questions with ids `q0`, `q1`, …, plus each question's text (the key of its answer).
fn trimmed<'a>(value: &'a Value, key: &str) -> &'a str {
    text(value, key).map(str::trim).unwrap_or_default()
}

fn questions_from(input: &Value) -> (Vec<HarnessQuestion>, Vec<String>) {
    // T3 `claudeUserInputQuestions`, `ClaudeAdapterV2.ts:5339-5388`: an empty question is
    // dropped; no header is `Question N` (N of the original list); an option without a
    // description shows its label.
    let mut questions = Vec::new();
    let mut texts = Vec::new();
    for (index, item) in input
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let question = trimmed(item, "question");
        if question.is_empty() {
            continue;
        }
        let header = match trimmed(item, "header") {
            "" => format!("Question {}", index + 1),
            header => header.to_owned(),
        };
        texts.push(question.to_owned());
        questions.push(HarnessQuestion {
            id: format!("q{}", questions.len()),
            question: question.to_owned(),
            header: Some(header),
            options: item
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|option| {
                    let label = trimmed(option, "label");
                    if label.is_empty() {
                        return None;
                    }
                    let description = match trimmed(option, "description") {
                        "" => label,
                        description => description,
                    };
                    Some(QuestionOption {
                        label: label.to_owned(),
                        description: Some(description.to_owned()),
                    })
                })
                .collect(),
            multi_select: item.get("multiSelect").and_then(Value::as_bool) == Some(true),
        });
    }
    (questions, texts)
}

/// T3 `ClaudeAdapterV2.test.ts:14176`: an interrupt the CLI never follows with a result is
/// bounded. The turn ends once as aborted, the open steps as interrupted, and the process is
/// closed.
/// The content of a user turn as Claude reads it (HOH-41): the pictures first, each a base64 block,
/// then the text, as T3's Claude adapter sends them. A turn of pictures alone carries no empty
/// text block (the model refuses one).
fn prompt_content(params: &PromptParams) -> Value {
    let mut content: Vec<Value> = params
        .images
        .iter()
        .map(|image| {
            json!({
                "type": "image",
                "source": { "type": "base64", "media_type": image.media_type, "data": image.data },
            })
        })
        .collect();
    if content.is_empty() || !params.text.is_empty() {
        content.push(json!({ "type": "text", "text": params.text }));
    }
    Value::Array(content)
}

fn spawn_interrupt_watch(inner: &Arc<Inner>, turn_id: String) {
    let handle = tokio::spawn(settle_unanswered_interrupt(inner.clone(), turn_id));
    let mut timers = lock(&inner.timers);
    timers.retain(|timer| !timer.is_finished());
    timers.push(handle.abort_handle());
}

async fn settle_unanswered_interrupt(inner: Arc<Inner>, turn_id: String) {
    tokio::time::sleep(inner.interrupt_timeout).await;
    let open: Vec<String> = {
        let mut state = lock(&inner.state);
        if state.turn.as_deref() != Some(turn_id.as_str())
            || inner.terminated.load(Ordering::SeqCst)
        {
            return;
        }
        state.turn = None;
        state.interrupting = false;
        state.declined.clear();
        state.subagent_tools.clear();
        state.consumed.clear();
        state.started.drain().collect()
    };
    inner.terminated.store(true, Ordering::SeqCst);
    // The settlement is not the watcher's to lose: the end of the stream aborts the watchers,
    // and the turn must still get its one end and the session its `SessionEnded`.
    tokio::spawn(async move {
        for item_id in open {
            inner
                .emit(HarnessEvent::ItemCompleted {
                    item_id,
                    outcome: ItemOutcome::Interrupted,
                    output: None,
                    exit_code: None,
                })
                .await;
        }
        release_requests(&inner).await;
        inner.emit(HarnessEvent::TurnAborted { turn_id }).await;
        inner
            .emit(HarnessEvent::SessionEnded {
                reason: EndReason::Terminated,
                message: Some(format!(
                    "Claude Code не подтвердил остановку за {} с",
                    inner.interrupt_timeout.as_secs_f32()
                )),
                exit_code: None,
            })
            .await;
        inner.sup.terminate(Duration::from_millis(500)).await;
    });
}

impl HarnessSession for ClaudeSession {
    fn prompt(&self, params: PromptParams) -> BoxFuture<'static, Result<(), HarnessFailure>> {
        let inner = self.inner.clone();
        Box::pin(async move {
            {
                let mut state = lock(&inner.state);
                if state.turn.is_some() {
                    return Err(HarnessFailure::new(
                        code::HARNESS,
                        "a turn of this session is already running",
                    ));
                }
                state.turn = Some(params.turn_id.clone());
                state.interrupting = false;
            }
            // A fresh uuid for every prompt: the CLI skips a prompt it has recorded before.
            let uuid = match crate::harness::random_uuid() {
                Ok(uuid) => uuid,
                Err(failure) => {
                    lock(&inner.state).turn = None;
                    return Err(failure);
                }
            };
            lock(&inner.state).prompt_uuid = Some(uuid.clone());
            let native = lock(&inner.state).native.clone();
            inner
                .emit(HarnessEvent::TurnStarted {
                    turn_id: params.turn_id.clone(),
                })
                .await;
            let sent = inner
                .send(json!({
                    "type": "user",
                    "message": { "role": "user", "content": prompt_content(&params) },
                    "parent_tool_use_id": null,
                    "session_id": native,
                    "uuid": uuid,
                }))
                .await;
            if sent.is_err() {
                // Reported once: by the error the slot ends the turn with, not by an event too.
                lock(&inner.state).turn = None;
            }
            sent
        })
    }

    fn interrupt(&self) -> BoxFuture<'static, Result<(), HarnessFailure>> {
        let inner = self.inner.clone();
        Box::pin(async move {
            {
                let mut state = lock(&inner.state);
                if state.turn.is_none() {
                    return Ok(());
                }
                state.interrupting = true;
            }
            let id = inner.request_id();
            let (tx, _rx) = oneshot::channel();
            lock(&inner.state).host.insert(id.clone(), tx);
            let sent = inner
                .send(json!({
                    "type": "control_request",
                    "request_id": id,
                    "request": { "subtype": "interrupt" },
                }))
                .await;
            if sent.is_err() {
                let mut state = lock(&inner.state);
                state.host.remove(&id);
                state.interrupting = false;
            } else if let Some(turn_id) = lock(&inner.state).turn.clone() {
                spawn_interrupt_watch(&inner, turn_id);
            }
            sent
        })
    }

    fn respond(&self, params: RespondParams) -> BoxFuture<'static, Result<(), HarnessFailure>> {
        let inner = self.inner.clone();
        Box::pin(async move {
            let pending = lock(&inner.state).pending.remove(&params.request_id);
            let Some(pending) = pending else {
                return Err(HarnessFailure::new(
                    code::PROTOCOL,
                    format!("no open request \"{}\"", params.request_id),
                ));
            };
            let mut declined = None;
            let kept = pending.clone();
            let response = match (pending, params.response) {
                (
                    Pending::Approval {
                        input,
                        tool_use_id,
                        native_id,
                    },
                    HarnessResponse::Approval { decision },
                ) => match decision {
                    ApprovalDecision::Allow => {
                        json!({
                            "behavior": "allow",
                            "updatedInput": input,
                            "toolUseID": native_id,
                            "decisionClassification": "user_temporary",
                        })
                    }
                    ApprovalDecision::Deny => {
                        declined = tool_use_id.clone();
                        json!({
                            "behavior": "deny",
                            "message": "User declined tool execution.",
                            "toolUseID": native_id,
                            "decisionClassification": "user_reject",
                        })
                    }
                },
                (
                    Pending::Question {
                        input,
                        questions,
                        native_id,
                    },
                    HarnessResponse::Answer { answers },
                ) => {
                    let mut updated = serde_json::Map::new();
                    if let Some(asked) = input.get("questions") {
                        updated.insert("questions".to_owned(), asked.clone());
                    }
                    updated.insert(
                        "answers".to_owned(),
                        answers_by_question(&questions, &answers),
                    );
                    json!({
                        "behavior": "allow",
                        "updatedInput": updated,
                        "toolUseID": native_id,
                    })
                }
                (pending, HarnessResponse::Cancelled) => match pending {
                    Pending::Approval {
                        tool_use_id,
                        native_id,
                        ..
                    } => {
                        declined = tool_use_id;
                        json!({
                            "behavior": "deny",
                            "message": "User cancelled tool execution.",
                            "toolUseID": native_id,
                            "decisionClassification": "user_reject",
                            "interrupt": true,
                        })
                    }
                    // T3 `ClaudeAdapterV2.ts:12433-12437`: a withdrawn question is a plain deny.
                    Pending::Question { .. } => {
                        json!({ "behavior": "deny", "message": "User cancelled tool execution." })
                    }
                },
                (pending, _) => {
                    // Put it back: the answer was of the wrong kind.
                    lock(&inner.state)
                        .pending
                        .insert(params.request_id.clone(), pending);
                    return Err(HarnessFailure::new(
                        code::PROTOCOL,
                        "the answer does not fit the request",
                    ));
                }
            };
            if let Some(item) = &declined {
                lock(&inner.state).declined.insert(item.clone());
            }
            let sent = inner.reply(&params.request_id, response).await;
            if sent.is_err() {
                // The answer did not reach the process: the request is not answered. It stays
                // open (and is released with the session), and the step is not marked declined.
                let gone = {
                    let mut state = lock(&inner.state);
                    if let Some(item) = declined {
                        state.declined.remove(&item);
                    }
                    if state.released {
                        // The session ended while the answer was on its way: the release
                        // did not see this request, so it is released here, once.
                        true
                    } else {
                        state.pending.insert(params.request_id.clone(), kept);
                        false
                    }
                };
                if gone {
                    inner
                        .emit(HarnessEvent::RequestResolved {
                            request_id: params.request_id.clone(),
                        })
                        .await;
                }
            }
            sent
        })
    }

    fn terminate(&self, grace: Duration) -> BoxFuture<'static, ()> {
        let inner = self.inner.clone();
        Box::pin(async move {
            inner.terminated.store(true, Ordering::SeqCst);
            abort_timers(&inner);
            release_requests(&inner).await;
            inner.sup.terminate(grace).await;
        })
    }

    fn diagnostics(&self) -> String {
        hoh_external_harness::redact::redact(&self.inner.sup.stderr_tail())
    }
}

fn answers_by_question(questions: &[String], answers: &[QuestionAnswer]) -> Value {
    let mut map = serde_json::Map::new();
    for answer in answers {
        let Some(index) = answer
            .id
            .strip_prefix('q')
            .and_then(|index| index.parse::<usize>().ok())
        else {
            continue;
        };
        let Some(question) = questions.get(index) else {
            continue;
        };
        let mut parts = answer.selected.clone();
        parts.extend(answer.custom.clone());
        map.insert(question.clone(), Value::String(parts.join(", ")));
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_is_the_command_then_title_description_reason_then_a_summary() {
        let input = json!({ "command": "make build" });
        let request = json!({ "title": "Run build", "decision_reason": "Not in allowlist" });
        assert_eq!(approval_prompt("Bash", &input, &request), "make build");
        assert_eq!(
            approval_reason("Bash", &input, &request).as_deref(),
            Some("Not in allowlist")
        );
        let request = json!({ "title": "T", "description": "D", "decision_reason": "R" });
        assert_eq!(approval_prompt("Other", &json!({}), &request), "T");
        let request = json!({ "description": "D", "decision_reason": "R" });
        assert_eq!(approval_prompt("Other", &json!({}), &request), "D");
        // A reason that only repeats the prompt is dropped.
        let request = json!({ "decision_reason": "make build" });
        assert_eq!(approval_reason("Bash", &input, &request), None);
        assert_eq!(
            approval_prompt("Read", &json!({ "file_path": "/a/b" }), &json!({})),
            "Read: /a/b"
        );
        let long = "x".repeat(500);
        let summary = approval_prompt("Tool", &json!({ "q": long }), &json!({}));
        assert!(summary.ends_with("...") && summary.chars().count() <= 410);
    }

    #[test]
    fn tool_result_meta_names_the_non_execution_kind() {
        let message = json!({ "tool_result_meta": [
            { "id": "a", "non_execution_kind": "interrupted" },
            { "id": "b", "non_execution_kind": null },
        ] });
        assert_eq!(non_execution_kind(&message, "a"), Some("interrupted"));
        assert_eq!(non_execution_kind(&message, "b"), None);
        assert_eq!(non_execution_kind(&json!({}), "a"), None);
    }

    #[test]
    fn pictures_go_first_as_base64_blocks_then_the_text() {
        let image = |media_type: &str, data: &str| hoh_external_harness::PromptImage {
            media_type: media_type.into(),
            data: data.into(),
            name: Some("a.png".into()),
        };
        let mut params = PromptParams {
            text: "what is on it?".into(),
            images: vec![image("image/png", "iVBO"), image("image/webp", "UklG")],
            ..Default::default()
        };
        assert_eq!(
            prompt_content(&params),
            json!([
                { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "iVBO" } },
                { "type": "image", "source": { "type": "base64", "media_type": "image/webp", "data": "UklG" } },
                { "type": "text", "text": "what is on it?" },
            ])
        );
        params.text.clear();
        assert_eq!(prompt_content(&params).as_array().map(Vec::len), Some(2));
        params.images.clear();
        assert_eq!(
            prompt_content(&params),
            json!([{ "type": "text", "text": "" }])
        );
    }
}

#[cfg(test)]
mod round2_tests {
    use super::*;

    #[test]
    fn terminal_escapes_are_stripped_from_the_prompt_and_the_reason() {
        assert_eq!(
            strip_terminal_escapes("\u{1b}[31mNot allowed\u{1b}[0m"),
            "Not allowed"
        );
        assert_eq!(strip_terminal_escapes("\u{1b}]0;title\u{7}ok"), "ok");
        let request = json!({ "decision_reason": "  \u{1b}[1mNot in allowlist\u{1b}[22m \n" });
        let input = json!({ "command": "make" });
        assert_eq!(
            approval_reason("Bash", &input, &request).as_deref(),
            Some("Not in allowlist")
        );
        // As in T3: the prompt is the title as it is (escapes and all), the reason is stripped
        // and differs from it.
        let request = json!({ "title": "\u{1b}[2mmake\u{1b}[0m", "decision_reason": "make" });
        assert_eq!(
            approval_reason("Other", &json!({}), &request).as_deref(),
            Some("make")
        );
        // An empty title is the prompt (`??`), so the reason is shown.
        let request = json!({ "title": "", "decision_reason": "X" });
        assert_eq!(approval_prompt("Other", &json!({}), &request), "");
        assert_eq!(
            approval_reason("Other", &json!({}), &request).as_deref(),
            Some("X")
        );
    }

    #[test]
    fn signals_are_named() {
        assert_eq!(
            crash_message(Some(9), ""),
            "Claude Code process terminated by signal SIGKILL"
        );
        assert_eq!(
            crash_message(Some(6), " boom\n"),
            "Claude Code process terminated by signal SIGABRT: boom"
        );
        assert_eq!(
            crash_message(Some(40), ""),
            "Claude Code process terminated by signal 40"
        );
        assert_eq!(crash_message(None, "oops"), "oops");
    }

    #[test]
    fn answers_join_several_choices_and_custom_text_and_skip_unknown_ids() {
        let questions = vec!["Which?".to_owned(), "Why?".to_owned()];
        let answers = vec![
            QuestionAnswer {
                id: "q0".into(),
                selected: vec!["A".into(), "B".into()],
                custom: Some("other".into()),
            },
            QuestionAnswer {
                id: "q7".into(),
                selected: vec!["X".into()],
                custom: None,
            },
            QuestionAnswer {
                id: "nope".into(),
                selected: vec!["Y".into()],
                custom: None,
            },
        ];
        assert_eq!(
            answers_by_question(&questions, &answers),
            json!({ "Which?": "A, B, other" })
        );
    }

    #[test]
    fn the_reason_is_cut_at_the_limit_of_the_summary() {
        let request = json!({ "decision_reason": "x".repeat(100 * 1024) });
        let reason = approval_reason("Bash", &json!({ "command": "make" }), &request).unwrap();
        assert_eq!(reason.chars().count(), 400);
    }

    #[test]
    fn the_single_character_csi_is_stripped() {
        assert_eq!(strip_terminal_escapes("\u{9b}31mX"), "X");
    }

    #[test]
    fn the_folder_of_a_command_is_the_session_only_for_a_plain_line() {
        let session = std::path::Path::new("/w/proj");
        for plain in [
            "ls",
            "ls -la",
            "cat a.txt",
            "rg foo src",
            "touch new.txt",
            "echo abcd",
            "grep -C3 foo a",
            "ls .",
            "grep -r x .",
            "git commit -m \"x\"",
            "git commit -m 'fix: x'",
        ] {
            assert_eq!(command_cwd(plain, session), "/w/proj", "{plain}");
        }
        for unknown in [
            "cd sub && make",
            "/bin/sh x.sh",
            "/usr/bin/env cd /etc",
            "/usr/bin/sudo ls",
            "make -C /etc install",
            "git -C /home/u/other reset --hard",
            "tar -C / -xf a.tar",
            "tar -C/ -xf a.tar",
            "make --directory=/etc install",
            "npm --prefix /etc i",
            "git --git-dir /x/.git status",
            "git --work-tree /etc checkout .",
            "tool --cwd /etc run",
            "tool --chdir /etc run",
            "java -chdir /etc x",
            "cd /tmp; rm x",
            "\\cd /tmp && rm x",
            "c''d /tmp && touch x",
            "c\\d /tmp && touch x",
            "c$1d /etc && touch x",
            "c${u}d /etc && touch x",
            "c$()d /etc && touch x",
            "c$@d /etc && touch x",
            "c=cd; $c /etc && touch x",
            "c=cd; builtin $c /etc && touch x",
            "c=cd; X=1 $c /etc && touch x",
            "c=cd; if true; then $c /etc; fi; touch x",
            "pushd /etc; echo x > cron.d/job",
            "eval \"cd /etc\"; echo x > cron.d/job",
            "cd",
            "cd /tmp",
            "'cd' /etc",
            "c\"\"d /etc",
            "popd",
            "exec bash",
            ". ./env.sh",
            "source ./env.sh",
            "env cd /etc",
            "xargs rm",
            "sh x.sh",
            "bash x.sh",
            "zsh x.sh",
            "time ls",
            "nohup ls",
            "sudo ls",
            "command ls",
            "ls | wc",
            "ls > out",
            "echo $HOME",
            "ls\nrm x",
        ] {
            assert_eq!(command_cwd(unknown, session), "/", "{unknown:?}");
        }
    }

    #[test]
    fn the_stripping_is_that_of_node() {
        for (input, expected) in [
            ("\u{1b}(B ok", " ok"),
            ("\u{1b}[31mX\u{1b}[0m", "X"),
            ("\u{9b}31mX", "X"),
            ("\u{1b}]0;title\u{7}ok", "ok"),
            ("\u{1b}]8;;http://x\u{1b}\\link\u{1b}]8;;\u{1b}\\", "link"),
            // What the pattern does not match stays, as in Node.
            ("a\rb\u{7}c", "a\rb\u{7}c"),
            ("plain", "plain"),
        ] {
            assert_eq!(strip_terminal_escapes(input), expected, "{input:?}");
        }
    }

    #[test]
    fn the_summary_keeps_the_order_of_keys_and_counts_utf16_units() {
        let input: Value = serde_json::from_str(r#"{"z":1,"a":{"y":2,"b":3}}"#).unwrap();
        assert_eq!(
            summarize_tool("Tool", &input),
            r#"Tool: {"z":1,"a":{"y":2,"b":3}}"#
        );
        // 300 emoji are 600 UTF-16 units: JS `slice(0, 400)` keeps 200 of them.
        let command = "\u{1f600}".repeat(300);
        let summary = summarize_tool("Bash", &json!({ "command": command }));
        assert_eq!(summary, format!("Bash: {}", "\u{1f600}".repeat(200)));
    }

    #[test]
    fn the_folder_shown_is_the_session_folder_while_the_list_is_off() {
        let session = std::path::Path::new("/w/proj");
        let shown = |command: &str| {
            let input = json!({ "command": command });
            match tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
                .block_on(action_for("Bash", &input, session))
            {
                HarnessAction::Command { cwd, .. } => cwd,
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(shown("echo \"a\"").as_deref(), Some("/w/proj"));
        assert_eq!(shown("ls | wc").as_deref(), Some("/w/proj"));
        assert_eq!(shown("cd /tmp && rm x").as_deref(), Some("/w/proj"));
    }

    /// A session whose process closed its stdin: nothing written to it arrives.
    async fn dead_stdin(
        interrupt_timeout: Duration,
    ) -> (Arc<ClaudeSession>, mpsc::Receiver<HarnessEvent>) {
        dead_stdin_with(interrupt_timeout, 64).await
    }

    async fn dead_stdin_with(
        interrupt_timeout: Duration,
        queue: usize,
    ) -> (Arc<ClaudeSession>, mpsc::Receiver<HarnessEvent>) {
        let spec = hoh_external_harness::ProcessSpec {
            program: PathBuf::from("sh"),
            args: vec!["-c".into(), "exec 0<&-; exec sleep 30".into()],
            cwd: None,
            env: Vec::new(),
            env_remove: Vec::new(),
        };
        let sup = Supervisor::spawn(
            &spec,
            hoh_external_harness::SupervisorLimits::default(),
            None,
        )
        .unwrap();
        let (tx, rx) = mpsc::channel(queue);
        let inner = Arc::new(Inner {
            sup,
            events: Mutex::new(Some(tx)),
            state: Mutex::new(State {
                native: "n".into(),
                ..State::default()
            }),
            cwd: PathBuf::from("/w/proj"),
            next_request: AtomicU64::new(1),
            terminated: AtomicBool::new(false),
            interrupt_timeout,
            config_dir: None,
            bypass: false,
            timers: Mutex::new(Vec::new()),
        });
        // The first write into the dead pipe only finds out that it is dead.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _ = inner.send(json!({ "type": "probe" })).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        (Arc::new(ClaudeSession { inner }), rx)
    }

    #[tokio::test]
    async fn the_watcher_of_an_interrupt_does_not_outlive_the_session() {
        let (session, _events) = dead_stdin(Duration::from_secs(60)).await;
        lock(&session.inner.state).turn = Some("t1".into());
        spawn_interrupt_watch(&session.inner, "t1".into());
        assert_eq!(Arc::strong_count(&session.inner), 2);
        session.terminate(Duration::from_millis(200)).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            Arc::strong_count(&session.inner),
            1,
            "the sleeping watcher still holds the session"
        );
    }

    #[tokio::test]
    async fn a_failed_send_of_a_prompt_is_reported_once_by_the_error() {
        let (session, mut events) = dead_stdin(Duration::from_secs(60)).await;
        let error = session
            .prompt(PromptParams {
                session_id: "s".into(),
                turn_id: "t1".into(),
                text: "hi".into(),
                ..Default::default()
            })
            .await
            .expect_err("the prompt cannot reach the process");
        assert_eq!(error.code, code::CLOSED);
        let mut seen = Vec::new();
        while let Ok(event) = events.try_recv() {
            seen.push(event);
        }
        assert!(
            !seen
                .iter()
                .any(|e| matches!(e, HarnessEvent::TurnFailed { .. })),
            "{seen:?}"
        );
        assert!(lock(&session.inner.state).turn.is_none());
        session.terminate(Duration::from_millis(200)).await;
    }

    #[tokio::test]
    async fn an_answer_racing_the_end_of_the_session_releases_its_request_once() {
        let (session, mut events) = dead_stdin(Duration::from_secs(60)).await;
        lock(&session.inner.state).pending.insert(
            "r1".into(),
            Pending::Approval {
                input: json!({}),
                tool_use_id: None,
                native_id: "r1".into(),
            },
        );
        let respond = session.respond(RespondParams {
            session_id: "s".into(),
            request_id: "r1".into(),
            response: HarnessResponse::Approval {
                decision: ApprovalDecision::Allow,
            },
        });
        let terminate = session.terminate(Duration::from_millis(200));
        let (answer, ()) = tokio::join!(respond, terminate);
        assert!(answer.is_err());
        let mut released = 0;
        while let Ok(event) = events.try_recv() {
            released += usize::from(
                matches!(event, HarnessEvent::RequestResolved { request_id } if request_id == "r1"),
            );
        }
        assert_eq!(released, 1);
    }

    #[tokio::test]
    async fn the_end_of_the_stream_does_not_take_the_settlement_of_an_interrupt_away() {
        // A queue of one that nobody reads holds the watcher while it reports; the end of the
        // stream then aborts the watchers. The turn still gets its end and the session its.
        let (session, mut events) = dead_stdin_with(Duration::from_millis(50), 1).await;
        {
            let mut state = lock(&session.inner.state);
            state.turn = Some("t1".into());
            state.started.insert("i1".into());
            state.started.insert("i2".into());
        }
        spawn_interrupt_watch(&session.inner, "t1".into());
        tokio::time::sleep(Duration::from_millis(300)).await;
        abort_timers(&session.inner);
        let mut aborted = 0;
        let mut ended = 0;
        loop {
            match tokio::time::timeout(Duration::from_secs(2), events.recv()).await {
                Ok(Some(HarnessEvent::TurnAborted { .. })) => aborted += 1,
                Ok(Some(HarnessEvent::SessionEnded { .. })) => {
                    ended += 1;
                    break;
                }
                Ok(Some(_)) => {}
                _ => break,
            }
        }
        assert_eq!((aborted, ended), (1, 1));
    }
}
