//! One supervised `codex app-server` process and its newline-delimited JSON-RPC lifecycle.

use std::collections::{HashMap, HashSet, hash_map::Entry};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures_util::future::BoxFuture;
use hoh_external_harness::{
    ApprovalDecision, EndReason, FileChange, FileChangeKind, Frame, HarnessAction, HarnessEvent,
    HarnessFailure, HarnessQuestion, HarnessResponse, HarnessSession, ItemOutcome, PromptParams,
    QuestionOption, RespondParams, Supervisor, code,
};
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot};

/// Explicit per-process trust masks previously trusted parent entries for an untrusted folder.
pub(crate) fn project_trust(cwd: &std::path::Path, trusted: bool) -> Map<String, Value> {
    let mut projects = Map::new();
    for ancestor in cwd.ancestors() {
        let git_root = ancestor.join(".git").exists();
        if !trusted || ancestor == cwd || git_root {
            projects.insert(
                ancestor.to_string_lossy().into_owned(),
                json!({"trust_level": if trusted { "trusted" } else { "untrusted" }}),
            );
        }
        if trusted && git_root {
            break;
        }
    }
    projects
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn parent_item_id(value: &Value) -> Option<String> {
    string(value, "parentItemId").or_else(|| string(value, "parent_item_id"))
}

fn event_parent_item_id(params: &Value) -> Option<String> {
    parent_item_id(params).or_else(|| params.get("item").and_then(parent_item_id))
}

fn rpc_key(id: &Value) -> String {
    serde_json::to_string(id).unwrap_or_else(|_| "null".to_owned())
}

fn is_typed_request_id(id: &Value) -> bool {
    match id {
        Value::String(_) => true,
        Value::Number(number) => number.as_i64().is_some(),
        _ => false,
    }
}

fn native_turn_of(params: &Value) -> Option<String> {
    string(params, "turnId").or_else(|| {
        params
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .map(str::to_owned)
    })
}

fn current_thread_context(state: &State, params: &Value) -> Result<RequestContext, String> {
    if state.closed {
        return Err("Codex sent a turn-scoped message after the session closed".to_owned());
    }
    let hoh_turn = state
        .hoh_turn
        .clone()
        .filter(|_| state.turn_opened)
        .ok_or_else(|| "Codex sent a turn-scoped message without an active HoH turn".to_owned())?;
    let native_turn = state.native_turn.clone().ok_or_else(|| {
        "Codex sent a turn-scoped message before the native turn opened".to_owned()
    })?;
    let thread_id = string(params, "threadId")
        .ok_or_else(|| "Codex turn-scoped message has no threadId".to_owned())?;
    if thread_id != state.thread_id {
        return Err("Codex turn-scoped message belongs to another thread".to_owned());
    }
    Ok(RequestContext {
        thread_id,
        native_turn,
        hoh_turn,
    })
}

fn active_context(state: &State, params: &Value) -> Result<RequestContext, String> {
    let context = current_thread_context(state, params)?;
    let message_turn = native_turn_of(params)
        .ok_or_else(|| "Codex turn-scoped message has no turnId".to_owned())?;
    if message_turn != context.native_turn {
        return Err("Codex turn-scoped message belongs to another turn".to_owned());
    }
    Ok(context)
}

fn elicitation_context(state: &State, params: &Value) -> Result<Option<RequestContext>, String> {
    if state.closed {
        return Err("Codex sent an MCP elicitation after the session closed".to_owned());
    }
    let thread_id = string(params, "threadId")
        .ok_or_else(|| "Codex MCP elicitation has no threadId".to_owned())?;
    if thread_id != state.thread_id {
        return Err("Codex MCP elicitation belongs to another thread".to_owned());
    }
    let Some(turn_id) = params.get("turnId") else {
        return if state.turn_opened && state.hoh_turn.is_some() && state.native_turn.is_some() {
            current_thread_context(state, params).map(Some)
        } else {
            Ok(None)
        };
    };
    if turn_id.is_null() {
        return if state.turn_opened && state.hoh_turn.is_some() && state.native_turn.is_some() {
            current_thread_context(state, params).map(Some)
        } else {
            Ok(None)
        };
    }
    let Some(message_turn) = turn_id.as_str() else {
        return Err("Codex MCP elicitation has an invalid turnId".to_owned());
    };
    let context = current_thread_context(state, params)?;
    if message_turn != context.native_turn {
        return Err("Codex MCP elicitation belongs to another turn".to_owned());
    }
    Ok(Some(context))
}

fn context_is_current(state: &State, context: &RequestContext) -> bool {
    !state.closed
        && state.turn_opened
        && state.thread_id == context.thread_id
        && state.native_turn.as_deref() == Some(context.native_turn.as_str())
        && state.hoh_turn.as_deref() == Some(context.hoh_turn.as_str())
}

#[derive(Clone, Debug)]
pub(crate) struct ModeSettings {
    pub approval_policy: Value,
    pub reviewer: &'static str,
    pub sandbox: &'static str,
    pub sandbox_policy: Value,
}

pub(crate) fn mode_settings(access: hoh_external_harness::AccessMode) -> ModeSettings {
    use hoh_external_harness::AccessMode;
    match access {
        AccessMode::Confirm => ModeSettings {
            approval_policy: json!("untrusted"),
            reviewer: "user",
            sandbox: "workspace-write",
            sandbox_policy: json!({"type":"workspaceWrite"}),
        },
        AccessMode::AutoAcceptEdits | AccessMode::AcceptEdits => ModeSettings {
            approval_policy: json!("on-request"),
            reviewer: "user",
            sandbox: "workspace-write",
            sandbox_policy: json!({"type":"workspaceWrite"}),
        },
        AccessMode::Auto => ModeSettings {
            approval_policy: json!("on-request"),
            reviewer: "auto_review",
            sandbox: "workspace-write",
            sandbox_policy: json!({"type":"workspaceWrite"}),
        },
        AccessMode::Full => ModeSettings {
            approval_policy: json!({"granular":{
                "mcp_elicitations":true,
                "request_permissions":false,
                "rules":false,
                "sandbox_approval":false,
                "skill_approval":false
            }}),
            reviewer: "user",
            sandbox: "danger-full-access",
            sandbox_policy: json!({"type":"dangerFullAccess"}),
        },
    }
}

type HostReply = oneshot::Sender<Result<Value, HarnessFailure>>;

#[derive(Clone)]
enum ServerRequestKind {
    CommandOrFile,
    Permissions { requested: Value },
    Question { ids: Vec<String> },
    McpElicitation,
}

#[derive(Clone)]
struct ServerRequest {
    id: Value,
    kind: ServerRequestKind,
    context: RequestContext,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RequestContext {
    thread_id: String,
    native_turn: String,
    hoh_turn: String,
}

#[derive(Default)]
struct State {
    host: HashMap<u64, HostReply>,
    server: HashMap<String, ServerRequest>,
    /// Native ids accepted during this app-server process, including requests the human already
    /// answered. `serverRequest/resolved` has no turn id, so reuse across turns is ambiguous and
    /// must fail closed rather than letting a late notification resolve a newer request.
    server_seen: HashMap<String, RequestContext>,
    /// Standalone MCP elicitations without an active turn are cancelled on their exact native id.
    /// Their ids remain reserved for the lifetime of this app-server process too.
    server_standalone_seen: HashSet<String>,
    items: HashMap<String, HarnessAction>,
    /// Parent tool call carried by a nested item. Later output, approvals and completion often
    /// name only the item id, so retain the relationship for the whole item lifecycle.
    item_parents: HashMap<String, String>,
    items_seen: HashSet<String>,
    thread_id: String,
    hoh_turn: Option<String>,
    native_turn: Option<String>,
    turn_opened: bool,
    /// Stop may arrive while `turn/start` is still in flight. Remember it until the canonical
    /// `turn/started` notification opens the native turn; its result alone is too early to stop.
    interrupt_requested: bool,
    interrupt_sent: bool,
    closed: bool,
}

fn take_pending_interrupt(state: &mut State) -> Option<RequestContext> {
    if state.closed || !state.turn_opened || !state.interrupt_requested || state.interrupt_sent {
        return None;
    }
    let native_turn = state.native_turn.clone()?;
    let hoh_turn = state.hoh_turn.clone()?;
    state.interrupt_sent = true;
    Some(RequestContext {
        thread_id: state.thread_id.clone(),
        native_turn,
        hoh_turn,
    })
}

struct Inner {
    supervisor: Arc<Supervisor>,
    events: Mutex<Option<mpsc::Sender<HarnessEvent>>>,
    state: Mutex<State>,
    next_request: AtomicU64,
    request_timeout: Duration,
    terminated: AtomicBool,
    mode: ModeSettings,
    cwd: String,
    trusted: bool,
    model: Option<String>,
    effort: Option<String>,
}

pub(crate) struct CodexSession {
    inner: Arc<Inner>,
    owner: Arc<Owner>,
}

struct Owner(Arc<Supervisor>);

impl Drop for Owner {
    fn drop(&mut self) {
        self.0.abandon();
    }
}

pub(crate) struct StartSettings {
    pub event_queue: usize,
    pub request_timeout: Duration,
    pub mode: ModeSettings,
    pub cwd: String,
    pub trusted: bool,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub resume: Option<String>,
}

pub(crate) struct Probe {
    pub models: Vec<Value>,
    pub account: Value,
}

impl CodexSession {
    async fn open(
        supervisor: Supervisor,
        event_queue: usize,
        request_timeout: Duration,
        mode: ModeSettings,
        cwd: String,
        trusted: bool,
        model: Option<String>,
        effort: Option<String>,
    ) -> Result<(Arc<Self>, mpsc::Receiver<HarnessEvent>), HarnessFailure> {
        let frames = supervisor
            .take_frames()
            .ok_or_else(|| HarnessFailure::new(code::CLOSED, "process frames are taken"))?;
        let (events_tx, events_rx) = mpsc::channel(event_queue.max(1));
        let supervisor = Arc::new(supervisor);
        let owner = Arc::new(Owner(supervisor.clone()));
        let inner = Arc::new(Inner {
            supervisor,
            events: Mutex::new(Some(events_tx)),
            state: Mutex::new(State::default()),
            next_request: AtomicU64::new(1),
            request_timeout,
            terminated: AtomicBool::new(false),
            mode,
            cwd,
            trusted,
            model,
            effort,
        });
        tokio::spawn(read_loop(inner.clone(), frames));
        let session = Arc::new(Self { inner, owner });
        session
            .request(
                "initialize",
                json!({
                    "clientInfo": {"name":"hoh","title":"HarnessOverHarness","version":env!("CARGO_PKG_VERSION")},
                    "capabilities": {"experimentalApi": true}
                }),
            )
            .await?;
        session.notify("initialized", json!({})).await?;
        Ok((session, events_rx))
    }

    pub(crate) async fn start(
        supervisor: Supervisor,
        settings: StartSettings,
    ) -> Result<(Arc<Self>, mpsc::Receiver<HarnessEvent>, String, bool), HarnessFailure> {
        let StartSettings {
            event_queue,
            request_timeout,
            mode,
            cwd,
            trusted,
            model,
            effort,
            resume,
        } = settings;
        let (session, events) = Self::open(
            supervisor,
            event_queue,
            request_timeout,
            mode,
            cwd,
            trusted,
            model,
            effort,
        )
        .await?;
        let mut params = session.thread_policy();
        let requested_resume = resume.clone();
        let resumed = requested_resume.is_some();
        let method = if let Some(thread_id) = resume {
            params["threadId"] = json!(thread_id);
            params["excludeTurns"] = json!(false);
            "thread/resume"
        } else {
            "thread/start"
        };
        let result = session.request(method, params).await.map_err(|failure| {
            if resumed
                && (failure.message.contains("not found")
                    || failure.message.contains("No rollout found")
                    || failure.message.contains("archived"))
            {
                HarnessFailure::new(code::SESSION_NOT_FOUND, failure.message)
            } else {
                failure
            }
        })?;
        let thread_id = result
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                HarnessFailure::new(code::PROTOCOL, "thread start did not return thread.id")
            })?
            .to_owned();
        if let Some(requested) = requested_resume
            && thread_id != requested
        {
            session.terminate_now(Duration::ZERO).await;
            return Err(HarnessFailure::new(
                code::PROTOCOL,
                format!(
                    "thread/resume returned a different thread.id: requested {requested:?}, got {thread_id:?}"
                ),
            ));
        }
        lock(&session.inner.state).thread_id.clone_from(&thread_id);
        Ok((session, events, thread_id, resumed))
    }

    pub(crate) async fn probe(
        supervisor: Supervisor,
        request_timeout: Duration,
    ) -> Result<Probe, HarnessFailure> {
        let (session, _events) = Self::open(
            supervisor,
            1,
            request_timeout,
            mode_settings(hoh_external_harness::AccessMode::Confirm),
            std::env::temp_dir().to_string_lossy().into_owned(),
            false,
            None,
            None,
        )
        .await?;
        let mut models = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..32 {
            let answer = session
                .request("model/list", json!({"cursor":cursor,"limit":100}))
                .await?;
            models.extend(
                answer
                    .get("data")
                    .or_else(|| answer.get("models"))
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            );
            cursor = answer
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        let account = session
            .request("account/read", json!({"refreshToken":false}))
            .await?;
        session.terminate_now(Duration::from_millis(300)).await;
        Ok(Probe { models, account })
    }

    fn thread_policy(&self) -> Value {
        let mode = &self.inner.mode;
        let mut params = json!({
            "cwd": self.inner.cwd,
            "approvalPolicy": mode.approval_policy,
            "approvalsReviewer": mode.reviewer,
            "sandbox": mode.sandbox,
            "config": {"projects": project_trust(std::path::Path::new(&self.inner.cwd), self.inner.trusted)}
        });
        if let Some(model) = &self.inner.model {
            params["model"] = json!(model);
        }
        params
    }

    fn turn_params(&self, text: &str) -> Value {
        let state = lock(&self.inner.state);
        let mode = &self.inner.mode;
        let mut params = json!({
            "threadId": state.thread_id,
            "input": [{"type":"text","text":text}],
            "cwd": self.inner.cwd,
            "approvalPolicy": mode.approval_policy,
            "approvalsReviewer": mode.reviewer,
            "sandboxPolicy": mode.sandbox_policy,
            "summary": "auto"
        });
        drop(state);
        // Supervised commands still require approval, but an approved command needs the
        // project's writable sandbox rather than a read-only sandbox and a second escalation.
        if mode.approval_policy == json!("untrusted") {
            params["sandboxPolicy"]["writableRoots"] = json!([self.inner.cwd]);
        }
        if let Some(model) = &self.inner.model {
            params["model"] = json!(model);
        }
        if let Some(effort) = &self.inner.effort {
            params["effort"] = json!(effort);
        }
        params
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, HarnessFailure> {
        request(&self.inner, method, params).await
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), HarnessFailure> {
        self.inner
            .supervisor
            .send(&json!({"method":method,"params":params}))
            .await
    }

    async fn send_server_result(&self, id: Value, result: Value) -> Result<(), HarnessFailure> {
        self.inner
            .supervisor
            .send(&json!({"id":id,"result":result}))
            .await
    }

    pub(crate) async fn terminate_now(&self, grace: Duration) {
        if !self.inner.terminated.swap(true, Ordering::SeqCst) {
            self.inner.supervisor.terminate(grace).await;
        }
    }
}

async fn request(inner: &Arc<Inner>, method: &str, params: Value) -> Result<Value, HarnessFailure> {
    let id = inner.next_request.fetch_add(1, Ordering::Relaxed);
    let (reply, answer) = oneshot::channel();
    {
        let mut state = lock(&inner.state);
        if state.closed {
            return Err(HarnessFailure::new(
                code::CLOSED,
                "Codex app-server is closed",
            ));
        }
        state.host.insert(id, reply);
    }
    if let Err(error) = inner
        .supervisor
        .send(&json!({"id":id,"method":method,"params":params}))
        .await
    {
        lock(&inner.state).host.remove(&id);
        return Err(error);
    }
    match tokio::time::timeout(inner.request_timeout, answer).await {
        Ok(Ok(answer)) => answer,
        Ok(Err(_)) => Err(HarnessFailure::new(
            code::CLOSED,
            format!("Codex app-server closed while waiting for {method}"),
        )),
        Err(_) => {
            lock(&inner.state).host.remove(&id);
            Err(HarnessFailure::new(
                code::PROTOCOL,
                format!("Codex app-server did not answer {method}"),
            ))
        }
    }
}

async fn send_interrupt(
    inner: &Arc<Inner>,
    context: &RequestContext,
) -> Result<(), HarnessFailure> {
    request(
        inner,
        "turn/interrupt",
        json!({"threadId":context.thread_id.clone(),"turnId":context.native_turn.clone()}),
    )
    .await?;
    Ok(())
}

fn interrupt_failure_is_current(inner: &Inner, context: &RequestContext) -> bool {
    context_is_current(&lock(&inner.state), context)
}

impl HarnessSession for CodexSession {
    fn prompt(&self, params: PromptParams) -> BoxFuture<'static, Result<(), HarnessFailure>> {
        let session = Arc::new(Self {
            inner: self.inner.clone(),
            owner: self.owner.clone(),
        });
        Box::pin(async move {
            {
                let mut state = lock(&session.inner.state);
                if state.hoh_turn.is_some() {
                    return Err(HarnessFailure::new(
                        code::PROTOCOL,
                        "Codex already has a running turn",
                    ));
                }
                state.hoh_turn = Some(params.turn_id.clone());
                state.native_turn = None;
                state.turn_opened = false;
                state.interrupt_requested = false;
                state.interrupt_sent = false;
                state.items.clear();
                state.item_parents.clear();
                state.items_seen.clear();
            }
            let result = session
                .request("turn/start", session.turn_params(&params.text))
                .await;
            let result = match result {
                Ok(result) => result,
                Err(error) => {
                    let mut state = lock(&session.inner.state);
                    if state.hoh_turn.as_deref() == Some(params.turn_id.as_str()) {
                        state.hoh_turn = None;
                        state.native_turn = None;
                        state.turn_opened = false;
                        state.interrupt_requested = false;
                        state.interrupt_sent = false;
                    }
                    return Err(error);
                }
            };
            let native = result
                .pointer("/turn/id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let Some(native) = native else {
                let mut state = lock(&session.inner.state);
                if state.hoh_turn.as_deref() == Some(params.turn_id.as_str()) {
                    state.hoh_turn = None;
                    state.native_turn = None;
                    state.turn_opened = false;
                    state.interrupt_requested = false;
                    state.interrupt_sent = false;
                }
                return Err(HarnessFailure::new(
                    code::PROTOCOL,
                    "turn/start returned no turn.id",
                ));
            };
            let mismatch = {
                let mut state = lock(&session.inner.state);
                if state.hoh_turn.as_deref() != Some(params.turn_id.as_str()) {
                    // The latched interrupt may have completed before the turn/start response, or
                    // a replacement turn may already exist. Never reopen or corrupt either state.
                    false
                } else if state
                    .native_turn
                    .as_ref()
                    .is_some_and(|turn| turn != &native)
                {
                    true
                } else {
                    // `turn/start` returning an id does not mean app-server accepts
                    // `turn/interrupt` yet. Only `turn/started` opens and announces the turn.
                    state.native_turn = Some(native);
                    false
                }
            };
            if mismatch {
                protocol_violation(
                    &session.inner,
                    "turn/start returned a turn.id different from turn/started",
                )
                .await;
                return Err(HarnessFailure::new(
                    code::PROTOCOL,
                    "Codex app-server changed the active turn id",
                ));
            }
            Ok(())
        })
    }

    fn interrupt(&self) -> BoxFuture<'static, Result<(), HarnessFailure>> {
        let session = Arc::new(Self {
            inner: self.inner.clone(),
            owner: self.owner.clone(),
        });
        Box::pin(async move {
            let interrupt = {
                let mut state = lock(&session.inner.state);
                if state.hoh_turn.is_none() {
                    return Ok(());
                }
                state.interrupt_requested = true;
                take_pending_interrupt(&mut state)
            };
            let Some(context) = interrupt else {
                return Ok(());
            };
            match send_interrupt(&session.inner, &context).await {
                Err(failure) if interrupt_failure_is_current(&session.inner, &context) => {
                    Err(failure)
                }
                _ => Ok(()),
            }
        })
    }

    fn respond(&self, params: RespondParams) -> BoxFuture<'static, Result<(), HarnessFailure>> {
        let session = Arc::new(Self {
            inner: self.inner.clone(),
            owner: self.owner.clone(),
        });
        Box::pin(async move {
            let pending = {
                let mut state = lock(&session.inner.state);
                let pending = state
                    .server
                    .get(&params.request_id)
                    .cloned()
                    .ok_or_else(|| {
                        HarnessFailure::new(code::PROTOCOL, "Codex request is no longer pending")
                    })?;
                if !context_is_current(&state, &pending.context) {
                    None
                } else {
                    Some(
                        state
                            .server
                            .remove(&params.request_id)
                            .expect("pending request"),
                    )
                }
            };
            let Some(pending) = pending else {
                protocol_violation(
                    &session.inner,
                    "an answer targeted a Codex request from another turn",
                )
                .await;
                return Err(HarnessFailure::new(
                    code::PROTOCOL,
                    "Codex request does not belong to the active turn",
                ));
            };
            let result = match (pending.kind, params.response) {
                (ServerRequestKind::CommandOrFile, HarnessResponse::Approval { decision }) => {
                    json!({"decision": if decision == ApprovalDecision::Allow {"accept"} else {"decline"}})
                }
                (
                    ServerRequestKind::Permissions { requested },
                    HarnessResponse::Approval { decision },
                ) => {
                    json!({"permissions": if decision == ApprovalDecision::Allow {requested} else {json!({})}, "scope":"turn"})
                }
                (ServerRequestKind::Question { ids }, HarnessResponse::Answer { answers }) => {
                    let mut output = Map::new();
                    for answer in answers {
                        if !ids.contains(&answer.id) {
                            continue;
                        }
                        let mut values = answer.selected;
                        if let Some(custom) = answer.custom.filter(|value| !value.is_empty()) {
                            values.push(custom);
                        }
                        output.insert(answer.id, json!({"answers":values}));
                    }
                    json!({"answers":output})
                }
                (ServerRequestKind::McpElicitation, HarnessResponse::Approval { decision }) => {
                    json!({"action": if decision == ApprovalDecision::Allow {"accept"} else {"decline"}})
                }
                (ServerRequestKind::CommandOrFile, HarnessResponse::Cancelled) => {
                    json!({"decision":"cancel"})
                }
                (ServerRequestKind::Permissions { .. }, HarnessResponse::Cancelled) => {
                    json!({"permissions":{},"scope":"turn"})
                }
                (ServerRequestKind::Question { .. }, HarnessResponse::Cancelled) => {
                    json!({"answers":{}})
                }
                (ServerRequestKind::McpElicitation, HarnessResponse::Cancelled) => {
                    json!({"action":"cancel"})
                }
                _ => {
                    return Err(HarnessFailure::new(
                        code::PROTOCOL,
                        "answer kind does not match the Codex request",
                    ));
                }
            };
            session.send_server_result(pending.id, result).await
        })
    }

    fn terminate(&self, grace: Duration) -> BoxFuture<'static, ()> {
        let session = Arc::new(Self {
            inner: self.inner.clone(),
            owner: self.owner.clone(),
        });
        Box::pin(async move { session.terminate_now(grace).await })
    }

    fn diagnostics(&self) -> String {
        hoh_external_harness::redact::redact(&self.inner.supervisor.stderr_tail())
    }
}

