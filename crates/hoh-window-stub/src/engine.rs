use crate::{
    approval,
    recording::{Action, Recording, kind, materialize},
};
use hoh_protocol::{
    framing,
    transport::{FrameWriter, Incoming},
    validate_params, validate_result,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct SessionState {
    cursor: usize,
    failed: bool,
    cancelled: bool,
    cwd: String,
    history: Vec<Value>,
}
struct Active {
    session: String,
    action: Action,
    reply_id: Option<Value>,
    query: bool,
    last_text: String,
    turn: u64,
    waiters: Vec<Value>,
}
enum Expected {
    Approval(String),
    Question(Value),
    AnyQuestion,
}
enum Answer {
    Continue,
    Rejected,
}
enum Effect {
    None,
    Probe,
    Cwd { session: String, cwd: String },
    Key(hoh_protocol::SecretString),
    Rules(hoh_protocol::SettingsRulesSetParams),
    Model(hoh_protocol::ModelSettings),
}
struct Deferred {
    id: Value,
    result: Value,
    effect: Effect,
}
struct Pending {
    id: String,
    method: String,
    params: Value,
    expected: Expected,
    deadline: Option<Instant>,
}
pub struct Engine<W> {
    writer: FrameWriter<W>,
    recording: Recording,
    fixtures: PathBuf,
    sessions: HashMap<String, SessionState>,
    active: Option<Active>,
    pending: Option<Pending>,
    initialized: bool,
    cwd: String,
    approvals: bool,
    questions: bool,
    key: Option<[u8; 32]>,
    next_request: u64,
    stopped: bool,
    privileged: Option<Deferred>,
    credentials: crate::credentials::StubCredentials,
    rules: Option<hoh_protocol::SettingsRulesSetParams>,
    model: Option<hoh_protocol::ModelSettings>,
}
impl<W: Write> Engine<W> {
    pub fn new(output: W, recording: Recording, fixtures: PathBuf) -> Self {
        Self {
            writer: FrameWriter::new(output),
            recording,
            fixtures,
            sessions: HashMap::new(),
            active: None,
            pending: None,
            initialized: false,
            cwd: String::new(),
            approvals: false,
            questions: false,
            key: None,
            next_request: 0,
            stopped: false,
            privileged: None,
            credentials: Default::default(),
            rules: None,
            model: None,
        }
    }
    pub fn stopped(&self) -> bool {
        self.stopped
    }
    pub fn deadline(&self) -> Option<Instant> {
        self.pending.as_ref().and_then(|p| p.deadline)
    }
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }
    fn write(&mut self, value: Value) -> io::Result<()> {
        self.writer.write(&value)
    }
    fn notify(&mut self, method: &str, params: Value) -> io::Result<()> {
        self.write(framing::notification(method, params))
    }
    fn reply(&mut self, id: Value, result: Value) -> io::Result<()> {
        self.write(framing::success(id, result))
    }
    fn reject(&mut self, id: Value, message: impl Into<String>) -> io::Result<()> {
        self.write(framing::error(id, -32603, message))
    }
    pub fn accept(&mut self, frame: Incoming) -> io::Result<()> {
        if self.deadline().is_some_and(|d| Instant::now() >= d) {
            self.expire()?;
        }
        match frame {
            Incoming::Request { id, method, params } => {
                let id = serde_json::to_value(id)?;
                if let Err(message) = self.request(id.clone(), &method, params) {
                    self.reject(id, message)?;
                }
            }
            Incoming::Response { id, result, error } => {
                if self.pending.as_ref().is_some_and(|p| p.id == id.key()) {
                    let pending = self.pending.take().unwrap();
                    let answer = if let Some(error) = error {
                        Err(error.message)
                    } else {
                        self.check_answer(&pending, &result)
                    };
                    if let Err(message) = answer {
                        if pending.method == "approval/request" && self.key.is_some() {
                            self.notify("approval.resolved", json!({"approvalId":pending.params["approvalId"],"outcome":"cancelled"}))?;
                        }
                        if let Some(deferred) = self.privileged.take() {
                            self.reject(deferred.id, message)?;
                        } else if message == "approval expired" || message == "host request expired"
                        {
                            self.cancel("approval cancelled: expired")?;
                        } else {
                            self.fail(&message)?;
                        }
                    } else if matches!(answer, Ok(Answer::Rejected)) {
                        self.notify(
                            "approval.resolved",
                            json!({"approvalId":pending.params["approvalId"],"outcome":"rejected"}),
                        )?;
                        if let Some(deferred) = self.privileged.take() {
                            self.reject(deferred.id, "approval rejected")?;
                        } else {
                            self.cancel("approval rejected")?;
                        }
                    } else if pending.method == "approval/request" && self.key.is_some() {
                        self.notify("approval.resolved", json!({"approvalId":pending.params["approvalId"],"outcome":result["outcome"]}))?;
                        if let Some(deferred) = self.privileged.take() {
                            self.commit(deferred)?;
                        }
                    }
                }
            }
            Incoming::Notification { .. } => {}
        }
        self.pump()
    }
    pub fn expire(&mut self) -> io::Result<()> {
        if let Some(pending) = self.pending.take() {
            if pending.method == "approval/request" && self.key.is_some() {
                self.notify(
                    "approval.resolved",
                    json!({"approvalId":pending.params["approvalId"],"outcome":"cancelled"}),
                )?;
            } else if pending.method == "user-question/ask" {
                self.notify("user-question.cancelled", json!({"requestId":pending.params["requestId"],"sessionId":pending.params["sessionId"]}))?;
            }
            if let Some(deferred) = self.privileged.take() {
                self.reject(deferred.id, "approval cancelled: expired")?;
            } else {
                self.cancel("approval cancelled: expired")?;
            }
        }
        Ok(())
    }
    fn session(&mut self, id: &str) -> &mut SessionState {
        self.sessions
            .entry(id.into())
            .or_insert_with(|| SessionState {
                cursor: self.recording.first,
                failed: false,
                cancelled: false,
                cwd: self.cwd.clone(),
                history: (0..self.recording.first)
                    .map(|i| self.recording.wire_event(i, id, &self.cwd))
                    .collect(),
            })
    }
    fn request(&mut self, id: Value, method: &str, params: Value) -> Result<(), String> {
        validate_params(method, &params)?;
        if method == "initialize" {
            if self.initialized {
                return Err("runtime is already initialized".into());
            }
            let host = params["capabilities"]["approvals"] == "host";
            if params["hoh"].get("approvalKey").is_some() && !host {
                return Err("approvalKey requires host approvals".into());
            }
            let key = if host {
                if params["hoh"]["protocolVersion"] != 1 {
                    return Err("host approvals require hoh.protocolVersion 1".into());
                }
                Some(approval::key_from_hex(
                    params["hoh"]["approvalKey"]
                        .as_str()
                        .ok_or("host approvals require approvalKey")?,
                )?)
            } else {
                None
            };
            self.key = key;
            self.cwd = params["cwd"].as_str().unwrap().into();
            self.approvals = host || params["approvalRequests"] == true;
            self.questions = params["capabilities"]["userQuestions"] == true;
            self.initialized = true;
            return self
                .reply(
                    id,
                    if host { json!({"serverInfo":{"name":"deepseek-harness-sdk-runtime","version":"0.1.0"},"hoh":{"protocolVersion":1,"approvals":"host"}}) } else {json!({"serverInfo":{"name":"deepseek-harness-sdk-runtime","version":"0.1.0"}})},
                )
                .map_err(|e| e.to_string());
        }
        if !self.initialized {
            return Err("runtime is not initialized".into());
        }
        match method {
            "shutdown" => {
                self.cancel("runtime shutdown").map_err(|e| e.to_string())?;
                self.stopped = true;
                self.reply(id, json!({})).map_err(|e| e.to_string())
            }
            "session/prompt" | "session/query" => {
                self.start(id, method == "session/query", &params)
            }
            "session/wait" => {
                let session = params["sessionId"].as_str().unwrap();
                if !self.sessions.contains_key(session) {
                    return Err("unknown session".into());
                }
                if let Some(active) = &mut self.active
                    && active.session == session
                {
                    active.waiters.push(id);
                    return Ok(());
                }
                if self.sessions[session].failed && !self.sessions[session].cancelled {
                    return Err("recording replay failed or was cancelled".into());
                }
                self.reply(id, json!({})).map_err(|e| e.to_string())
            }
            "session/resume" => {
                let session = params["sessionId"].as_str().unwrap();
                if self.sessions.contains_key(session) {
                    return Err("session already owned".into());
                }
                self.session(session);
                self.reply(id, json!({})).map_err(|e| e.to_string())
            }
            "session/cancel" => {
                if self
                    .active
                    .as_ref()
                    .is_some_and(|a| a.session == params["sessionId"])
                {
                    self.cancel("user cancellation")
                        .map_err(|e| e.to_string())?;
                }
                self.reply(id, json!({})).map_err(|e| e.to_string())
            }
            "session/working-directory/get" | "session/working-directory/set" => {
                let session = params["sessionId"].as_str().unwrap();
                if method.ends_with("/set") {
                    let path = PathBuf::from(params["path"].as_str().unwrap());
                    let state = self.session(session);
                    let cwd = if path.is_absolute() {
                        path
                    } else {
                        PathBuf::from(&state.cwd).join(path)
                    }
                    .to_string_lossy()
                    .into_owned();
                    let effect = Effect::Cwd {
                        session: session.into(),
                        cwd: cwd.clone(),
                    };
                    return self.defer(id, method, params.clone(), json!({"cwd":cwd}), effect);
                }
                let cwd = self.session(session).cwd.clone();
                self.reply(id, json!({"cwd":cwd}))
                    .map_err(|e| e.to_string())
            }
            "session/list" => {
                let mut ids: Vec<_> = self.sessions.keys().cloned().collect();
                ids.sort();
                let sessions: Vec<_> = ids.into_iter().filter(|_| params.get("employeeId").is_none() || params["employeeId"] == "employee-1").map(|id| json!({"id":id,"title":"Записанная сессия","employeeId":"employee-1","updatedAt":0})).collect();
                self.reply(id, json!({"sessions":sessions}))
                    .map_err(|e| e.to_string())
            }
            "session/events" => {
                let session = params["sessionId"].as_str().unwrap();
                let state = self.sessions.get(session).ok_or("unknown session")?;
                let after = params["afterSeq"].as_i64().unwrap_or(-1);
                let limit = params["limit"].as_u64().unwrap();
                if !(1..=1000).contains(&limit) || after < -1 {
                    return Err("events requires limit 1..1000 and afterSeq >= -1".into());
                }
                let events: Vec<_> = state
                    .history
                    .iter()
                    .filter(|e| e["seq"].as_i64().unwrap_or(-1) > after)
                    .take(limit as usize)
                    .cloned()
                    .collect();
                let next = events
                    .last()
                    .and_then(|e| e["seq"].as_i64())
                    .unwrap_or(after);
                self.reply(id, json!({"events":events,"nextSeq":next}))
                    .map_err(|e| e.to_string())
            }
            _ if method.starts_with("hoh/") => {
                let path = self
                    .fixtures
                    .join(format!("{}.response.json", method.replace('/', "-")));
                let value: Value = serde_json::from_str(
                    &fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?,
                )
                .map_err(|e| e.to_string())?;
                let result = value.get("result").ok_or("fixture has no result")?.clone();
                validate_result(method, &result)?;
                let mut result = result;
                if method == "hoh/settings/get"
                    && let Some(model) = &self.model
                {
                    result["model"] =
                        serde_json::to_value(model).map_err(|_| "invalid saved model settings")?;
                }
                if method == "hoh/settings/get"
                    && let Some(rules) = &self.rules
                {
                    result["security"]["internet"] = serde_json::to_value(&rules.internet)
                        .map_err(|_| "invalid setup internet mode")?;
                    result["security"]["folders"] = json!([rules.folder]);
                }
                let (effect, action_params) = match method {
                    "hoh/model/key/set" => {
                        let typed: hoh_protocol::ModelKeySetParams =
                            serde_json::from_value(params.clone())
                                .map_err(|_| "invalid model credential")?;
                        if typed.api_key.expose().is_empty() || typed.api_key.expose().len() > 8192
                        {
                            return Err("model credential must contain 1..8192 bytes".into());
                        }
                        let key_hash =
                            approval::action_hash(&json!({"apiKey":typed.api_key.expose()}));
                        (
                            Effect::Key(typed.api_key),
                            json!({"keyHash":key_hash,"baseUrl":typed.base_url}),
                        )
                    }
                    "hoh/settings/rules/set" => {
                        let rules = serde_json::from_value(params.clone())
                            .map_err(|_| "invalid setup rules")?;
                        (Effect::Rules(rules), params.clone())
                    }
                    "hoh/model/probe" => (Effect::Probe, params.clone()),
                    "hoh/settings/set" if params.get("model").is_some() => {
                        let model = serde_json::from_value(params["model"].clone())
                            .map_err(|_| "invalid model settings")?;
                        (Effect::Model(model), params.clone())
                    }
                    _ => (Effect::None, params.clone()),
                };
                if hoh_protocol::requires_host_approval(method, &params) {
                    self.defer(id, method, action_params, result, effect)
                } else {
                    self.reply(id, result).map_err(|e| e.to_string())
                }
            }
            _ => Err(format!(
                "unknown DeepSeek Harness SDK runtime method: {method}"
            )),
        }
    }
    fn defer(
        &mut self,
        id: Value,
        method: &str,
        params: Value,
        result: Value,
        effect: Effect,
    ) -> Result<(), String> {
        if self.key.is_none() {
            return Err("host approval required".into());
        }
        if self.pending.is_some() || self.active.is_some() {
            return Err("approval already pending".into());
        }
        let request =
            self.host_request("host", method, json!({"method":method,"params":params}))?;
        self.privileged = Some(Deferred { id, result, effect });
        self.reverse(
            "approval/request",
            request,
            Expected::Approval("allowed-once".into()),
        )
    }
    fn commit(&mut self, deferred: Deferred) -> io::Result<()> {
        let mut response = deferred.result;
        let result = match deferred.effect {
            Effect::None => Ok(()),
            Effect::Probe => self.credentials.probe_authenticated().map(|authenticated| {
                response["ok"] = json!(authenticated);
            }),
            Effect::Cwd { session, cwd } => {
                self.session(&session).cwd = cwd;
                Ok(())
            }
            Effect::Key(key) => self.credentials.store_model_key(key.expose()),
            Effect::Rules(rules) => {
                self.rules = Some(rules);
                Ok(())
            }
            Effect::Model(model) => {
                response["model"] = serde_json::to_value(&model)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                self.model = Some(model);
                Ok(())
            }
        };
        match result {
            Ok(()) => self.reply(deferred.id, response),
            Err(message) => self.reject(deferred.id, message),
        }
    }
    fn start(&mut self, id: Value, query: bool, params: &Value) -> Result<(), String> {
        if self.active.is_some() || self.pending.is_some() {
            return Err("stub is replaying an action; wait or cancel it first".into());
        }
        let session = params["sessionId"].as_str().unwrap();
        if session.is_empty() {
            return Err("sessionId must not be empty".into());
        }
        let state = self.session(session);
        if state.failed {
            return Err("recording replay failed or was cancelled".into());
        }
        let cursor = state.cursor;
        let cwd = state.cwd.clone();
        let action = self.recording.action(cursor)?;
        let mut expected = action
            .command
            .as_ref()
            .map(|s| json!([{"type":"text","text":s}]))
            .or_else(|| action.input.clone())
            .ok_or("action has no input")?;
        materialize(&mut expected, session, &cwd);
        if params["contentBlocks"] != expected {
            return Err("input does not match next recording action".into());
        }
        if action.command.is_some() && !query {
            return Err("native commands require session/query".into());
        }
        self.active = Some(Active {
            session: session.into(),
            action,
            reply_id: Some(id),
            query,
            last_text: String::new(),
            turn: 0,
            waiters: Vec::new(),
        });
        Ok(())
    }
    fn pump(&mut self) -> io::Result<()> {
        while self.pending.is_none() && self.active.is_some() {
            let active = self.active.as_ref().unwrap();
            let session = active.session.clone();
            let cursor = self.sessions[&session].cursor;
            if cursor >= active.action.end {
                self.finish()?;
                continue;
            }
            let event = self
                .recording
                .wire_event(cursor, &session, &self.sessions[&session].cwd);
            if kind(&event) == "turn/start" {
                self.active.as_mut().unwrap().turn = event["data"]["turn"].as_u64().unwrap_or(0);
                self.notify(
                    "session.status",
                    json!({"sessionId":session,"status":"running"}),
                )?;
            }
            self.sessions.get_mut(&session).unwrap().cursor += 1;
            self.sessions
                .get_mut(&session)
                .unwrap()
                .history
                .push(event.clone());
            self.notify("session.event", json!({"sessionId":session,"event":event}))?;
            if kind(&event) == "assistant/message" {
                let text = event["data"]["message"]["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|b| b["type"] == "text")
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("");
                if !text.is_empty() {
                    self.active.as_mut().unwrap().last_text = text;
                }
            }
            self.receipt(cursor, &event)?;
            if kind(&event) == "approval/asked" {
                if let Err(message) = self.ask_approval(&event) {
                    self.fail(&message)?;
                }
            } else if kind(&event) == "tool/call"
                && matches!(
                    event["data"]["name"].as_str(),
                    Some("exit_plan_mode" | "ask_user_question")
                )
                && let Err(message) = self.ask_question(&event)
            {
                self.fail(&message)?;
            }
        }
        Ok(())
    }
    fn receipt(&mut self, cursor: usize, event: &Value) -> io::Result<()> {
        let active = self.active.as_ref().unwrap();
        if active.reply_id.is_none() {
            return Ok(());
        }
        let command = active.action.command.is_some();
        if command {
            if active.action.command_done != Some(cursor) || !active.action.has_turn {
                return Ok(());
            }
            let execution = command_execution(event);
            let id = self.active.as_mut().unwrap().reply_id.take().unwrap();
            self.reply(id, json!({"kind":"command","execution":execution}))
        } else if kind(event) == "agent/inbox/spliced"
            && event["data"]["inserted"]
                .as_array()
                .is_some_and(|items| items.iter().any(|m| m["source"]["kind"] == "user"))
        {
            let active = self.active.as_mut().unwrap();
            let id = active.reply_id.take().unwrap();
            let mut result = json!({});
            if active.query {
                result["kind"] = json!("prompt")
            }
            result["messageId"] = json!(active.action.message_id);
            self.reply(id, result)
        } else {
            Ok(())
        }
    }
    fn finish(&mut self) -> io::Result<()> {
        let active = self.active.take().unwrap();
        let cursor = self.sessions[&active.session].cursor;
        if active.action.has_turn {
            self.notify(
                "session.status",
                json!({"sessionId":active.session,"status":"idle"}),
            )?;
        }
        if let Some(command_id) = &active.action.command_id {
            let done = self.recording.events[active.action.command_done.unwrap()].clone();
            let text = if active.action.has_turn {
                active.last_text.clone()
            } else {
                done["data"]["text"].as_str().unwrap_or("").into()
            };
            self.notify("session.query.completed",json!({"sessionId":active.session,"commandId":command_id,"finishReason":"completed","finalResponse":text}))?;
            if let Some(id) = active.reply_id {
                let execution = command_execution(&done);
                self.reply(id, json!({"kind":"command","execution":execution}))?;
            }
        } else if let Some(id) = active.reply_id {
            self.reject(
                id,
                format!("recording action ended at {cursor} without an enqueue receipt"),
            )?;
        }
        for id in active.waiters {
            self.reply(id, json!({}))?;
        }
        Ok(())
    }
    fn reverse(&mut self, method: &str, params: Value, expected: Expected) -> Result<(), String> {
        if self.key.is_some() {
            hoh_protocol::validate_params_host(method, &params)?;
        } else {
            validate_params(method, &params)?;
        }
        self.next_request += 1;
        let id = format!("stub-request-{}", self.next_request);
        let deadline = (method == "approval/request" && self.key.is_some())
            .then(|| Instant::now() + Duration::from_secs(300));
        self.pending = Some(Pending {
            id: format!("s:{id}"),
            method: method.into(),
            params: params.clone(),
            expected,
            deadline,
        });
        self.write(framing::request(json!(id), method, params))
            .map_err(|e| e.to_string())
    }
    fn host_request(&mut self, session: &str, tool: &str, action: Value) -> Result<Value, String> {
        let human_text = self.approval_text(tool, &action["params"])?;
        let params = json!({"sessionId":session,"toolName":tool,
            "approvalId":format!("approval-{}",self.next_request+1),"risk":"high",
            "humanText":human_text,
            "actionHash":approval::action_hash(&action),"action":action,"expiresAt":now_ms()+300_000});
        hoh_protocol::validate_params_host("approval/request", &params)?;
        self.notify("approval.pending",json!({"approvalId":params["approvalId"],"sessionId":session,"risk":params["risk"],"humanText":params["humanText"],"expiresAt":params["expiresAt"]})).map_err(|e|e.to_string())?;
        Ok(params)
    }
    fn approval_text(&self, method: &str, params: &Value) -> Result<String, String> {
        match method {
            "hoh/settings/rules/set" => {
                let rules: hoh_protocol::SettingsRulesSetParams =
                    serde_json::from_value(params.clone()).map_err(|_| "invalid setup rules")?;
                let internet = match rules.internet {
                    hoh_protocol::SetupInternetMode::Ask => "с запросом разрешения",
                    hoh_protocol::SetupInternetMode::Public => "к общедоступным сайтам",
                };
                Ok(format!(
                    "Разрешить работу в папке {:?} и доступ в интернет {internet}?",
                    rules.folder
                ))
            }
            "hoh/model/key/set" => {
                let address = params["baseUrl"]
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .ok_or("model provider address missing")?;
                Ok(format!("Сохранить API-ключ для провайдера {address:?}?"))
            }
            "hoh/model/probe" => {
                let probe: hoh_protocol::ModelProbeParams =
                    serde_json::from_value(params.clone()).map_err(|_| "invalid model probe")?;
                Ok(format!(
                    "Разрешить проверку модели {:?} у провайдера {:?}?",
                    probe.model, probe.base_url
                ))
            }
            _ => Ok(format!("Разрешить действие {method}?")),
        }
    }
    fn ask_approval(&mut self, event: &Value) -> Result<(), String> {
        if !self.approvals {
            return Err("recording needs approvalRequests or host approvals".into());
        }
        let active = self.active.as_ref().unwrap();
        let data = &event["data"];
        let decision = self.recording.events[self.sessions[&active.session].cursor..]
            .iter()
            .find(|e| kind(e) == "approval/decided" && e["data"]["id"] == data["id"])
            .ok_or("approval has no recorded decision")?;
        let outcome = decision["data"]["outcome"]
            .as_str()
            .ok_or("approval outcome missing")?
            .to_owned();
        let mut params = json!({"sessionId":active.session,"toolName":data["toolName"]});
        for key in ["callId", "reason"] {
            if let Some(value) = data.get(key) {
                params[key] = value.clone();
            }
        }
        if self.key.is_some() {
            let session = active.session.clone();
            let action = json!({"method":"approval/request","params":params});
            params = self.host_request(
                &session,
                data["toolName"].as_str().unwrap_or("action"),
                action,
            )?;
            for field in ["reason", "callId"] {
                if let Some(value) = data.get(field) {
                    params[field] = value.clone();
                }
            }
        }
        self.reverse("approval/request", params, Expected::Approval(outcome))
    }
    fn ask_question(&mut self, event: &Value) -> Result<(), String> {
        if !self.questions {
            return Err("recording needs capabilities.userQuestions".into());
        }
        let active = self.active.as_ref().unwrap();
        let data = &event["data"];
        let args: Value = serde_json::from_str(
            data["arguments"]
                .as_str()
                .ok_or("question arguments missing")?,
        )
        .map_err(|e| e.to_string())?;
        let (questions, expected) = if data["name"] == "exit_plan_mode" {
            let result = self
                .recording
                .tool_result(self.sessions[&active.session].cursor, &data["callId"])
                .ok_or("plan review result missing")?;
            let text = result["data"]["message"]["content"][0]["text"]
                .as_str()
                .unwrap_or("");
            let expected = if text.contains("dismissed") {
                json!({"cancelled":true})
            } else if text.contains("Plan approved") {
                json!({"answers":[{"id":"plan-review","selected":["Approve"]}]})
            } else {
                let mut item = json!({"id":"plan-review","selected":["Keep planning"]});
                if let Some((_, feedback)) = text.split_once("their feedback: ") {
                    item["custom"] = json!(feedback)
                }
                json!({"answers":[item]})
            };
            (
                json!([{"id":"plan-review","header":"Plan review","question":"Approve this plan and leave plan mode?","detail":args["plan"],
                "options":[{"label":"Approve","description":"Leave plan mode; the plan is carried out from the next step."},{"label":"Keep planning","description":"Stay in plan mode; feedback goes back to the model."}],
                "intent":{"kind":"plan-review","approve":"Approve","callId":data["callId"]}}]),
                Expected::Question(expected),
            )
        } else {
            (args["questions"].clone(), Expected::AnyQuestion)
        };
        let params = json!({"requestId":format!("stub-question-{}",self.next_request+1),"sessionId":active.session,"questions":questions});
        self.reverse("user-question/ask", params, expected)
    }
    fn check_answer(&self, pending: &Pending, result: &Value) -> Result<Answer, String> {
        if pending
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err("host request expired".into());
        }
        if self.key.is_some() {
            hoh_protocol::validate_result_host(&pending.method, result)?;
        } else {
            validate_result(&pending.method, result)?;
        }
        match &pending.expected {
            Expected::Approval(expected) => {
                if let Some(key) = &self.key {
                    let decision =
                        serde_json::from_value(result.clone()).map_err(|e| e.to_string())?;
                    let request = serde_json::from_value(pending.params.clone())
                        .map_err(|e| e.to_string())?;
                    approval::verify_decision(key, &request, &decision, now_ms())?;
                }
                if result["outcome"] != *expected {
                    if self.key.is_some() && result["outcome"] == "rejected" {
                        return Ok(Answer::Rejected);
                    }
                    return Err("approval outcome differs from recording".into());
                }
            }
            Expected::Question(expected) if result != expected => {
                return Err("question answer differs from recording".into());
            }
            Expected::AnyQuestion if result.get("cancelled").is_none() => {
                let questions = pending.params["questions"].as_array().unwrap();
                let answers = result["answers"].as_array().unwrap();
                if answers.len() != questions.len()
                    || questions
                        .iter()
                        .any(|q| answers.iter().filter(|a| a["id"] == q["id"]).count() != 1)
                {
                    return Err("answer each question exactly once".into());
                }
            }
            _ => {}
        }
        Ok(Answer::Continue)
    }
    fn cancel(&mut self, message: &str) -> io::Result<()> {
        if let Some(pending) = self.pending.take() {
            if pending.method == "user-question/ask" {
                self.notify("user-question.cancelled",json!({"requestId":pending.params["requestId"],"sessionId":pending.params["sessionId"]}))?;
            } else if self.key.is_some() {
                self.notify(
                    "approval.resolved",
                    json!({"approvalId":pending.params["approvalId"],"outcome":"cancelled"}),
                )?;
            }
        }
        if let Some(deferred) = self.privileged.take() {
            self.reject(deferred.id, message)?;
        }
        self.end_failed(message, true)
    }
    fn fail(&mut self, message: &str) -> io::Result<()> {
        self.end_failed(message, false)
    }
    fn end_failed(&mut self, message: &str, cancelled: bool) -> io::Result<()> {
        let Some(active) = self.active.take() else {
            return Ok(());
        };
        let state = self.sessions.get_mut(&active.session).unwrap();
        state.failed = true;
        state.cancelled = cancelled;
        let seq = state.cursor;
        if active.action.has_turn {
            let reason = if cancelled {
                json!({"kind":"aborted","reason":{"kind":"user"}})
            } else {
                json!({"kind":"error","error":{"message":message,"code":"UNKNOWN"}})
            };
            let event = json!({"type":"turn/end","seq":seq,"time":0,"data":{"turn":active.turn,"reason":reason}});
            self.sessions
                .get_mut(&active.session)
                .unwrap()
                .history
                .push(event.clone());
            self.notify(
                "session.event",
                json!({"sessionId":active.session,"event":event}),
            )?;
            self.notify(
                "session.status",
                json!({"sessionId":active.session,"status":"idle"}),
            )?;
        }
        if let Some(command_id) = active.action.command_id {
            self.notify("session.query.completed",json!({"sessionId":active.session,"commandId":command_id,"finishReason":if cancelled {"aborted"} else {"error"},"finalResponse":message}))?;
        }
        if let Some(id) = active.reply_id {
            self.reject(id, message)?;
        }
        for id in active.waiters {
            if cancelled {
                self.reply(id, json!({}))?;
            } else {
                self.reject(id, message)?;
            }
        }
        Ok(())
    }
}
fn command_execution(event: &Value) -> Value {
    let data = &event["data"];
    let mut result = json!({"kind":data["kind"]});
    for key in ["text", "sourceEventSeq"] {
        if let Some(value) = data.get(key) {
            result[key] = value.clone();
        }
    }
    json!({"commandId":data["commandId"],"result":result})
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_millis() as u64
}

