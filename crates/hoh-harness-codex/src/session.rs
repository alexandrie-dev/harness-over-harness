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

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn rpc_key(id: &Value) -> String {
    serde_json::to_string(id).unwrap_or_else(|_| "null".to_owned())
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
            sandbox: "read-only",
            sandbox_policy: json!({"type":"readOnly"}),
        },
        AccessMode::AutoAcceptEdits => ModeSettings {
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
}

#[derive(Default)]
struct State {
    host: HashMap<u64, HostReply>,
    server: HashMap<String, ServerRequest>,
    /// Native ids accepted during this turn, including requests the human already answered. This
    /// closes the race where a repeated id arrives while the first answer is being sent.
    server_seen: HashSet<String>,
    items: HashMap<String, HarnessAction>,
    thread_id: String,
    hoh_turn: Option<String>,
    native_turn: Option<String>,
    turn_opened: bool,
    closed: bool,
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
            "config": {}
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
        if let Some(model) = &self.inner.model {
            params["model"] = json!(model);
        }
        if let Some(effort) = &self.inner.effort {
            params["effort"] = json!(effort);
        }
        params
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, HarnessFailure> {
        let id = self.inner.next_request.fetch_add(1, Ordering::Relaxed);
        let (reply, answer) = oneshot::channel();
        {
            let mut state = lock(&self.inner.state);
            if state.closed {
                return Err(HarnessFailure::new(
                    code::CLOSED,
                    "Codex app-server is closed",
                ));
            }
            state.host.insert(id, reply);
        }
        if let Err(error) = self
            .inner
            .supervisor
            .send(&json!({"id":id,"method":method,"params":params}))
            .await
        {
            lock(&self.inner.state).host.remove(&id);
            return Err(error);
        }
        match tokio::time::timeout(self.inner.request_timeout, answer).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(_)) => Err(HarnessFailure::new(
                code::CLOSED,
                format!("Codex app-server closed while waiting for {method}"),
            )),
            Err(_) => {
                lock(&self.inner.state).host.remove(&id);
                Err(HarnessFailure::new(
                    code::PROTOCOL,
                    format!("Codex app-server did not answer {method}"),
                ))
            }
        }
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
                state.items.clear();
                state.server_seen.clear();
            }
            let result = session
                .request("turn/start", session.turn_params(&params.text))
                .await;
            let result = match result {
                Ok(result) => result,
                Err(error) => {
                    let mut state = lock(&session.inner.state);
                    state.hoh_turn = None;
                    return Err(error);
                }
            };
            let native = result
                .pointer("/turn/id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    HarnessFailure::new(code::PROTOCOL, "turn/start returned no turn.id")
                })?
                .to_owned();
            let emit = {
                let mut state = lock(&session.inner.state);
                state.native_turn = Some(native);
                if state.turn_opened {
                    None
                } else {
                    state.turn_opened = true;
                    state.hoh_turn.clone()
                }
            };
            if let Some(turn_id) = emit {
                emit_event(&session.inner, HarnessEvent::TurnStarted { turn_id }).await;
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
            let (thread_id, turn_id) = {
                let state = lock(&session.inner.state);
                (state.thread_id.clone(), state.native_turn.clone())
            };
            let Some(turn_id) = turn_id else {
                return Ok(());
            };
            session
                .request(
                    "turn/interrupt",
                    json!({"threadId":thread_id,"turnId":turn_id}),
                )
                .await?;
            Ok(())
        })
    }

    fn respond(&self, params: RespondParams) -> BoxFuture<'static, Result<(), HarnessFailure>> {
        let session = Arc::new(Self {
            inner: self.inner.clone(),
            owner: self.owner.clone(),
        });
        Box::pin(async move {
            let pending = lock(&session.inner.state)
                .server
                .remove(&params.request_id)
                .ok_or_else(|| {
                    HarnessFailure::new(code::PROTOCOL, "Codex request is no longer pending")
                })?;
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
    let request_id = format!("codex:{}", rpc_key(&id));
    let item_id = string(&params, "itemId");
    let reason = string(&params, "reason");
    let (kind, event) = match method {
        "item/commandExecution/requestApproval" => {
            let action = item_id
                .as_ref()
                .and_then(|item| lock(&inner.state).items.get(item).cloned())
                .unwrap_or_else(|| HarnessAction::Command {
                    command: string(&params, "command").unwrap_or_else(|| "command".to_owned()),
                    cwd: string(&params, "cwd"),
                });
            (
                ServerRequestKind::CommandOrFile,
                HarnessEvent::ApprovalRequested {
                    request_id: request_id.clone(),
                    item_id,
                    action,
                    reason,
                },
            )
        }
        "item/fileChange/requestApproval" => {
            let action = item_id
                .as_ref()
                .and_then(|item| lock(&inner.state).items.get(item).cloned())
                .unwrap_or_else(|| HarnessAction::FileChange {
                    changes: file_changes(&params),
                });
            (
                ServerRequestKind::CommandOrFile,
                HarnessEvent::ApprovalRequested {
                    request_id: request_id.clone(),
                    item_id,
                    action,
                    reason,
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
                    item_id,
                    action: HarnessAction::Permission {
                        description: reason.clone().unwrap_or_else(|| requested.to_string()),
                    },
                    reason,
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
                reason,
            },
        ),
        _ => {
            let _ = inner
                .supervisor
                .send(&json!({"id":id,"error":{"code":-32601,"message":"unsupported client request"}}))
                .await;
            return;
        }
    };
    let duplicate = {
        let mut state = lock(&inner.state);
        if !state.server_seen.insert(request_id.clone()) {
            true
        } else {
            match state.server.entry(request_id.clone()) {
                Entry::Vacant(entry) => {
                    entry.insert(ServerRequest {
                        id: id.clone(),
                        kind,
                    });
                    false
                }
                Entry::Occupied(_) => true,
            }
        }
    };
    if duplicate {
        close(
            inner,
            EndReason::ProtocolError,
            Some(format!(
                "Codex app-server reused JSON-RPC request id in one turn: {}",
                rpc_key(&id)
            )),
        )
        .await;
        if !inner.terminated.swap(true, Ordering::SeqCst) {
            inner.supervisor.terminate(Duration::ZERO).await;
        }
        return;
    }
    emit_event(inner, event).await;
}

async fn handle_notification(inner: &Arc<Inner>, method: &str, params: Value) {
    match method {
        "turn/started" => {
            let emit = {
                let mut state = lock(&inner.state);
                if state.native_turn.is_none() {
                    state.native_turn = params
                        .pointer("/turn/id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                if state.turn_opened {
                    None
                } else {
                    state.turn_opened = true;
                    state.hoh_turn.clone()
                }
            };
            if let Some(turn_id) = emit {
                emit_event(inner, HarnessEvent::TurnStarted { turn_id }).await;
            }
        }
        "item/agentMessage/delta" => {
            if let (Some(message_id), Some(text)) =
                (string(&params, "itemId"), string(&params, "delta"))
            {
                emit_event(inner, HarnessEvent::TextDelta { message_id, text }).await;
            }
        }
        "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
            if let (Some(message_id), Some(text)) =
                (string(&params, "itemId"), string(&params, "delta"))
            {
                emit_event(inner, HarnessEvent::ReasoningDelta { message_id, text }).await;
            }
        }
        "item/commandExecution/outputDelta"
        | "item/fileChange/outputDelta"
        | "item/mcpToolCall/outputDelta"
        | "item/dynamicToolCall/outputDelta" => {
            if let (Some(item_id), Some(text)) =
                (string(&params, "itemId"), string(&params, "delta"))
            {
                emit_event(inner, HarnessEvent::ItemOutput { item_id, text }).await;
            }
        }
        "item/started" => {
            if let Some(item) = params.get("item")
                && let Some((item_id, action)) = action_of(item)
            {
                lock(&inner.state)
                    .items
                    .insert(item_id.clone(), action.clone());
                emit_event(inner, HarnessEvent::ItemStarted { item_id, action }).await;
            }
        }
        "item/completed" => {
            if let Some(item) = params.get("item") {
                complete_item(inner, item).await;
            }
        }
        "turn/completed" => complete_turn(inner, &params).await,
        "serverRequest/resolved" => {
            let raw = params
                .get("requestId")
                .or_else(|| params.get("id"))
                .cloned();
            if let Some(raw) = raw {
                let request_id = format!("codex:{}", rpc_key(&raw));
                if lock(&inner.state).server.remove(&request_id).is_some() {
                    emit_event(inner, HarnessEvent::RequestResolved { request_id }).await;
                }
            }
        }
        _ => {}
    }
}

async fn complete_turn(inner: &Arc<Inner>, params: &Value) {
    let (turn_id, interrupted) = {
        let mut state = lock(&inner.state);
        let status = params
            .pointer("/turn/status")
            .or_else(|| params.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("completed");
        let interrupted = status == "interrupted" || status == "cancelled";
        let turn_id = state.hoh_turn.take();
        state.native_turn = None;
        state.turn_opened = false;
        state.items.clear();
        (turn_id, interrupted)
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

async fn complete_item(inner: &Arc<Inner>, item: &Value) {
    let Some(item_id) = string(item, "id") else {
        return;
    };
    match item.get("type").and_then(Value::as_str).unwrap_or("") {
        "agentMessage" => {
            emit_event(
                inner,
                HarnessEvent::TextFinal {
                    message_id: item_id,
                    text: string(item, "text").unwrap_or_default(),
                },
            )
            .await;
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
                    message_id: item_id,
                    text,
                },
            )
            .await;
        }
        kind if is_step(kind) => {
            if !lock(&inner.state).items.contains_key(&item_id)
                && let Some((_, action)) = action_of(item)
            {
                lock(&inner.state)
                    .items
                    .insert(item_id.clone(), action.clone());
                emit_event(
                    inner,
                    HarnessEvent::ItemStarted {
                        item_id: item_id.clone(),
                        action,
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
                },
            )
            .await;
            lock(&inner.state).items.remove(&item_id);
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