async fn emit_event(inner: &Inner, event: HarnessEvent) {
    let sender = lock(&inner.events).clone();
    if let Some(sender) = sender {
        let _ = sender.send(event).await;
    }
}

async fn read_loop(inner: Arc<Inner>, mut frames: mpsc::Receiver<Frame>) {
    while let Some(frame) = frames.recv().await {
        match frame {
            Frame::Message(message) => handle_message(&inner, message).await,
            Frame::Violation(violation) => {
                close(
                    &inner,
                    EndReason::ProtocolError,
                    Some(hoh_external_harness::redact::redact(&violation)),
                )
                .await;
                inner.supervisor.abandon();
                return;
            }
            Frame::Closed { truncated } => {
                let exit = inner.supervisor.wait().await;
                let reason = if exit.success() && !truncated {
                    EndReason::Exited
                } else if truncated {
                    EndReason::StreamClosed
                } else {
                    EndReason::Crashed
                };
                close(
                    &inner,
                    reason,
                    Some(hoh_external_harness::redact::redact(
                        &inner.supervisor.stderr_tail(),
                    )),
                )
                .await;
                return;
            }
        }
    }
}

async fn close(inner: &Inner, reason: EndReason, message: Option<String>) {
    let (host, requests) = {
        let mut state = lock(&inner.state);
        if state.closed {
            return;
        }
        state.closed = true;
        (
            std::mem::take(&mut state.host),
            std::mem::take(&mut state.server),
        )
    };
    finish_close(inner, host, requests, reason, message).await;
}