#[cfg(test)]
mod expiry_tests {
    use super::*;
    #[test]
    fn credential_effect_stays_private_until_a_valid_host_decision() {
        let source = std::env::var_os("HOH_DSH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness"));
        let recording =
            Recording::read(&source.join("snapshots/sdk/approval/session.v4.jsonl")).unwrap();
        let mut engine = Engine::new(
            Vec::new(),
            recording,
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../protocol/fixtures"),
        );
        engine.request(json!(1),"initialize",json!({"cwd":"/w","provider":"p","model":"m","capabilities":{"approvals":"host"},"hoh":{"protocolVersion":1,"approvalKey":"42".repeat(32)}})).unwrap();
        for valid in [false, true] {
            engine
                .request(
                    json!(2),
                    "hoh/model/key/set",
                    json!({"apiKey":"test-key-only-in-credentials","baseUrl":"https://api.example.test/v1"}),
                )
                .unwrap();
            assert!(!engine.credentials.probe_authenticated().unwrap());
            let pending = engine.pending.as_ref().unwrap();
            let request: hoh_protocol::HostApprovalRequest =
                serde_json::from_value(pending.params.clone()).unwrap();
            let id = pending.id.strip_prefix("s:").unwrap().to_owned();
            let decision = if valid {
                json!({"approvalId":request.approval_id,"outcome":"allowed-once","mac":approval::mac(&[0x42;32],&request,"allowed-once")})
            } else {
                json!({"outcome":"allowed-once"})
            };
            engine
                .accept(
                    hoh_protocol::transport::classify(framing::success(json!(id), decision))
                        .unwrap(),
                )
                .unwrap();
            assert_eq!(engine.credentials.probe_authenticated().unwrap(), valid);
        }
    }
    #[test]
    fn initialize_rejects_weak_key_before_mutating_state() {
        let source = std::env::var_os("HOH_DSH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness"));
        let recording =
            Recording::read(&source.join("snapshots/sdk/approval/session.v4.jsonl")).unwrap();
        let mut engine = Engine::new(Vec::new(), recording, PathBuf::new());
        for key in ["00".repeat(32), "42".repeat(31), "42".repeat(33)] {
            assert!(engine.request(json!(1),"initialize",json!({"cwd":"/w","provider":"p","model":"m","capabilities":{"approvals":"host"},"hoh":{"protocolVersion":1,"approvalKey":key}})).is_err());
            assert!(!engine.initialized);
            assert!(engine.key.is_none());
        }
    }
    #[test]
    fn expiry_cancels_approval_without_failed_turn() {
        let source = std::env::var_os("HOH_DSH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness"));
        let recording =
            Recording::read(&source.join("snapshots/sdk/approval/session.v4.jsonl")).unwrap();
        let content = recording.events[recording.first]["data"]["inserted"][0]["content"].clone();
        let mut engine = Engine::new(
            Vec::new(),
            recording,
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../protocol/fixtures"),
        );
        engine.accept(hoh_protocol::transport::classify(framing::request(json!(1),"initialize",json!({"cwd":"/w","provider":"p","model":"m","capabilities":{"approvals":"host"},"hoh":{"protocolVersion":1,"approvalKey":"42".repeat(32)}}))).unwrap()).unwrap();
        engine
            .accept(
                hoh_protocol::transport::classify(framing::request(
                    json!(2),
                    "session/prompt",
                    json!({"sessionId":"s","contentBlocks":content}),
                ))
                .unwrap(),
            )
            .unwrap();
        engine.pending.as_mut().unwrap().deadline = Some(Instant::now() - Duration::from_secs(1));
        engine
            .accept(
                hoh_protocol::transport::classify(framing::success(json!("late"), json!({})))
                    .unwrap(),
            )
            .unwrap();
        assert!(engine.pending.is_none());
        let output = String::from_utf8(engine.writer.into_inner()).unwrap();
        let frames: Vec<Value> = output
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert!(
            frames.iter().any(
                |f| f["method"] == "approval.resolved" && f["params"]["outcome"] == "cancelled"
            )
        );
        assert!(
            !frames
                .iter()
                .any(|f| f["params"]["event"]["data"]["reason"]["kind"] == "error")
        );
    }
}
