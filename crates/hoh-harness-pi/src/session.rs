use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use hoh_external_harness::{
    EndReason, FileChange, FileChangeKind, Frame, HarnessAction, HarnessEvent, HarnessFailure,
    HarnessResult, HarnessSession, ItemOutcome, ProcessSpec, PromptParams, RespondParams,
    Supervisor, SupervisorLimits, TitleRequest, code, redact::redact,
};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

const QUEUE: usize = 64;
// Pi repeats attachments in stdout events. Leave space for text and metadata in the frame.
const MAX_ENCODED_IMAGES: usize = 6 * 1024 * 1024;

struct Call {
    request: Value,
    reply: oneshot::Sender<HarnessResult<Value>>,
    turn_id: Option<String>,
}

pub(crate) struct PiSession {
    supervisor: Arc<Supervisor>,
    calls: mpsc::Sender<Call>,
    worker: JoinHandle<()>,
    pub(crate) title_route: Option<crate::PiTitleRoute>,
}

impl PiSession {
    pub(crate) fn spawn(spec: &ProcessSpec) -> HarnessResult<(Self, mpsc::Receiver<HarnessEvent>)> {
        // Pi's agent_end record combines prompt attachments and tool results in one frame.
        let limits = SupervisorLimits {
            max_frame_bytes: 16 * 1024 * 1024,
            frame_queue: 16,
            ..SupervisorLimits::default()
        };
        let supervisor = Arc::new(Supervisor::spawn(spec, limits, None)?);
        let frames = supervisor
            .take_frames()
            .expect("new process owns its stdout");
        let (calls, receiver) = mpsc::channel(QUEUE);
        let (events, stream) = mpsc::channel(QUEUE);
        let worker = tokio::spawn(run(supervisor.clone(), frames, receiver, events));
        Ok((
            Self {
                supervisor,
                calls,
                worker,
                title_route: None,
            },
            stream,
        ))
    }

    pub(crate) fn call(&self, request: Value) -> BoxFuture<'static, HarnessResult<Value>> {
        self.request(request, None)
    }

    fn request(
        &self,
        request: Value,
        turn_id: Option<String>,
    ) -> BoxFuture<'static, HarnessResult<Value>> {
        let calls = self.calls.clone();
        let supervisor = self.supervisor.clone();
        Box::pin(async move {
            let (reply, response) = oneshot::channel();
            let result = tokio::time::timeout(crate::TIMEOUT, async {
                calls
                    .send(Call {
                        request,
                        reply,
                        turn_id,
                    })
                    .await
                    .map_err(|_| closed())?;
                response.await.map_err(|_| closed())?
            })
            .await;
            match result {
                Ok(result) => result,
                Err(_) => {
                    supervisor.abandon();
                    Err(HarnessFailure::new(
                        code::TIMEOUT,
                        "Pi did not answer an RPC command.",
                    ))
                }
            }
        })
    }
}

impl Drop for PiSession {
    fn drop(&mut self) {
        self.worker.abort();
        self.supervisor.abandon();
    }
}

impl HarnessSession for PiSession {
    fn generate_title(&self, request: TitleRequest) -> BoxFuture<'static, HarnessResult<String>> {
        let route = self.title_route.clone();
        let state = self.call(json!({"type":"get_state"}));
        Box::pin(async move {
            route
                .ok_or_else(crate::title_route_unavailable)?
                .title(request, state)
                .await
        })
    }

    fn prompt(&self, params: PromptParams) -> BoxFuture<'static, HarnessResult<()>> {
        if params
            .images
            .iter()
            .map(|image| image.data.len())
            .sum::<usize>()
            > MAX_ENCODED_IMAGES
        {
            return Box::pin(std::future::ready(Err(HarnessFailure::new(
                code::HARNESS,
                "Pi Agent supports up to about 4.5 MiB of images per message.",
            ))));
        }
        let images: Vec<_> = params
            .images
            .into_iter()
            .map(|image| {
                json!({
                    "type": "image",
                    "mimeType": image.media_type,
                    "data": image.data,
                })
            })
            .collect();
        let call = self.request(
            json!({
                "type": "prompt",
                "message": params.text,
                "images": images,
            }),
            Some(params.turn_id),
        );
        Box::pin(async move { call.await.map(|_| ()) })
    }

    fn interrupt(&self) -> BoxFuture<'static, HarnessResult<()>> {
        let call = self.call(json!({"type":"abort"}));
        Box::pin(async move { call.await.map(|_| ()) })
    }

    fn respond(&self, _params: RespondParams) -> BoxFuture<'static, HarnessResult<()>> {
        Box::pin(std::future::ready(Err(HarnessFailure::new(
            code::PROTOCOL,
            "Pi has no pending user request.",
        ))))
    }

    fn terminate(&self, grace: Duration) -> BoxFuture<'static, ()> {
        let supervisor = self.supervisor.clone();
        let worker = self.worker.abort_handle();
        Box::pin(async move {
            supervisor.terminate(grace).await;
            worker.abort();
        })
    }

    fn diagnostics(&self) -> String {
        self.supervisor.stderr_tail()
    }
}