async fn finish_close(
    inner: &Inner,
    host: HashMap<u64, HostReply>,
    requests: HashMap<String, ServerRequest>,
    reason: EndReason,
    message: Option<String>,
) {
    for (_, reply) in host {
        let _ = reply.send(Err(HarnessFailure::new(
            code::CLOSED,
            "Codex app-server ended",
        )));
    }
    for request_id in requests.into_keys() {
        emit_event(inner, HarnessEvent::RequestResolved { request_id }).await;
    }
    let sender = lock(&inner.events).take();
    if let Some(sender) = sender {
        let _ = sender
            .send(HarnessEvent::SessionEnded {
                reason,
                message: message.filter(|value| !value.trim().is_empty()),
                exit_code: inner
                    .supervisor
                    .exit_status()
                    .and_then(|status| status.code.map(i64::from)),
            })
            .await;
    }
}

async fn protocol_violation(inner: &Arc<Inner>, message: impl Into<String>) {
    close(inner, EndReason::ProtocolError, Some(message.into())).await;
    if !inner.terminated.swap(true, Ordering::SeqCst) {
        inner.supervisor.terminate(Duration::ZERO).await;
    }
}

async fn interrupt_protocol_violation_if_current(
    inner: &Arc<Inner>,
    context: &RequestContext,
    message: String,
) {
    let (host, requests) = {
        let mut state = lock(&inner.state);
        if state.closed || !context_is_current(&state, context) {
            return;
        }
        state.closed = true;
        (
            std::mem::take(&mut state.host),
            std::mem::take(&mut state.server),
        )
    };
    finish_close(
        inner,
        host,
        requests,
        EndReason::ProtocolError,
        Some(message),
    )
    .await;
    if !inner.terminated.swap(true, Ordering::SeqCst) {
        inner.supervisor.terminate(Duration::ZERO).await;
    }
}

