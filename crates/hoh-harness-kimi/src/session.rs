use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use hoh_external_harness::{
    EndReason, Frame, HarnessAction, HarnessEvent, HarnessFailure, HarnessResult, HarnessSession,
    ItemOutcome, ProcessSpec, PromptParams, RespondParams, Supervisor, SupervisorLimits, code,
};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

const TIMEOUT: Duration = Duration::from_secs(20);
const QUEUE: usize = 64;

pub(crate) struct Peer {
    supervisor: Arc<Supervisor>,
    frames: mpsc::Receiver<Frame>,
    next_id: u64,
}

impl Peer {
    pub(crate) fn spawn(spec: &ProcessSpec) -> HarnessResult<Self> {
        let supervisor = Arc::new(Supervisor::spawn(spec, SupervisorLimits::default(), None)?);
        let frames = supervisor
            .take_frames()
            .expect("new process owns its stdout");
        Ok(Self {
            supervisor,
            frames,
            next_id: 0,
        })
    }

    pub(crate) async fn initialize(&mut self) -> HarnessResult<Value> {
        self.call(
            "initialize",
            json!({
                "protocolVersion": 1, "clientCapabilities": {},
                "clientInfo": {"name":"HoH", "version":env!("CARGO_PKG_VERSION")}
            }),
        )
        .await
    }

    pub(crate) async fn resume(&mut self, id: &str, cwd: &Path) -> HarnessResult<Value> {
        self.call(
            "session/resume",
            json!({"sessionId":id,"cwd":cwd,"mcpServers":[]}),
        )
        .await
    }

    pub(crate) async fn call(&mut self, method: &str, params: Value) -> HarnessResult<Value> {
        tokio::time::timeout(TIMEOUT, async {
            let id = self.request(method, params).await?;
            loop {
                let frame = parse_frame(self.frames.recv().await)?;
                if frame["method"].is_string() {
                    self.reply_to_agent(&frame).await?;
                } else {
                    return response(frame, id, method);
                }
            }
        })
        .await
        .map_err(|_| {
            HarnessFailure::new(code::TIMEOUT, format!("Kimi Code did not answer {method}."))
        })?
    }

    pub(crate) async fn close(self) {
        // Allow Kimi's CLI to close its global stores after a health check as well.
        graceful_shutdown(&self.supervisor, Duration::from_secs(2)).await;
    }