fn closed() -> HarnessFailure {
    HarnessFailure::new(code::CLOSED, "The Pi process closed its RPC stream.")
}

async fn run(
    supervisor: Arc<Supervisor>,
    mut frames: mpsc::Receiver<Frame>,
    mut calls: mpsc::Receiver<Call>,
    events: mpsc::Sender<HarnessEvent>,
) {
    let mut next_id = 0_u64;
    let mut pending = HashMap::<String, Call>::new();
    let mut turn: Option<Turn> = None;
    let (reason, failure) = loop {
        tokio::select! {
            call = calls.recv() => {
                let Some(mut call) = call else { return };
                if let Some(id) = &call.turn_id {
                    if turn.is_some() {
                        let _ = call.reply.send(Err(HarnessFailure::new(code::HARNESS, "Pi already has an active turn.")));
                        continue;
                    }
                    turn = Some(Turn { id: id.clone(), message: 0, error: None, tool_output: HashMap::new() });
                    let _ = events.send(HarnessEvent::TurnStarted { turn_id: id.clone() }).await;
                }
                next_id += 1;
                let id = next_id.to_string();
                call.request["id"] = json!(id);
                if let Err(error) = supervisor.send(&call.request).await {
                    let _ = call.reply.send(Err(error.clone()));
                    break (EndReason::StreamClosed, error);
                }
                pending.insert(id, call);
            }
            frame = frames.recv() => {
                let frame = match frame {
                    Some(Frame::Message(frame)) => frame,
                    Some(Frame::Violation(message)) => break (EndReason::ProtocolError, HarnessFailure::new(code::PROTOCOL, message)),
                    Some(Frame::Closed { .. }) | None => break (EndReason::StreamClosed, closed()),
                };
                if frame["type"] == "response" {
                    let call = frame["id"].as_str().and_then(|id| pending.remove(id));
                    let Some(call) = call else {
                        break (EndReason::ProtocolError, HarnessFailure::new(code::PROTOCOL, "Unexpected Pi RPC response ID."));
                    };
                    if frame["command"] != call.request["type"] || !frame["success"].is_boolean() {
                        let error = HarnessFailure::new(code::PROTOCOL, "Invalid Pi RPC response.");
                        let _ = call.reply.send(Err(error.clone()));
                        break (EndReason::ProtocolError, error);
                    }
                    if frame["success"] == true {
                        let data = frame["data"].clone();
                        if data["disposition"] == "handled"
                            && let Some(turn_id) = call.turn_id
                        {
                            turn = None;
                            let _ = events.send(HarnessEvent::TurnCompleted { turn_id }).await;
                        }
                        let _ = call.reply.send(Ok(data));
                    } else {
                        if call.turn_id.is_some() {
                            turn = None;
                        }
                        let message = frame["error"].as_str().unwrap_or("Pi rejected an RPC command.");
                        let _ = call.reply.send(Err(HarnessFailure::new(code::HARNESS, redact(message))));
                    }
                } else if frame["type"] == "agent_settled" {
                    if let Some(turn) = turn.take() {
                        let event = if frame["aborted"] == true {
                            HarnessEvent::TurnAborted { turn_id: turn.id }
                        } else if let Some(error) = turn.error {
                            HarnessEvent::TurnFailed { turn_id: turn.id, error }
                        } else {
                            HarnessEvent::TurnCompleted { turn_id: turn.id }
                        };
                        let _ = events.send(event).await;
                    }
                } else if let Some(turn) = &mut turn {
                    turn.accept(&frame, &events).await;
                }
            }
        }
    };
    supervisor.abandon();
    for (_, call) in pending {
        let _ = call.reply.send(Err(failure.clone()));
    }
    let _ = events
        .send(HarnessEvent::SessionEnded {
            reason,
            message: Some(failure.message),
            exit_code: supervisor
                .exit_status()
                .and_then(|exit| exit.code)
                .map(i64::from),
        })
        .await;
}