async fn handle_message(inner: &Arc<Inner>, message: Value) {
    if message.get("id").is_some()
        && (message.get("result").is_some() || message.get("error").is_some())
        && message.get("method").is_none()
    {
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            return;
        };
        let reply = lock(&inner.state).host.remove(&id);
        if let Some(reply) = reply {
            let result = if let Some(error) = message.get("error") {
                let detail = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Codex app-server request failed");
                Err(HarnessFailure::new(
                    code::HARNESS,
                    hoh_external_harness::redact::redact(detail),
                ))
            } else {
                Ok(message.get("result").cloned().unwrap_or(Value::Null))
            };
            let _ = reply.send(result);
        }
        return;
    }
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return;
    };
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
    if let Some(id) = message.get("id").cloned() {
        handle_server_request(inner, method, id, params).await;
    } else {
        handle_notification(inner, method, params).await;
    }
}

async fn handle_server_request(inner: &Arc<Inner>, method: &str, id: Value, params: Value) {
    enum Registration {
        Event(HarnessEvent),
        CancelStandalone,
    }

    if !matches!(
        method,
        "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "item/permissions/requestApproval"
            | "item/tool/requestUserInput"
            | "mcpServer/elicitation/request"
    ) {
        let _ = inner
            .supervisor
            .send(&json!({"id":id,"error":{"code":-32601,"message":"unsupported client request"}}))
            .await;
        return;
    }
    if !is_typed_request_id(&id) {
        protocol_violation(inner, "Codex server request has a non-string/non-int64 id").await;
        return;
    }
    let request_id = format!("codex:{}", rpc_key(&id));
    let item_id = string(&params, "itemId");
    let reason = string(&params, "reason");
    let registered = {
        let mut state = lock(&inner.state);
        let context = if method == "mcpServer/elicitation/request" {
            elicitation_context(&state, &params)
        } else {
            active_context(&state, &params).map(Some)
        };
        context.and_then(|context| {
            let Some(context) = context else {
                if state.server_seen.contains_key(&request_id)
                    || state.server_standalone_seen.contains(&request_id)
                    || state.server.contains_key(&request_id)
                {
                    return Err(format!(
                        "Codex app-server reused standalone JSON-RPC request id: {}",
                        rpc_key(&id)
                    ));
                }
                state.server_standalone_seen.insert(request_id.clone());
                return Ok(Registration::CancelStandalone);
            };
            let request_parent = item_id
                .as_deref()
                .and_then(|item| state.item_parents.get(item).cloned())
                .or_else(|| parent_item_id(&params));
            let (kind, event) = match method {
                "item/commandExecution/requestApproval" => {
                    let item = item_id
                        .as_deref()
                        .ok_or_else(|| "Codex command approval has no itemId".to_owned())?;
                    let action = state.items.get(item).cloned().ok_or_else(|| {
                        "Codex command approval names an item outside the active turn".to_owned()
                    })?;
                    match &action {
                        HarnessAction::Command { command, cwd }
                            if string(&params, "command")
                                .is_none_or(|requested| requested == command.as_str())
                                && (!params.get("cwd").is_some()
                                    || string(&params, "cwd") == cwd.clone()) => {}
                        _ => {
                            return Err(
                                "Codex command approval reused or changed another action's itemId"
                                    .to_owned(),
                            );
                        }
                    }
                    (
                        ServerRequestKind::CommandOrFile,
                        HarnessEvent::ApprovalRequested {
                            request_id: request_id.clone(),
                            item_id: item_id.clone(),
                            action,
                            reason: reason.clone(),
                            parent_item_id: request_parent.clone(),
                        },
                    )
                }
                "item/fileChange/requestApproval" => {
                    let item = item_id
                        .as_deref()
                        .ok_or_else(|| "Codex file approval has no itemId".to_owned())?;
                    let action = state.items.get(item).cloned().ok_or_else(|| {
                        "Codex file approval names an item outside the active turn".to_owned()
                    })?;
                    match &action {
                        HarnessAction::FileChange { changes }
                            if params.get("changes").is_none()
                                || file_changes(&params) == changes.clone() => {}
                        _ => {
                            return Err(
                                "Codex file approval reused or changed another action's itemId"
                                    .to_owned(),
                            );
                        }
                    }
                    (
                        ServerRequestKind::CommandOrFile,
                        HarnessEvent::ApprovalRequested {
                            request_id: request_id.clone(),
                            item_id: item_id.clone(),
                            action,
                            reason: reason.clone(),
                            parent_item_id: request_parent.clone(),
                        },
                    )
                }
                "item/permissions/requestApproval" => {
                    let requested = params
                        .get("permissions")
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    (
                        ServerRequestKind::Permissions {
                            requested: requested.clone(),
                        },
                        HarnessEvent::ApprovalRequested {
                            request_id: request_id.clone(),
                            item_id: item_id.clone(),
                            action: HarnessAction::Permission {
                                description: reason
                                    .clone()
                                    .unwrap_or_else(|| requested.to_string()),
                            },
                            reason: reason.clone(),
                            parent_item_id: request_parent.clone(),
                        },
                    )
                }
                "item/tool/requestUserInput" => {
                    let questions = questions(&params);
                    let ids = questions
                        .iter()
                        .map(|question| question.id.clone())
                        .collect();
                    (
                        ServerRequestKind::Question { ids },
                        HarnessEvent::QuestionAsked {
                            request_id: request_id.clone(),
                            questions,
                            parent_item_id: request_parent.clone(),
                        },
                    )
                }
                "mcpServer/elicitation/request" => (
                    ServerRequestKind::McpElicitation,
                    HarnessEvent::ApprovalRequested {
                        request_id: request_id.clone(),
                        item_id: None,
                        action: HarnessAction::Tool {
                            name: "mcp-elicitation".to_owned(),
                            input: params.clone(),
                        },
                        reason: reason.clone(),
                        parent_item_id: request_parent,
                    },
                ),
                _ => unreachable!("supported method checked above"),
            };
            if state.server_seen.contains_key(&request_id)
                || state.server_standalone_seen.contains(&request_id)
            {
                return Err(format!(
                    "Codex app-server reused JSON-RPC request id in one process: {}",
                    rpc_key(&id)
                ));
            }
            state
                .server_seen
                .insert(request_id.clone(), context.clone());
            match state.server.entry(request_id.clone()) {
                Entry::Vacant(entry) => {
                    entry.insert(ServerRequest {
                        id: id.clone(),
                        kind,
                        context,
                    });
                    Ok(Registration::Event(event))
                }
                Entry::Occupied(_) => Err(format!(
                    "Codex app-server reused pending JSON-RPC request id: {}",
                    rpc_key(&id)
                )),
            }
        })
    };
    match registered {
        Ok(Registration::Event(event)) => emit_event(inner, event).await,
        Ok(Registration::CancelStandalone) => {
            let _ = inner
                .supervisor
                .send(&json!({"id":id,"result":{"action":"cancel"}}))
                .await;
        }
        Err(message) => protocol_violation(inner, message).await,
    }
}