    async fn request(&mut self, method: &str, params: Value) -> HarnessResult<u64> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await?;
        Ok(id)
    }

    async fn send(&self, value: Value) -> HarnessResult<()> {
        tokio::time::timeout(TIMEOUT, self.supervisor.send(&value))
            .await
            .map_err(|_| timed_out())?
    }

    async fn reply_to_agent(&self, frame: &Value) -> HarnessResult<()> {
        let Some(id) = frame.get("id") else {
            return Ok(());
        };
        if !(id.is_string() || id.as_i64().is_some()) {
            return Err(protocol("Kimi Code sent a malformed request ID."));
        }
        // HoH advertises no client tools. Full access sets Kimi's unattended mode. A native
        // permission or elicitation still arriving in that mode must never be auto-approved here.
        let mut reply = json!({"jsonrpc":"2.0","id":id});
        if frame["method"] == "session/request_permission" {
            reply["result"] = json!({"outcome":{"outcome":"cancelled"}});
        } else {
            reply["error"] = json!({"code":-32601,"message":"Method not supported by HoH"});
        }
        self.send(reply).await
    }

    pub(crate) fn into_session(
        self,
        native_id: String,
        images: bool,
    ) -> (Session, mpsc::Receiver<HarnessEvent>) {
        let supervisor = self.supervisor.clone();
        let (commands, control) = mpsc::channel(QUEUE);
        let (events, stream) = mpsc::channel(QUEUE);
        let worker = tokio::spawn(self.run(native_id, control, events));
        (
            Session {
                supervisor,
                commands,
                images,
                worker,
            },
            stream,
        )
    }

    async fn run(
        mut self,
        native_id: String,
        mut commands: mpsc::Receiver<Command>,
        events: mpsc::Sender<HarnessEvent>,
    ) {
        let mut turn = None;
        let failure = loop {
            let result = tokio::select! {
                command = commands.recv() => {
                    let Some(command) = command else { return };
                    self.command(command, &native_id, &mut turn, &events).await
                }
                frame = self.frames.recv() => {
                    match parse_frame(frame) {
                        Ok(frame) => self.frame(frame, &native_id, &mut turn, &events).await,
                        Err(error) => Err(error),
                    }
                }
            };
            if let Err(error) = result {
                break error;
            }
        };
        self.supervisor.abandon();
        let _ = events
            .send(HarnessEvent::SessionEnded {
                reason: if failure.code == code::PROTOCOL {
                    EndReason::ProtocolError
                } else {
                    EndReason::StreamClosed
                },
                message: Some(failure.message),
                exit_code: None,
            })
            .await;
    }

    async fn command(
        &mut self,
        command: Command,
        native_id: &str,
        turn: &mut Option<Turn>,
        events: &mpsc::Sender<HarnessEvent>,
    ) -> HarnessResult<()> {
        match command {
            Command::Prompt { params, reply } => {
                if turn.is_some() {
                    let _ = reply.send(Err(protocol("Kimi Code already has an active turn.")));
                    return Ok(());
                }
                if reply.is_closed() {
                    return Ok(());
                }
                let mut prompt = vec![json!({"type":"text","text":params.text})];
                prompt.extend(params.images.into_iter().map(
                    |image| json!({"type":"image","data":image.data,"mimeType":image.media_type}),
                ));
                let id = match self
                    .request(
                        "session/prompt",
                        json!({"sessionId":native_id,"prompt":prompt}),
                    )
                    .await
                {
                    Ok(id) => id,
                    Err(error) => {
                        let _ = reply.send(Err(error.clone()));
                        return Err(error);
                    }
                };
                let turn_id = params.turn_id;
                *turn = Some(Turn {
                    id,
                    turn_id: turn_id.clone(),
                    cancelled: false,
                    items: HashMap::new(),
                });
                // ACP's response is the end of the turn. Acknowledge the write now so HoH can Stop.
                let _ = reply.send(Ok(()));
                emit(events, HarnessEvent::TurnStarted { turn_id }).await
            }
            Command::Interrupt(reply) => {
                let result = if let Some(turn) = turn.as_mut().filter(|turn| !turn.cancelled) {
                    turn.cancelled = true;
                    self.send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":native_id}})).await
                } else {
                    Ok(())
                };
                let _ = reply.send(result.clone());
                result
            }
        }
    }

    async fn frame(
        &self,
        frame: Value,
        native_id: &str,
        turn: &mut Option<Turn>,
        events: &mpsc::Sender<HarnessEvent>,
    ) -> HarnessResult<()> {
        if frame["method"].is_string() {
            if frame["method"] == "session/update" && frame.get("id").is_none() {
                let params = &frame["params"];
                if params["sessionId"] == native_id
                    && let Some(turn) = turn.as_mut().filter(|turn| !turn.cancelled)
                {
                    turn.update(&params["update"], events).await?;
                }
                return Ok(());
            }
            return self.reply_to_agent(&frame).await;
        }

        let turn = turn
            .take()
            .ok_or_else(|| protocol("Kimi Code sent a response without an active turn."))?;
        let result = response(frame, turn.id, "session/prompt");
        if result
            .as_ref()
            .is_err_and(|error| error.code == code::PROTOCOL)
        {
            return result.map(|_| ());
        }
        let turn_id = turn.turn_id;
        let event = match result
            .as_ref()
            .ok()
            .and_then(|result| result["stopReason"].as_str())
        {
            _ if turn.cancelled => HarnessEvent::TurnAborted { turn_id },
            Some("cancelled") => HarnessEvent::TurnAborted { turn_id },
            Some("end_turn") => HarnessEvent::TurnCompleted { turn_id },
            Some("max_tokens" | "max_turn_requests" | "refusal") => HarnessEvent::TurnFailed {
                turn_id,
                error: HarnessFailure::new(
                    code::HARNESS,
                    "Kimi Code stopped before completing the turn.",
                ),
            },
            _ => HarnessEvent::TurnFailed {
                turn_id,
                error: result.err().unwrap_or_else(|| {
                    protocol("Kimi Code did not report a valid turn stop reason.")
                }),
            },
        };
        emit(events, event).await
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.supervisor.abandon();
    }
}

