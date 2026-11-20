//! NDJSON clients; stdout is parsed strictly while the child is live.
use crate::{Result, corpus::Scenario, normalize::map_strings};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
};

pub struct Rpc {
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    next_id: u64,
    pending: HashMap<u64, Value>,
    pub frames: Vec<Value>,
    pub raw: String,
    pub notifications: Vec<Value>,
    pub live_sessions: Vec<String>,
    permissions: VecDeque<String>,
    sdk_approvals: VecDeque<String>,
    reviews: VecDeque<String>,
    pub initialized: bool,
    pub results: Vec<Value>,
    timeout: Duration,
    /// Bytes of the line being read; kept across cancelled reads.
    partial: Vec<u8>,
    /// A scripted answer the agent made impossible: the run is rejected after the step (`scriptError`).
    script_error: Option<String>,
}
impl Rpc {
    pub fn new(child: &mut Child, s: &Scenario, script: &Value, timeout: Duration) -> Result<Self> {
        Ok(Self {
            input: Some(child.stdin.take().ok_or("missing child stdin")?),
            output: BufReader::new(child.stdout.take().ok_or("missing child stdout")?),
            next_id: 0,
            pending: HashMap::new(),
            frames: Vec::new(),
            raw: String::new(),
            notifications: Vec::new(),
            live_sessions: if s.suite == "sdk" {
                vec!["fixture-root-session".into()]
            } else {
                Vec::new()
            },
            permissions: script["permissionAnswers"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v["kind"].as_str().map(str::to_owned))
                .collect(),
            sdk_approvals: if s.name == "approval" {
                VecDeque::from(["allowed-once".into(), "rejected".into()])
            } else {
                VecDeque::new()
            },
            reviews: if s.name == "plan-review" {
                VecDeque::from(["keep".into(), "approve".into(), "dismiss".into()])
            } else {
                VecDeque::new()
            },
            initialized: false,
            results: Vec::new(),
            timeout,
            partial: Vec::new(),
            script_error: None,
        })
    }
    pub async fn send(&mut self, value: &Value) -> Result<()> {
        self.input
            .as_mut()
            .ok_or("RPC input is closed")?
            .write_all((hoh_util_values::js_json_stringify(value) + "\n").as_bytes())
            .await
            .map_err(|e| {
                // A child that already closed its stream is the first error; the failed write to it
                // is only its echo (harness.ts keeps the child's own error).
                if e.kind() == std::io::ErrorKind::BrokenPipe {
                    "protocol stdout closed before completion".to_owned()
                } else {
                    e.to_string()
                }
            })
    }
    pub async fn begin(&mut self, method: &str, params: Value) -> Result<u64> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await?;
        Ok(id)
    }
    pub async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(&json!({"jsonrpc":"2.0","method":method,"params":params}))
            .await
    }
    /// The next non-blank stdout line (`ndJsonStream` skips blank lines); `None` at EOF.
    /// Cancel-safe: a half-read line stays in `partial`.
    async fn next_line(&mut self) -> Result<Option<String>> {
        loop {
            let buffer = self.output.fill_buf().await.map_err(|e| e.to_string())?;
            if buffer.is_empty() {
                if self.partial.is_empty() {
                    return Ok(None);
                }
            } else if let Some(end) = buffer.iter().position(|b| *b == b'\n') {
                self.partial.extend_from_slice(&buffer[..=end]);
                self.output.consume(end + 1);
            } else {
                let length = buffer.len();
                self.partial.extend_from_slice(buffer);
                self.output.consume(length);
                continue;
            }
            let line = String::from_utf8_lossy(&std::mem::take(&mut self.partial)).into_owned();
            if !hoh_util_values::js_trim(&line).is_empty() {
                return Ok(Some(line));
            }
        }
    }
    async fn receive(&mut self) -> Result<()> {
        tokio::time::timeout(self.timeout, self.receive_untimed())
            .await
            .map_err(|_| "protocol response timeout")?
    }
    /// Read frames while `work` runs (the source's client callbacks run beside the step), so an
    /// agent that writes a lot or asks for permission before the marker is never blocked on a pipe.
    pub async fn serving<T>(
        &mut self,
        work: impl std::future::Future<Output = Result<T>>,
    ) -> Result<T> {
        tokio::pin!(work);
        loop {
            // Only the (cancel-safe) read races `work`; a frame that was read is always handled to
            // the end, so a reply to an agent request is never dropped half-sent.
            let line = tokio::select! {
                done = &mut work => return done,
                line = self.next_line() => line?,
            };
            self.handle_line(line.ok_or("protocol stdout closed before completion")?)
                .await?;
        }
    }
    async fn receive_untimed(&mut self) -> Result<()> {
        let line = self
            .next_line()
            .await?
            .ok_or("protocol stdout closed before completion")?;
        self.handle_line(line).await
    }
    async fn handle_line(&mut self, line: String) -> Result<()> {
        self.raw.push_str(&line);
        let frame = hoh_util_values::js_json_parse(&line)
            .map_err(|e| format!("invalid protocol stdout: {e}"))?;
        if !frame.is_object() || frame["jsonrpc"] != "2.0" {
            return Err("invalid JSON-RPC 2.0 frame".into());
        }
        self.frames.push(frame.clone());
        if let Some(method) = frame["method"].as_str() {
            if let Some(id) = frame.get("id") {
                let params = &frame["params"];
                let result = match method {
                    "session/request_permission" => {
                        if let Some(kind) = self.permissions.pop_front() {
                            let offered = params["options"].as_array().cloned().unwrap_or_default();
                            match offered.iter().find(|o| o["kind"] == kind) {
                                Some(option) => {
                                    json!({"outcome":{"outcome":"selected","optionId":option["optionId"]}})
                                }
                                None => {
                                    // harness.ts:316-319: answered `cancelled`, the run is rejected after the step.
                                    let kinds: Vec<&str> =
                                        offered.iter().filter_map(|o| o["kind"].as_str()).collect();
                                    self.script_error = Some(format!(
                                        "snapshot-harness: scripted permission answer {kind} not among the offered options [{}]",
                                        kinds.join(", ")
                                    ));
                                    json!({"outcome":{"outcome":"cancelled"}})
                                }
                            }
                        } else {
                            json!({"outcome":{"outcome":"cancelled"}})
                        }
                    }
                    "approval/request" => {
                        json!({"outcome":self.sdk_approvals.pop_front().ok_or("unexpected SDK approval request")?})
                    }
                    "user-question/ask" => {
                        let choice = self.reviews.pop_front().ok_or("unexpected user question")?;
                        let questions = params["questions"]
                            .as_array()
                            .ok_or("missing plan questions")?;
                        if questions.len() != 1 || questions[0]["intent"]["kind"] != "plan-review" {
                            return Err("invalid plan-review question".into());
                        }
                        if choice == "dismiss" {
                            json!({"cancelled":true})
                        } else {
                            let mut answer = json!({"id":questions[0]["id"],"selected":[if choice=="approve"{"Approve"}else{"Keep planning"}]});
                            if choice == "keep" {
                                answer["custom"] = json!("Add rollback.");
                            }
                            json!({"answers":[answer]})
                        }
                    }
                    _ => return Err(format!("unexpected agent request {method}")),
                };
                self.send(&json!({"jsonrpc":"2.0","id":id,"result":result}))
                    .await?;
            } else {
                if method == "subagent.started"
                    && let Some(id) = frame["params"]["childSessionId"].as_str()
                    && !self.live_sessions.iter().any(|s| s == id)
                {
                    self.live_sessions.push(id.into());
                }
                self.notifications
                    .push(json!({"method":method,"params":frame["params"]}));
            }
        } else if let Some(id) = frame["id"].as_u64() {
            self.pending.insert(id, frame);
        } else {
            return Err("uncorrelated RPC response".into());
        }
        Ok(())
    }
    /// One request has one deadline (`self.timeout`), as the source's per-request timer: notifications
    /// arriving meanwhile do not extend it, and a run of many requests is not bounded as a whole.
    pub async fn finish(&mut self, id: u64) -> Result<Value> {
        let deadline = tokio::time::Instant::now() + self.timeout;
        loop {
            if let Some(frame) = self.pending.remove(&id) {
                if let Some(error) = frame.get("error") {
                    return Err(format!(
                        "RPC error: {}",
                        hoh_util_values::js_json_stringify(error)
                    ));
                }
                return frame
                    .get("result")
                    .cloned()
                    .ok_or("RPC response lacks result".into());
            }
            tokio::time::timeout_at(deadline, self.receive())
                .await
                .map_err(|_| "protocol response timeout")??;
        }
    }
    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.begin(method, params).await?;
        self.finish(id).await
    }
    pub async fn until(&mut self, mut predicate: impl FnMut(&[Value]) -> bool) -> Result<()> {
        let deadline = tokio::time::Instant::now() + self.timeout;
        while !predicate(&self.notifications) {
            tokio::time::timeout_at(deadline, self.receive())
                .await
                .map_err(|_| "notification timeout")??;
        }
        Ok(())
    }
    pub async fn close(&mut self) -> Result<()> {
        self.input.take();
        loop {
            let Some(line) = tokio::time::timeout(self.timeout, self.next_line())
                .await
                .map_err(|_| "protocol shutdown timeout")??
            else {
                break;
            };
            self.raw.push_str(&line);
            let frame = hoh_util_values::js_json_parse(&line)
                .map_err(|e| format!("invalid protocol stdout: {e}"))?;
            self.frames.push(frame);
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Action {
    pub turn: Option<u64>,
    pub content: Option<Value>,
}
pub fn sdk_actions(rows: &[Value], standalone: bool) -> Result<Vec<Action>> {
    let mut actions = Vec::new();
    let mut command: Option<String> = None;
    let mut current: Option<Action> = None;
    let mut command_inputs = HashSet::new();
    for r in rows {
        let d = &r["data"];
        match r["type"].as_str().unwrap_or("") {
            "command/run" if standalone && current.is_none() && d["source"]["kind"] == "user" => {
                command = Some(d["commandId"].as_str().ok_or("command has no id")?.into());
                actions.push(Action{turn:None,content:Some(json!([{"type":"text","text":format!("/{}{}",d["name"].as_str().ok_or("command has no name")?,d["args"].as_str().ok_or("command has no args")?)}]))});
            }
            "agent/inbox/spliced" if command.is_some() => {
                for m in d["inserted"].as_array().into_iter().flatten() {
                    if let Some(id) = m["id"].as_str() {
                        command_inputs.insert(id.to_owned());
                    }
                }
            }
            "command/done" if d["commandId"].as_str() == command.as_deref() => command = None,
            "turn/start" => {
                current = Some(Action {
                    turn: Some(d["turn"].as_u64().ok_or("turn/start lacks turn")?),
                    content: None,
                })
            }
            "user/message" => {
                if let Some(action) = &mut current
                    && action.content.is_none()
                    && d["source"]["kind"] == "user"
                    && d["content"].is_array()
                    && !d["id"]
                        .as_str()
                        .is_some_and(|id| command_inputs.contains(id))
                {
                    action.content = Some(d["content"].clone());
                }
            }
            "turn/end" => {
                if let Some(action) = current.take() {
                    actions.push(action);
                }
            }
            _ => {}
        }
    }
    Ok(actions)
}

fn materialize_input(
    mut content: Value,
    s: &Scenario,
    cwd: &Path,
    sessions: &[String],
) -> Result<Value> {
    for block in content
        .as_array_mut()
        .ok_or("input content must be array")?
    {
        if block["type"] == "image" {
            let id = &block["attachment"]["attachmentId"];
            let a = s.manifest["input"]["attachments"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|a| &a["id"] == id)
                .ok_or("no input bytes for image attachment")?;
            *block = json!({"type":"image","data":a["data"],"mimeType":a["mediaType"]});
        } else {
            let mut error = None;
            map_strings(block, &mut |text, _| {
                let mut text = text.replace("{{cwd}}", &cwd.to_string_lossy());
                for (i, id) in sessions.iter().enumerate() {
                    text = text.replace(&format!("{{{{session:{}}}}}", i + 1), id);
                }
                if text.contains("{{session:") {
                    error = Some("input session token not bound".into());
                }
                text
            });
            if let Some(e) = error {
                return Err(e);
            }
        }
    }
    Ok(content)
}

pub async fn sdk(rpc: &mut Rpc, s: &Scenario, rows: &[Value], cwd: &Path) -> Result<()> {
    let route = rows
        .iter()
        .find(|r| r["type"] == "request/header")
        .map(|r| &r["data"]["header"]["config"])
        .ok_or("fixture has no model")?;
    let mut initialize =
        json!({"cwd":cwd.to_string_lossy(),"provider":route["provider"],"model":route["model"]});
    if s.name == "approval" {
        initialize["approvalRequests"] = json!(true);
    }
    rpc.request("initialize", initialize).await?;
    rpc.initialized = true;
    let standalone = [
        "manual-compact",
        "permission-switch",
        "command-identity",
        "plan-review",
        "goal-rounds",
        "command-attachments",
    ]
    .contains(&s.name.as_str());
    let mut actions = sdk_actions(rows, standalone)?;
    if s.name == "serial-created" {
        actions.retain(|a| a.turn.is_some() && a.content.is_some());
    }
    for action in actions {
        let start = rpc.notifications.len();
        if let Some(content) = action.content {
            let content = materialize_input(content, s, cwd, &rpc.live_sessions)?;
            let receipt = rpc
                .request(
                    "session/query",
                    json!({"sessionId":"fixture-root-session","contentBlocks":content}),
                )
                .await?;
            if receipt["kind"] == "prompt" {
                let message = receipt["messageId"]
                    .as_str()
                    .ok_or("prompt receipt lacks messageId")?
                    .to_owned();
                rpc.until(|ns| {
                    let ns = &ns[start..];
                    let admitted = ns.iter().position(|n| {
                        n["method"] == "session.event"
                            && n["params"]["sessionId"] == "fixture-root-session"
                            && n["params"]["event"]["type"] == "agent/inbox/spliced"
                            && n["params"]["event"]["data"]["inserted"]
                                .as_array()
                                .is_some_and(|ms| ms.iter().any(|m| m["id"] == message))
                    });
                    admitted.is_some_and(|i| {
                        ns[i..].iter().any(|n| {
                            n["method"] == "session.status"
                                && n["params"]["sessionId"] == "fixture-root-session"
                                && n["params"]["status"] == "idle"
                        })
                    })
                })
                .await?;
                let events: Vec<Value> = rpc.notifications[start..]
                    .iter()
                    .filter(|n| {
                        n["method"] == "session.event"
                            && n["params"]["sessionId"] == "fixture-root-session"
                    })
                    .map(|n| n["params"]["event"].clone())
                    .collect();
                rpc.results.push(json!({"sessionId":"fixture-root-session","finalResponse":crate::compare::final_text(&events)}));
            } else if receipt["kind"] != "command" {
                return Err("session/query invalid routing result".into());
            }
        }
        if let Some(turn) = action.turn {
            rpc.until(|ns| {
                ns.iter().any(|n| {
                    n["method"] == "session.event"
                        && n["params"]["sessionId"] == "fixture-root-session"
                        && n["params"]["event"]["type"] == "turn/end"
                        && n["params"]["event"]["data"]["turn"] == turn
                })
            })
            .await?;
        }
    }
    if ["serial-created", "subagent-continuable"].contains(&s.name.as_str()) {
        rpc.request("session/wait", json!({"sessionId":"fixture-root-session"}))
            .await?;
    }
    // SDK client.ts:494–508 treats shutdown failure as diagnostic; EOF/termination reaps the process.
    let _ = tokio::time::timeout(
        Duration::from_millis(1000),
        rpc.request("shutdown", json!({})),
    )
    .await;
    Ok(())
}

/// The step's own `timeoutMs` (harness.ts `waitFor…(…, timeoutMs)`), else the run's wait limit.
fn limit(step: &Value, default: Duration) -> Duration {
    step["timeoutMs"]
        .as_u64()
        .map_or(default, Duration::from_millis)
}

pub async fn wait_file(cwd: &Path, step: &Value, default: Duration) -> Result<()> {
    let path = step["path"].as_str().ok_or("waitForFile lacks path")?;
    let timeout = limit(step, default);
    let wait = async {
        while !cwd.join(path).exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(())
    };
    tokio::time::timeout(timeout, wait).await.map_err(|_| {
        format!(
            "snapshot-harness: workspace file \"{path}\" did not appear within {}ms",
            timeout.as_millis()
        )
    })?
}
pub async fn wait_boundary(root: &Path, id: &str, end: bool, timeout: Duration) -> Result<()> {
    if end {
        crate::waits::turn_end(root, id, timeout).await
    } else {
        crate::waits::turn_start(root, id, timeout, None).await
    }
}

pub async fn acp(
    rpc: &mut Rpc,
    script: &Value,
    cwd: &Path,
    sessions_root: &Path,
    timeout: Duration,
) -> Result<()> {
    let mut session = String::new();
    for step in script["steps"]
        .as_array()
        .ok_or("input steps must be an array")?
    {
        match step["op"].as_str().ok_or("input step lacks op")? {
            "initialize" => {
                rpc.request(
                    "initialize",
                    json!({"protocolVersion":1,"clientCapabilities":{}}),
                )
                .await?;
                rpc.initialized = true;
            }
            "newSession" => {
                let result = rpc
                    .request(
                        "session/new",
                        json!({"cwd":cwd.to_string_lossy(),"mcpServers":[]}),
                    )
                    .await?;
                session = result["sessionId"]
                    .as_str()
                    .ok_or("newSession lacks sessionId")?
                    .into();
                rpc.live_sessions.push(session.clone());
            }
            "newSessionExpectError" => {
                let mut params = json!({"cwd":cwd.to_string_lossy(),"mcpServers":[]});
                if let Some(dirs) = step.get("additionalDirectories") {
                    params["additionalDirectories"] = dirs.clone();
                }
                match rpc.request("session/new", params).await {
                    Err(e) if e.starts_with("RPC error:") => {}
                    Err(e) => return Err(e),
                    Ok(_) => {
                        return Err(
                            "snapshot-harness: expected session/new to be rejected but it succeeded"
                                .into(),
                        );
                    }
                }
            }
            "prompt" | "promptContent" | "promptAndCancel" => {
                if session.is_empty() {
                    return Err(format!(
                        "snapshot-harness: {} before newSession",
                        step["op"].as_str().unwrap_or_default()
                    ));
                }
                let content = if step["op"] == "promptContent" {
                    step["content"].clone()
                } else {
                    json!([{"type":"text","text":step["text"]}])
                };
                let id = rpc
                    .begin(
                        "session/prompt",
                        json!({"sessionId":session,"prompt":content}),
                    )
                    .await?;
                if step["op"] == "promptAndCancel" {
                    // The agent keeps talking while the step waits for its marker.
                    if let Some(file) = step.get("waitForFile") {
                        rpc.serving(wait_file(cwd, file, timeout)).await?;
                    } else {
                        rpc.serving(wait_boundary(sessions_root, &session, false, timeout))
                            .await?;
                    }
                    rpc.notify("session/cancel", json!({"sessionId":session}))
                        .await?;
                }
                rpc.finish(id).await?;
            }
            "waitForTurnEnd" => {
                if session.is_empty() {
                    return Err("snapshot-harness: waitForTurnEnd before newSession".into());
                }
                wait_boundary(sessions_root, &session, true, limit(step, timeout)).await?
            }
            "waitForFile" => wait_file(cwd, step, timeout).await?,
            "promptAndWaitForAgentMessage" => {
                let id = prompt_id(rpc, &session, step, "promptAndWaitForAgentMessage").await?;
                let wanted = step["waitForText"].clone();
                rpc.finish(id).await?;
                rpc.until(|frames| {
                    frames.iter().any(|f| {
                        let update = &f["params"]["update"];
                        update["sessionUpdate"] == "agent_message_chunk"
                            && update["content"]["type"] == "text"
                            && update["content"]["text"] == wanted
                    })
                })
                .await?;
            }
            "promptExpectError" => {
                let id = prompt_id(rpc, &session, step, "promptExpectError").await?;
                match rpc.finish(id).await {
                    Err(e) if e.starts_with("RPC error:") => {}
                    Err(e) => return Err(e),
                    Ok(_) => {
                        return Err(
                            "snapshot-harness: expected the prompt to fail but it succeeded".into(),
                        );
                    }
                }
            }
            "cancel" => {
                need_session(&session, "cancel")?;
                if let Some(file) = step.get("waitForFile") {
                    rpc.serving(wait_file(cwd, file, timeout)).await?;
                }
                rpc.notify("session/cancel", json!({"sessionId":session}))
                    .await?;
            }
            "waitForTurnStart" => {
                need_session(&session, "waitForTurnStart")?;
                crate::waits::turn_start(
                    sessions_root,
                    &session,
                    limit(step, timeout),
                    step["minimumTurn"].as_u64(),
                )
                .await?
            }
            "waitForGoalPhase" => {
                need_session(&session, "waitForGoalPhase")?;
                let phase = step["phase"]
                    .as_str()
                    .ok_or("waitForGoalPhase lacks phase")?;
                crate::waits::goal_phase(sessions_root, &session, phase, limit(step, timeout))
                    .await?
            }
            "waitForInboxMessage" => {
                need_session(&session, "waitForInboxMessage")?;
                let text = step["text"]
                    .as_str()
                    .ok_or("waitForInboxMessage lacks text")?;
                crate::waits::inbox_message(sessions_root, &session, text, limit(step, timeout))
                    .await?
            }
            "waitForTitleAfterTurnEnd" => {
                need_session(&session, "waitForTitleAfterTurnEnd")?;
                crate::waits::title_after_turn_end(sessions_root, &session, limit(step, timeout))
                    .await?
            }
            "waitForEventAfterTurnEnd" => {
                need_session(&session, "waitForEventAfterTurnEnd")?;
                let kind = step["type"]
                    .as_str()
                    .ok_or("waitForEventAfterTurnEnd lacks type")?;
                crate::waits::event_after_turn_end(
                    sessions_root,
                    &session,
                    kind,
                    limit(step, timeout),
                )
                .await?
            }
            "waitForSubagentTurnEnd" => {
                crate::waits::child_turn_end(
                    sessions_root,
                    step["child"].as_u64().unwrap_or(1) as usize,
                    limit(step, timeout),
                    step["minimumTurn"].as_u64().unwrap_or(1),
                )
                .await?
            }
            _ => {
                return Err(format!(
                    "snapshot-harness: unknown input op {}",
                    hoh_util_values::js_json_stringify(step)
                ));
            }
        }
        if let Some(error) = rpc.script_error.take() {
            return Err(error);
        }
    }
    Ok(())
}

fn need_session(session: &str, op: &str) -> Result<()> {
    if session.is_empty() {
        return Err(format!("snapshot-harness: {op} before newSession"));
    }
    Ok(())
}

/// Send a text prompt and return its request id (the reply is read by the caller).
async fn prompt_id(rpc: &mut Rpc, session: &str, step: &Value, op: &str) -> Result<u64> {
    need_session(session, op)?;
    rpc.begin(
        "session/prompt",
        json!({"sessionId":session,"prompt":[{"type":"text","text":step["text"]}]}),
    )
    .await
}