async fn handle_notification(inner: &Arc<Inner>, method: &str, params: Value) {
    let turn_scoped = matches!(
        method,
        "item/agentMessage/delta"
            | "item/reasoning/summaryTextDelta"
            | "item/reasoning/textDelta"
            | "item/commandExecution/outputDelta"
            | "item/fileChange/outputDelta"
            | "item/mcpToolCall/outputDelta"
            | "item/dynamicToolCall/outputDelta"
            | "item/started"
            | "item/completed"
            | "turn/completed"
    );
    if turn_scoped || method == "turn/started" {
        // Codex also publishes subagent turn and item notifications on this connection.
        // They must not open, end, or add items to the conversation we own.
        let state = lock(&inner.state);
        if !state.thread_id.is_empty()
            && params
                .get("threadId")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty() && id != state.thread_id)
        {
            return;
        }
    }
    if turn_scoped {
        let checked = active_context(&lock(&inner.state), &params);
        if let Err(message) = checked {
            protocol_violation(inner, message).await;
            return;
        }
    }
    match method {
        "turn/started" => {
            let opened = {
                let mut state = lock(&inner.state);
                if state.closed {
                    return;
                }
                let thread_id = string(&params, "threadId")
                    .ok_or_else(|| "Codex turn/started has no threadId".to_owned());
                let native_turn = native_turn_of(&params)
                    .ok_or_else(|| "Codex turn/started has no turn.id".to_owned());
                match (thread_id, native_turn, state.hoh_turn.clone()) {
                    (Ok(thread_id), Ok(native_turn), Some(hoh_turn))
                        if thread_id == state.thread_id
                            && state
                                .native_turn
                                .as_ref()
                                .is_none_or(|current| current == &native_turn) =>
                    {
                        state.native_turn = Some(native_turn);
                        let emit = if state.turn_opened {
                            None
                        } else {
                            Some(hoh_turn)
                        };
                        state.turn_opened = true;
                        let interrupt = take_pending_interrupt(&mut state);
                        Ok((emit, interrupt))
                    }
                    (Ok(thread_id), _, _) if thread_id != state.thread_id => {
                        Err("Codex turn/started belongs to another thread".to_owned())
                    }
                    (Ok(_), Ok(_), None) => {
                        Err("Codex turn/started has no active HoH turn".to_owned())
                    }
                    (Ok(_), Ok(_), Some(_)) => {
                        Err("Codex turn/started changed the active native turn".to_owned())
                    }
                    (Err(message), _, _) | (_, Err(message), _) => Err(message),
                }
            };
            match opened {
                Ok((emit, interrupt)) => {
                    if let Some(turn_id) = emit {
                        emit_event(inner, HarnessEvent::TurnStarted { turn_id }).await;
                    }
                    if let Some(context) = interrupt {
                        let inner = inner.clone();
                        tokio::spawn(async move {
                            if let Err(failure) = send_interrupt(&inner, &context).await {
                                interrupt_protocol_violation_if_current(
                                    &inner,
                                    &context,
                                    format!(
                                        "Codex failed the latched turn interrupt: {}",
                                        failure.message
                                    ),
                                )
                                .await;
                            }
                        });
                    }
                }
                Err(message) => protocol_violation(inner, message).await,
            }
        }
        "item/agentMessage/delta" => {
            if let (Some(message_id), Some(text)) =
                (string(&params, "itemId"), string(&params, "delta"))
            {
                let parent_item_id = event_parent_item_id(&params)
                    .or_else(|| lock(&inner.state).item_parents.get(&message_id).cloned());
                emit_event(
                    inner,
                    HarnessEvent::TextDelta {
                        message_id,
                        text,
                        parent_item_id,
                    },
                )
                .await;
            }
        }
        "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
            if let (Some(message_id), Some(text)) =
                (string(&params, "itemId"), string(&params, "delta"))
            {
                let parent_item_id = event_parent_item_id(&params)
                    .or_else(|| lock(&inner.state).item_parents.get(&message_id).cloned());
                emit_event(
                    inner,
                    HarnessEvent::ReasoningDelta {
                        message_id,
                        text,
                        parent_item_id,
                    },
                )
                .await;
            }
        }
        "item/commandExecution/outputDelta"
        | "item/fileChange/outputDelta"
        | "item/mcpToolCall/outputDelta"
        | "item/dynamicToolCall/outputDelta" => {
            if let (Some(item_id), Some(text)) =
                (string(&params, "itemId"), string(&params, "delta"))
            {
                let parent_item_id = event_parent_item_id(&params)
                    .or_else(|| lock(&inner.state).item_parents.get(&item_id).cloned());
                emit_event(
                    inner,
                    HarnessEvent::ItemOutput {
                        item_id,
                        text,
                        parent_item_id,
                    },
                )
                .await;
            }
        }
        "item/started" => {
            if let Some(item) = params.get("item") {
                let parent_item_id = event_parent_item_id(&params);
                if let (Some(item_id), Some(parent_item_id)) =
                    (string(item, "id"), parent_item_id.as_ref())
                {
                    lock(&inner.state)
                        .item_parents
                        .insert(item_id, parent_item_id.clone());
                }
                let Some((item_id, action)) = action_of(item) else {
                    return;
                };
                let inserted = {
                    let mut state = lock(&inner.state);
                    if !state.items_seen.insert(item_id.clone()) {
                        false
                    } else {
                        state.items.insert(item_id.clone(), action.clone());
                        true
                    }
                };
                if !inserted {
                    protocol_violation(
                        inner,
                        format!("Codex reused itemId {item_id:?} in one turn"),
                    )
                    .await;
                    return;
                }
                emit_event(
                    inner,
                    HarnessEvent::ItemStarted {
                        item_id,
                        action,
                        parent_item_id,
                    },
                )
                .await;
            }
        }
        "item/completed" => {
            if let Some(item) = params.get("item") {
                complete_item(inner, item, event_parent_item_id(&params)).await;
            }
        }
        "turn/completed" => complete_turn(inner, &params).await,
        "serverRequest/resolved" => {
            let raw = params.get("requestId").cloned();
            let Some(raw) = raw else {
                protocol_violation(inner, "Codex serverRequest/resolved has no request id").await;
                return;
            };
            if !is_typed_request_id(&raw) {
                protocol_violation(
                    inner,
                    "Codex serverRequest/resolved has a non-string/non-int64 request id",
                )
                .await;
                return;
            }
            let request_id = format!("codex:{}", rpc_key(&raw));
            let resolved = {
                let mut state = lock(&inner.state);
                if state.closed {
                    return;
                }
                let thread_id = string(&params, "threadId")
                    .ok_or_else(|| "Codex serverRequest/resolved has no threadId".to_owned());
                match thread_id {
                    Err(message) => Err(message),
                    Ok(thread_id) if thread_id != state.thread_id => {
                        Err("Codex serverRequest/resolved belongs to another thread".to_owned())
                    }
                    Ok(_) if state.server_standalone_seen.contains(&request_id) => Ok(false),
                    Ok(_) => {
                        current_thread_context(&state, &params).and_then(|context| {
                            match state.server.get(&request_id) {
                                Some(pending)
                                    if pending.context == context
                                        && context_is_current(&state, &pending.context) =>
                                {
                                    state.server.remove(&request_id);
                                    Ok(true)
                                }
                                Some(_) => {
                                    Err("Codex resolved a request belonging to another turn"
                                        .to_owned())
                                }
                                None => match state.server_seen.get(&request_id) {
                                    Some(seen)
                                        if seen == &context && context_is_current(&state, seen) =>
                                    {
                                        Ok(false)
                                    }
                                    Some(_) => {
                                        Err("Codex resolved a request belonging to a prior turn"
                                            .to_owned())
                                    }
                                    None => Err("Codex resolved an unknown request id".to_owned()),
                                },
                            }
                        })
                    }
                }
            };
            match resolved {
                Ok(true) => {
                    emit_event(inner, HarnessEvent::RequestResolved { request_id }).await;
                }
                Ok(false) => {}
                Err(message) => protocol_violation(inner, message).await,
            }
        }
        _ => {}
    }
}