fn parse_frame(frame: Option<Frame>) -> HarnessResult<Value> {
    match frame {
        Some(Frame::Message(frame)) if frame["jsonrpc"] == "2.0" => Ok(frame),
        Some(Frame::Message(_) | Frame::Violation(_)) => {
            Err(protocol("Kimi Code sent an invalid ACP frame."))
        }
        Some(Frame::Closed { .. }) | None => Err(closed()),
    }
}

fn response(frame: Value, id: u64, method: &str) -> HarnessResult<Value> {
    if frame["id"].as_u64() != Some(id) {
        return Err(protocol("Kimi Code sent an unexpected response ID."));
    }
    if let Some(result) = frame.get("result") {
        return Ok(result.clone());
    }
    let error = &frame["error"];
    match error["code"].as_i64() {
        Some(-32000) => Err(HarnessFailure::new(crate::AUTH_REQUIRED, crate::LOGIN)),
        Some(-32602)
            if method == "session/resume"
                && error["message"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("Unknown sessionId:")) =>
        {
            Err(HarnessFailure::new(
                code::SESSION_NOT_FOUND,
                "Kimi Code no longer has this native session.",
            ))
        }
        Some(-32601) if method == "initialize" => Err(HarnessFailure::new(
            code::UNSUPPORTED_VERSION,
            "Kimi Code CLI does not support ACP v1.",
        )),
        Some(_) => Err(HarnessFailure::new(
            code::HARNESS,
            format!("Kimi Code rejected {method}."),
        )),
        None => Err(protocol("Kimi Code did not return an ACP result or error.")),
    }
}

fn protocol(message: &str) -> HarnessFailure {
    HarnessFailure::new(code::PROTOCOL, message)
}

fn closed() -> HarnessFailure {
    HarnessFailure::new(code::CLOSED, "Kimi Code closed its ACP stream.")
}

fn timed_out() -> HarnessFailure {
    HarnessFailure::new(code::TIMEOUT, "Kimi Code did not answer an ACP command.")
}

async fn emit(events: &mpsc::Sender<HarnessEvent>, event: HarnessEvent) -> HarnessResult<()> {
    events.send(event).await.map_err(|_| closed())
}

type Reply = oneshot::Sender<HarnessResult<()>>;

enum Command {
    Prompt { params: PromptParams, reply: Reply },
    Interrupt(Reply),
}

pub(crate) struct Session {
    supervisor: Arc<Supervisor>,
    commands: mpsc::Sender<Command>,
    images: bool,
    worker: JoinHandle<()>,
}

impl Session {
    fn command(
        &self,
        build: impl FnOnce(Reply) -> Command + Send + 'static,
    ) -> BoxFuture<'static, HarnessResult<()>> {
        let commands = self.commands.clone();
        let supervisor = self.supervisor.clone();
        Box::pin(async move {
            let (reply, response) = oneshot::channel();
            let result = tokio::time::timeout(TIMEOUT, async {
                commands.send(build(reply)).await.map_err(|_| closed())?;
                response.await.map_err(|_| closed())?
            })
            .await;
            result.unwrap_or_else(|_| {
                supervisor.abandon();
                Err(timed_out())
            })
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.worker.abort();
        self.supervisor.abandon();
    }
}

impl HarnessSession for Session {
    fn prompt(&self, params: PromptParams) -> BoxFuture<'static, HarnessResult<()>> {
        if !self.images && !params.images.is_empty() {
            return Box::pin(std::future::ready(Err(HarnessFailure::new(
                code::HARNESS,
                "This Kimi Code CLI does not accept images over ACP.",
            ))));
        }
        self.command(move |reply| Command::Prompt { params, reply })
    }