struct Turn {
    id: String,
    message: u64,
    error: Option<HarnessFailure>,
    tool_output: HashMap<String, String>,
}

impl Turn {
    async fn accept(&mut self, frame: &Value, events: &mpsc::Sender<HarnessEvent>) {
        let message_id = format!("{}:{}", self.id, self.message);
        let message = &frame["message"];
        let event = match frame["type"].as_str() {
            Some("message_update") => {
                let update = &frame["assistantMessageEvent"];
                let Some(text) = update["delta"].as_str().map(str::to_owned) else {
                    return;
                };
                match update["type"].as_str() {
                    Some("text_delta") => HarnessEvent::TextDelta {
                        message_id,
                        text,
                        parent_item_id: None,
                    },
                    Some("thinking_delta") => HarnessEvent::ReasoningDelta {
                        message_id,
                        text,
                        parent_item_id: None,
                    },
                    _ => return,
                }
            }
            Some("message_end") if message["role"] == "assistant" => {
                self.message += 1;
                self.error = (message["stopReason"] == "error").then(|| {
                    HarnessFailure::new(
                        code::HARNESS,
                        redact(
                            message["errorMessage"]
                                .as_str()
                                .unwrap_or("Pi's model request failed."),
                        ),
                    )
                });
                let text = content(message, "text");
                if let Some(text) = content(message, "thinking") {
                    let _ = events
                        .send(HarnessEvent::ReasoningFinal {
                            message_id: message_id.clone(),
                            text,
                            parent_item_id: None,
                        })
                        .await;
                }
                let Some(text) = text else { return };
                HarnessEvent::TextFinal {
                    message_id,
                    text,
                    parent_item_id: None,
                }
            }
            Some("tool_execution_start" | "tool_execution_update" | "tool_execution_end") => {
                let Some(event) = tool_event(frame, &mut self.tool_output) else {
                    return;
                };
                event
            }
            _ => return,
        };
        let _ = events.send(event).await;
    }
}

fn content(value: &Value, kind: &str) -> Option<String> {
    let text: String = value["content"]
        .as_array()?
        .iter()
        .filter(|block| block["type"] == kind)
        .filter_map(|block| block[kind].as_str())
        .collect();
    (!text.is_empty()).then_some(text)
}

fn tool_event(frame: &Value, output: &mut HashMap<String, String>) -> Option<HarnessEvent> {
    let item_id = frame["toolCallId"].as_str()?.to_owned();
    let parent_item_id = frame["parentToolCallId"].as_str().map(str::to_owned);
    Some(match frame["type"].as_str()? {
        "tool_execution_start" => HarnessEvent::ItemStarted {
            item_id,
            action: tool_action(frame["toolName"].as_str()?, &frame["args"]),
            parent_item_id,
        },
        "tool_execution_update" => {
            let snapshot = content(&frame["partialResult"], "text")?;
            let previous = output.entry(item_id.clone()).or_default();
            let delta = snapshot.strip_prefix(previous.as_str())?.to_owned();
            if delta.is_empty() {
                return None;
            }
            *previous = snapshot;
            HarnessEvent::ItemOutput {
                item_id,
                text: delta,
                parent_item_id,
            }
        }
        "tool_execution_end" => {
            output.remove(&item_id);
            HarnessEvent::ItemCompleted {
                item_id,
                outcome: if frame["isError"] == true {
                    ItemOutcome::Failed
                } else {
                    ItemOutcome::Completed
                },
                output: content(&frame["result"], "text"),
                exit_code: None,
                parent_item_id,
            }
        }
        _ => return None,
    })
}

fn tool_action(name: &str, args: &Value) -> HarnessAction {
    match name {
        "bash" if args["command"].is_string() => HarnessAction::Command {
            command: args["command"].as_str().unwrap_or_default().to_owned(),
            cwd: None,
        },
        "edit" | "write" if args["path"].is_string() => HarnessAction::FileChange {
            changes: vec![FileChange {
                path: args["path"].as_str().unwrap_or_default().to_owned(),
                kind: FileChangeKind::Update,
                move_path: None,
            }],
        },
        _ => HarnessAction::Tool {
            name: name.to_owned(),
            input: args.clone(),
        },
    }
}