async fn complete_turn(inner: &Arc<Inner>, params: &Value) {
    let completed = {
        let mut state = lock(&inner.state);
        if !state.server.is_empty() {
            Err("Codex completed a turn while a human request was still pending".to_owned())
        } else {
            let status = params
                .pointer("/turn/status")
                .or_else(|| params.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("completed");
            let interrupted = status == "interrupted" || status == "cancelled";
            let turn_id = state.hoh_turn.take();
            state.native_turn = None;
            state.turn_opened = false;
            state.interrupt_requested = false;
            state.interrupt_sent = false;
            state.items.clear();
            state.item_parents.clear();
            Ok((turn_id, interrupted))
        }
    };
    let (turn_id, interrupted) = match completed {
        Ok(completed) => completed,
        Err(message) => {
            protocol_violation(inner, message).await;
            return;
        }
    };
    let Some(turn_id) = turn_id else { return };
    let status = params
        .pointer("/turn/status")
        .or_else(|| params.get("status"))
        .and_then(Value::as_str)
        .unwrap_or("completed");
    let event = if interrupted {
        HarnessEvent::TurnAborted { turn_id }
    } else if status == "failed" {
        HarnessEvent::TurnFailed {
            turn_id,
            error: HarnessFailure::new(
                code::HARNESS,
                params
                    .pointer("/turn/error/message")
                    .or_else(|| params.pointer("/error/message"))
                    .and_then(Value::as_str)
                    .unwrap_or("Codex turn failed"),
            ),
        }
    } else {
        HarnessEvent::TurnCompleted { turn_id }
    };
    emit_event(inner, event).await;
}

async fn complete_item(inner: &Arc<Inner>, item: &Value, carried_parent: Option<String>) {
    let Some(item_id) = string(item, "id") else {
        return;
    };
    let parent_item_id = carried_parent
        .or_else(|| parent_item_id(item))
        .or_else(|| lock(&inner.state).item_parents.get(&item_id).cloned());
    match item.get("type").and_then(Value::as_str).unwrap_or("") {
        "agentMessage" => {
            emit_event(
                inner,
                HarnessEvent::TextFinal {
                    message_id: item_id.clone(),
                    text: string(item, "text").unwrap_or_default(),
                    parent_item_id: parent_item_id.clone(),
                },
            )
            .await;
            lock(&inner.state).item_parents.remove(&item_id);
        }
        "reasoning" => {
            let text = text_parts(item.get("summary"))
                .into_iter()
                .chain(text_parts(item.get("content")))
                .collect::<Vec<_>>()
                .join("\n");
            emit_event(
                inner,
                HarnessEvent::ReasoningFinal {
                    message_id: item_id.clone(),
                    text,
                    parent_item_id: parent_item_id.clone(),
                },
            )
            .await;
            lock(&inner.state).item_parents.remove(&item_id);
        }
        kind if is_step(kind) => {
            let started = {
                let mut state = lock(&inner.state);
                if state.items.contains_key(&item_id) {
                    Ok(None)
                } else if !state.items_seen.insert(item_id.clone()) {
                    Err(format!(
                        "Codex reused completed itemId {item_id:?} in one turn"
                    ))
                } else if let Some((_, action)) = action_of(item) {
                    if let Some(parent_item_id) = parent_item_id.as_ref() {
                        state
                            .item_parents
                            .insert(item_id.clone(), parent_item_id.clone());
                    }
                    state.items.insert(item_id.clone(), action.clone());
                    Ok(Some(action))
                } else {
                    Ok(None)
                }
            };
            let started = match started {
                Ok(started) => started,
                Err(message) => {
                    protocol_violation(inner, message).await;
                    return;
                }
            };
            if let Some(action) = started {
                emit_event(
                    inner,
                    HarnessEvent::ItemStarted {
                        item_id: item_id.clone(),
                        action,
                        parent_item_id: parent_item_id.clone(),
                    },
                )
                .await;
            }
            let status = item
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("completed");
            let outcome = match status {
                "failed" => ItemOutcome::Failed,
                "declined" => ItemOutcome::Declined,
                "interrupted" | "cancelled" => ItemOutcome::Interrupted,
                _ => ItemOutcome::Completed,
            };
            emit_event(
                inner,
                HarnessEvent::ItemCompleted {
                    item_id: item_id.clone(),
                    outcome,
                    output: string(item, "aggregatedOutput").or_else(|| string(item, "output")),
                    exit_code: item.get("exitCode").and_then(Value::as_i64),
                    parent_item_id,
                },
            )
            .await;
            let mut state = lock(&inner.state);
            state.items.remove(&item_id);
            state.item_parents.remove(&item_id);
        }
        _ => {}
    }
}

fn is_step(kind: &str) -> bool {
    matches!(
        kind,
        "commandExecution"
            | "fileChange"
            | "mcpToolCall"
            | "dynamicToolCall"
            | "webSearch"
            | "imageView"
            | "plan"
    )
}

fn action_of(item: &Value) -> Option<(String, HarnessAction)> {
    let id = string(item, "id")?;
    let action = match item.get("type").and_then(Value::as_str)? {
        "commandExecution" => HarnessAction::Command {
            command: string(item, "command").unwrap_or_else(|| "command".to_owned()),
            cwd: string(item, "cwd"),
        },
        "fileChange" => HarnessAction::FileChange {
            changes: file_changes(item),
        },
        kind if is_step(kind) => HarnessAction::Tool {
            name: kind.to_owned(),
            input: item.clone(),
        },
        _ => return None,
    };
    Some((id, action))
}

fn file_changes(value: &Value) -> Vec<FileChange> {
    value
        .get("changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|change| {
            let path = string(change, "path")?;
            let (kind, move_path) = match change.get("kind") {
                Some(Value::String(kind)) if kind == "add" || kind == "create" => {
                    (FileChangeKind::Add, None)
                }
                Some(Value::String(kind)) if kind == "delete" => (FileChangeKind::Delete, None),
                Some(Value::Object(kind)) if kind.contains_key("move") => (
                    FileChangeKind::Update,
                    kind.get("move")
                        .and_then(|value| value.get("path").or(Some(value)))
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                ),
                _ => (FileChangeKind::Update, None),
            };
            Some(FileChange {
                path,
                kind,
                move_path,
            })
        })
        .collect()
}

fn questions(params: &Value) -> Vec<HarnessQuestion> {
    params
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|question| {
            let id = string(question, "id")?;
            Some(HarnessQuestion {
                id,
                question: string(question, "question").unwrap_or_default(),
                header: string(question, "header"),
                options: question
                    .get("options")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|option| {
                        Some(QuestionOption {
                            label: string(option, "label")?,
                            description: string(option, "description"),
                        })
                    })
                    .collect(),
                multi_select: question
                    .get("multiSelect")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect()
}

fn text_parts(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| match part {
            Value::String(text) => Some(text.clone()),
            Value::Object(object) => object
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_owned),
            _ => None,
        })
        .collect()
}