    fn interrupt(&self) -> BoxFuture<'static, HarnessResult<()>> {
        self.command(Command::Interrupt)
    }

    fn respond(&self, _params: RespondParams) -> BoxFuture<'static, HarnessResult<()>> {
        Box::pin(std::future::ready(Err(protocol(
            "Kimi Code has no pending interactive request.",
        ))))
    }

    fn terminate(&self, grace: Duration) -> BoxFuture<'static, ()> {
        let supervisor = self.supervisor.clone();
        let worker = self.worker.abort_handle();
        Box::pin(async move {
            graceful_shutdown(&supervisor, grace).await;
            worker.abort();
        })
    }

    fn diagnostics(&self) -> String {
        self.supervisor.stderr_tail()
    }
}

async fn graceful_shutdown(supervisor: &Supervisor, grace: Duration) {
    // Kimi flushes its session store when its ACP input reaches EOF.
    let graceful = grace / 2;
    if tokio::time::timeout(graceful, async {
        let _ = supervisor.send_input_and_close(b"", 0).await;
        supervisor.wait().await
    })
    .await
    .is_err()
    {
        supervisor.terminate(grace - graceful).await;
    }
}

struct Turn {
    id: u64,
    turn_id: String,
    cancelled: bool,
    items: HashMap<String, bool>,
}

impl Turn {
    async fn update(
        &mut self,
        update: &Value,
        events: &mpsc::Sender<HarnessEvent>,
    ) -> HarnessResult<()> {
        let event = match update["sessionUpdate"].as_str() {
            Some("agent_message_chunk" | "agent_thought_chunk") => {
                let content = &update["content"];
                let Some(text) = content["text"]
                    .as_str()
                    .filter(|_| content["type"] == "text")
                else {
                    return Ok(());
                };
                if update["sessionUpdate"] == "agent_message_chunk" {
                    HarnessEvent::TextDelta {
                        message_id: format!("{}:text", self.turn_id),
                        text: text.into(),
                        parent_item_id: None,
                    }
                } else {
                    HarnessEvent::ReasoningDelta {
                        message_id: format!("{}:thinking", self.turn_id),
                        text: text.into(),
                        parent_item_id: None,
                    }
                }
            }
            Some("tool_call" | "tool_call_update") => {
                let Some(id) = update["toolCallId"].as_str().filter(|id| !id.is_empty()) else {
                    return Ok(());
                };
                match self.items.get(id) {
                    Some(true) => return Ok(()),
                    Some(false) => {}
                    None => {
                        self.items.insert(id.into(), false);
                        emit(
                            events,
                            HarnessEvent::ItemStarted {
                                item_id: id.into(),
                                action: action(update),
                                parent_item_id: None,
                            },
                        )
                        .await?;
                    }
                }
                let outcome = match update["status"].as_str() {
                    Some("completed") => ItemOutcome::Completed,
                    Some("failed") => ItemOutcome::Failed,
                    _ => return Ok(()),
                };
                self.items.insert(id.into(), true);
                let output = update["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|block| block.pointer("/content/text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n");
                HarnessEvent::ItemCompleted {
                    item_id: id.into(),
                    outcome,
                    output: (!output.is_empty()).then_some(output),
                    exit_code: None,
                    parent_item_id: None,
                }
            }
            _ => return Ok(()),
        };
        emit(events, event).await
    }
}

fn action(update: &Value) -> HarnessAction {
    if update["kind"] == "execute"
        && let Some(command) = update.pointer("/rawInput/command").and_then(Value::as_str)
    {
        return HarnessAction::Command {
            command: command.into(),
            cwd: update
                .pointer("/rawInput/cwd")
                .and_then(Value::as_str)
                .map(str::to_owned),
        };
    }
    HarnessAction::Tool {
        name: update["title"].as_str().unwrap_or("Kimi tool").into(),
        input: update["rawInput"].clone(),
    }
}
