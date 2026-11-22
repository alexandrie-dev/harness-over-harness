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
/// What a conversation bound to Claude runs with.
struct ClaudeBinding {
    /// The model chosen when the chat was created (`default` when none).
    model: String,
    /// The mode of this conversation's live process: set when a turn starts it, kept between turns.
    running: Option<&'static str>,
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
    Settings(hoh_protocol::SettingsSetParams),
    Access(&'static str),
    Select(Option<String>),
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
    settings: Option<hoh_protocol::Settings>,
    /// Test clients without a host (the UI checks drive the stub directly): privileged methods are applied at once, as the
    /// host's approval would have allowed them. Off by default: a real host must sign every decision.
    local_approvals: bool,
    /// Access mode of the Claude executor (`confirm` | `full`), kept in memory like the engine keeps it.
    access_mode: &'static str,
    /// Model chosen with `hoh/executor/select` (`None`: new chats run on the engine's own model).
    selected: Option<String>,
    /// Every prompt that went through the Claude executor, for the window's checks (`hoh/stub/executor-calls`).
    claude_calls: Vec<Value>,
    /// The conversations bound to Claude, as the engine's binding record keeps them: bound at the
    /// first prompt of a new chat while Claude is selected, and kept whatever is selected later.
    claude_sessions: HashMap<String, ClaudeBinding>,
    /// The mode of the Claude process started last, while it runs (as `running_launch` of the engine).
    last_running: Option<&'static str>,
    /// A Claude turn waiting for the answer to its confirmation question.
    claude_turn: Option<(String, u64)>,
    /// Scenario file and fallback text for `discover` (HOH_STUB_CLAUDE_FILE / HOH_STUB_CLAUDE, read at start).
    claude_file: Option<PathBuf>,
    claude_state: Option<String>,
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
            settings: None,
            local_approvals: false,
            access_mode: "confirm",
            selected: None,
            claude_calls: Vec::new(),
            claude_sessions: HashMap::new(),
            last_running: None,
            claude_turn: None,
            claude_file: std::env::var_os("HOH_STUB_CLAUDE_FILE").map(PathBuf::from),
            claude_state: std::env::var("HOH_STUB_CLAUDE").ok(),
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
                    if pending.method == "user-question/ask"
                        && let Some(turn) = self.claude_turn.take()
                    {
                        let allowed = result["answers"][0]["selected"][0] == "Разрешить";
                        self.claude_finish(turn, allowed)?;
                        return Ok(());
                    }
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
    pub fn set_local_approvals(&mut self, on: bool) {
        self.local_approvals = on;
    }
    /// `hoh/executor/access/get` and `/set` for the Claude executor. The contract is not in hoh-protocol yet,
    /// so the two methods are checked here; `set` is privileged and goes through the same host approval as the other settings.
    fn executor_access(&mut self, id: Value, method: &str, params: Value) -> Result<(), String> {
        if params["executor"] != "claude" {
            return Err("executor access: only the claude executor is supported".into());
        }
        match method {
            "hoh/executor/access/get" => {
                let mode = self.access_mode;
                let asked = params["sessionId"].as_str();
                let binding = asked.and_then(|id| self.claude_sessions.get(id));
                // The mode of the process asked about: the conversation's own, else (no
                // `sessionId`) the one started last; none running: what the next start would use.
                let shown = match asked {
                    Some(_) => binding.and_then(|bound| bound.running),
                    None => self.last_running,
                }
                .unwrap_or(mode);
                let (flags, text) = if shown == "full" {
                    (
                        // The order and the switches of `ClaudeConfig::launch_flags` (confirm/full).
                        json!([
                            "--permission-mode",
                            "bypassPermissions",
                            "--setting-sources",
                            "user",
                            "--permission-prompt-tool",
                            "stdio",
                            "--allow-dangerously-skip-permissions"
                        ]),
                        "Полный доступ: Помощник сам удаляет и меняет файлы в любых папках, куда есть доступ у вас, запускает программы и выходит в интернет, не спрашивая. Сделанное нельзя отменить.",
                    )
                } else {
                    (
                        // Confirm closes the network: the CLI's two network tools stay off.
                        json!([
                            "--permission-mode",
                            "default",
                            "--setting-sources",
                            "user",
                            "--permission-prompt-tool",
                            "stdio",
                            "--disallowedTools",
                            "WebFetch,WebSearch"
                        ]),
                        "С подтверждением: команды, правки файлов и сеть Claude выполняет после вашего подтверждения",
                    )
                };
                let selected = self.selected.as_ref().map_or(
                    json!({"executor":null,"model":null}),
                    |model| json!({"executor":"claude","model":model}),
                );
                let mut result = json!({"executor":"claude","mode":mode,"effective":{"mode":shown,"flags":flags,"text":text},"selected":selected});
                if let Some(id) = asked {
                    result["session"] = match self.claude_sessions.get(id) {
                        Some(bound) => {
                            json!({"executor":"claude","model":bound.model,"privacy":"remote",
                            "mode":mode,"running":bound.running,"pending":bound.running.is_some_and(|r| r != mode)})
                        }
                        None => json!({"executor":null,"model":null,"privacy":null,
                            "mode":null,"running":null,"pending":false}),
                    };
                }
                self.reply(id, result).map_err(|e| e.to_string())
            }
            "hoh/executor/access/set" => {
                let mode: &'static str = match params["mode"].as_str() {
                    Some("confirm") => "confirm",
                    Some("full") => "full",
                    _ => return Err("executor access: mode must be confirm or full".into()),
                };
                // The engine's answer (`ExecutorAccessSetResult`): the executor is checked above.
                self.defer(
                    id,
                    method,
                    params,
                    json!({"executor":"claude","mode":mode}),
                    Effect::Access(mode),
                )
            }
            _ => Err(format!(
                "unknown DeepSeek Harness SDK runtime method: {method}"
            )),
        }
    }
    /// The state of the `claude` command line tool as the window sees it. A scenario file (re-read on every call, so a
    /// check can change it between two discovers) wins over the environment: found | noauth | missing | unknown.
    fn claude_scenario(&self) -> String {
        self.claude_file
            .as_ref()
            .and_then(|path| fs::read_to_string(path).ok())
            .or_else(|| self.claude_state.clone())
            .map(|text| text.trim().to_owned())
            .unwrap_or_else(|| "found".into())
    }
    /// `hoh/executor/discover`: read-only, a real call every time (the contract of the engine, o1).
    fn executor_discover(&mut self, id: Value, params: &Value) -> Result<(), String> {
        if params["executor"] != "claude" {
            return Err("executor discover: only the claude executor is supported".into());
        }
        let models = json!([
            {"id":"default","isDefault":true},
            {"id":"opus","displayName":"Opus"}
        ]);
        let result = match self.claude_scenario().as_str() {
            "noauth" => {
                json!({"executor":"claude","installed":true,"version":"2.1.3","versionStatus":"supported",
                "auth":"unauthenticated","models":[],"detail":"claude: вход не выполнен"})
            }
            "missing" => {
                json!({"executor":"claude","installed":false,"version":null,"versionStatus":"unknown",
                "auth":"unknown","models":[],"detail":"claude: команда не найдена"})
            }
            "unsupported" => {
                json!({"executor":"claude","installed":true,"version":"1.0.0","versionStatus":"unsupported",
                "auth":"unknown","models":[],"detail":"Версия claude 1.0.0 не поддерживается"})
            }
            "unknown" => {
                json!({"executor":"claude","installed":true,"version":null,"versionStatus":"unknown",
                "auth":"unknown","models":[],"detail":"Версию claude не удалось определить"})
            }
            _ => {
                json!({"executor":"claude","installed":true,"version":"2.1.3","versionStatus":"supported",
                "auth":"authenticated","models":models,"detail":null})
            }
        };
        self.reply(id, result).map_err(|e| e.to_string())
    }
    /// `hoh/executor/select`: privileged. `executor: null` returns new chats to the engine's own model.
    fn executor_select(&mut self, id: Value, params: Value) -> Result<(), String> {
        let model = params["model"]
            .as_str()
            .filter(|model| !model.is_empty())
            .ok_or("executor select: model must be a non-empty string")?
            .to_owned();
        let (selected, result) = match params["executor"].as_str() {
            Some("claude") => (
                Some(model.clone()),
                json!({"executor":"claude","model":model}),
            ),
            None if params["executor"].is_null() => (None, json!({"executor":null,"model":model})),
            _ => return Err("executor select: only the claude executor is supported".into()),
        };
        self.defer(
            id,
            "hoh/executor/select",
            params,
            result,
            Effect::Select(selected),
        )
    }
    fn claude_event(&mut self, session: &str, kind: &str, seq: u64, data: Value) -> io::Result<()> {
        let event = json!({"type":kind,"seq":seq,"time":0,"data":data});
        self.session(session).history.push(event.clone());
        self.notify("session.event", json!({"sessionId":session,"event":event}))
    }
    /// A prompt goes to Claude when its conversation is bound to it, or is a new chat while Claude
    /// is selected (a known conversation is never moved by what is selected later).
    fn goes_to_claude(&self, params: &Value) -> bool {
        let id = params["sessionId"].as_str().unwrap_or("");
        self.claude_sessions.contains_key(id)
            || (self.selected.is_some() && !self.sessions.contains_key(id))
    }
    /// A prompt of a new chat while Claude is selected: recorded for the checks, then a short synthetic turn.
    /// In `confirm` the turn stops at a confirmation question; in `full` there is none.
    fn claude_prompt(&mut self, id: Value, query: bool, params: &Value) -> Result<(), String> {
        if self.active.is_some() || self.pending.is_some() || self.claude_turn.is_some() {
            return Err("stub is replaying an action; wait or cancel it first".into());
        }
        let session = params["sessionId"].as_str().unwrap_or("").to_owned();
        if session.is_empty() {
            return Err("sessionId must not be empty".into());
        }
        let text = params["contentBlocks"][0]["text"].as_str().unwrap_or("");
        let mode = self.access_mode;
        // The first prompt of a new chat binds it to the selected model; a bound one goes on as it
        // was bound, whatever is selected now.
        let model = match self.claude_sessions.get(&session) {
            Some(bound) => bound.model.clone(),
            None => self.selected.clone().unwrap_or_default(),
        };
        // A turn starts the process anew when its mode is not the chosen one (the engine does it
        // at the start of the turn).
        self.claude_sessions
            .entry(session.clone())
            .or_insert_with(|| ClaudeBinding {
                model: if model.is_empty() {
                    "default".into()
                } else {
                    model.clone()
                },
                running: None,
            })
            .running = Some(mode);
        self.last_running = Some(mode);
        self.claude_calls.push(
            json!({"executor":"claude","model":model,"mode":mode,"sessionId":session,"text":text}),
        );
        let turn = self.claude_calls.len() as u64;
        let mut reply = json!({"messageId":format!("claude-user-{turn}")});
        if query {
            reply["kind"] = json!("prompt");
        }
        self.reply(id, reply).map_err(|e| e.to_string())?;
        let io = |e: io::Error| e.to_string();
        self.claude_event(
            &session,
            "agent/inbox/spliced",
            turn * 10,
            json!({"inserted":[{"id":format!("claude-user-{turn}"),
            "source":{"kind":"user"},"content":[{"type":"text","text":text}]}]}),
        )
        .map_err(io)?;
        self.notify(
            "session.status",
            json!({"sessionId":session,"status":"running"}),
        )
        .map_err(io)?;
        self.claude_event(&session, "turn/start", turn * 10 + 1, json!({"turn":turn}))
            .map_err(io)?;
        if mode == "confirm" && self.questions {
            let params = json!({"requestId":format!("stub-question-{}",self.next_request+1),"sessionId":session,
                "questions":[{"id":"claude-command","header":"Claude","question":"Claude хочет выполнить команду ls в рабочей папке. Разрешить?",
                    "options":[{"label":"Разрешить"},{"label":"Не разрешать"}]}]});
            self.claude_turn = Some((session, turn));
            self.reverse("user-question/ask", params, Expected::AnyQuestion)?;
            return Ok(());
        }
        self.claude_turn = Some((session, turn));
        let turn = self.claude_turn.take().unwrap();
        self.claude_finish(turn, true).map_err(io)
    }
    fn claude_finish(&mut self, (session, turn): (String, u64), allowed: bool) -> io::Result<()> {
        let model = self
            .claude_sessions
            .get(&session)
            .map(|bound| bound.model.clone())
            .unwrap_or_default();
        let answer = if allowed {
            format!(
                "Готово. Ответ Claude (модель {model}, режим {}).",
                self.access_mode
            )
        } else {
            "Хорошо, команду не выполняю.".to_owned()
        };
        self.claude_event(&session, "assistant/message", turn * 10 + 2, json!({"turn":turn,
            "message":{"id":format!("claude-answer-{turn}"),"role":"assistant","content":[{"type":"text","text":answer}]}}))?;
        self.claude_event(
            &session,
            "turn/end",
            turn * 10 + 3,
            json!({"turn":turn,"reason":{"kind":"completed"}}),
        )?;
        self.notify(
            "session.status",
            json!({"sessionId":session,"status":"idle"}),
        )
    }
    fn request(&mut self, id: Value, method: &str, params: Value) -> Result<(), String> {
        if self.initialized {
            if method.starts_with("hoh/executor/access/") {
                return self.executor_access(id, method, params);
            }
            match method {
                "hoh/executor/discover" => return self.executor_discover(id, &params),
                "hoh/executor/select" => return self.executor_select(id, params),
                "hoh/stub/executor-calls" => {
                    let calls = json!({"calls":self.claude_calls});
                    return self.reply(id, calls).map_err(|e| e.to_string());
                }
                "session/prompt" | "session/query" if self.goes_to_claude(&params) => {
                    return self.claude_prompt(id, method == "session/query", &params);
                }
                _ => {}
            }
        }
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
                    && let Some(settings) = &self.settings
                {
                    result =
                        serde_json::to_value(settings).map_err(|_| "invalid saved settings")?;
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
                    "hoh/settings/set" => {
                        let settings = serde_json::from_value(params.clone())
                            .map_err(|_| "invalid settings")?;
                        (Effect::Settings(settings), params.clone())
                    }
                    _ => (Effect::None, params.clone()),
                };
                if hoh_protocol::requires_host_approval(method, &params) {
                    self.defer(id, method, action_params, result, effect)
                } else if method == "hoh/settings/set" {
                    self.commit(Deferred { id, result, effect })
                        .map_err(|error| error.to_string())
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
            if self.local_approvals {
                return self
                    .commit(Deferred { id, result, effect })
                    .map_err(|error| error.to_string());
            }
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
                let mut settings = self.current_settings()?;
                settings.security.internet = match rules.internet {
                    hoh_protocol::SetupInternetMode::Ask => hoh_protocol::InternetMode::Ask,
                    hoh_protocol::SetupInternetMode::Public => hoh_protocol::InternetMode::Public,
                };
                settings.security.folders = vec![rules.folder];
                self.settings = Some(settings);
                Ok(())
            }
            Effect::Access(mode) => {
                self.access_mode = mode;
                Ok(())
            }
            Effect::Select(model) => {
                self.selected = model;
                Ok(())
            }
            Effect::Settings(update) => {
                let mut settings = self.current_settings()?;
                if let Some(model) = update.model {
                    settings.model = model;
                }
                if let Some(security) = update.security {
                    settings.security = security;
                }
                if let Some(view) = update.view {
                    settings.view = view;
                }
                response = serde_json::to_value(&settings)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                self.settings = Some(settings);
                Ok(())
            }
        };
        match result {
            Ok(()) => self.reply(deferred.id, response),
            Err(message) => self.reject(deferred.id, message),
        }
    }
    fn current_settings(&self) -> io::Result<hoh_protocol::Settings> {
        if let Some(settings) = &self.settings {
            return Ok(settings.clone());
        }
        let path = self.fixtures.join("hoh-settings-get.response.json");
        let frame: Value = serde_json::from_str(&fs::read_to_string(&path)?)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        serde_json::from_value(frame["result"].clone())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
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
            "session/working-directory/set" => {
                let request: hoh_protocol::SessionWorkingDirectorySetParams =
                    serde_json::from_value(params.clone())
                        .map_err(|_| "invalid working directory")?;
                Ok(format!(
                    "Разрешить работу в папке {}?",
                    quoted(&request.path)
                ))
            }
            "hoh/executor/select" => Ok(
                match (params["executor"].as_str(), params["model"].as_str()) {
                    (Some(executor), Some(model)) => {
                        format!(
                            "Отвечать будет {} (модель {model})?",
                            executor_name(executor)
                        )
                    }
                    _ => "Отвечать будет модель на этом компьютере?".to_owned(),
                },
            ),
            "hoh/executor/access/set" => {
                let name = executor_name(params["executor"].as_str().unwrap_or("claude"));
                Ok(if params["mode"] == "full" {
                    format!(
                        "Включить полный доступ для {name}? Помощник сам удаляет и меняет файлы в любых папках, куда есть доступ у вас, запускает программы и выходит в интернет, не спрашивая. Сделанное нельзя отменить."
                    )
                } else {
                    format!("Вернуть режим «Спрашивать разрешение» для {name}?")
                })
            }
            "hoh/settings/set" => {
                let request: hoh_protocol::SettingsSetParams =
                    serde_json::from_value(params.clone()).map_err(|_| "invalid settings")?;
                let mut changes = Vec::new();
                if let Some(model) = request.model {
                    changes.push(format!(
                        "модель — {}, провайдер — {}",
                        quoted(&model.model),
                        quoted(&model.base_url)
                    ));
                }
                if let Some(security) = request.security {
                    changes.push(format!("права: {}", permissions_text(&security)));
                }
                if let Some(view) = request.view {
                    changes.push(format!(
                        "вид — {}, размер текста — {}",
                        theme_text(&view.theme),
                        view.font_size
                    ));
                }
                if changes.is_empty() {
                    return Err("settings update has no changes".into());
                }
                Ok(format!("Изменить настройки: {}?", changes.join("; ")))
            }
            "hoh/employee/create" => {
                let request: hoh_protocol::EmployeeCreateParams =
                    serde_json::from_value(params.clone()).map_err(|_| "invalid employee draft")?;
                Ok(format!(
                    "Создать сотрудника {} с ролью {}; правила — {}; назначенные задачи — {}; права: {}?",
                    quoted(&request.draft.name),
                    quoted(&request.draft.role),
                    quoted_list(&request.draft.rules),
                    quoted_list(&request.draft.task_ids),
                    permissions_text(&request.draft.permissions)
                ))
            }
            "hoh/employee/update" => {
                let request: hoh_protocol::EmployeeUpdateParams =
                    serde_json::from_value(params.clone())
                        .map_err(|_| "invalid employee update")?;
                let mut changes = Vec::new();
                if let Some(name) = request.patch.name {
                    changes.push(format!("имя — {}", quoted(&name)));
                }
                if let Some(role) = request.patch.role {
                    changes.push(format!("роль — {}", quoted(&role)));
                }
                if let Some(rules) = request.patch.rules {
                    changes.push(format!("правила — {}", quoted_list(&rules)));
                }
                if let Some(task_ids) = request.patch.task_ids {
                    changes.push(format!("назначенные задачи — {}", quoted_list(&task_ids)));
                }
                if let Some(permissions) = request.patch.permissions {
                    changes.push(format!("права: {}", permissions_text(&permissions)));
                }
                if changes.is_empty() {
                    return Err("privileged employee update has no rights change".into());
                }
                Ok(format!(
                    "Изменить сотрудника {}: {}?",
                    quoted(&request.id),
                    changes.join("; ")
                ))
            }
            "hoh/settings/rules/set" => {
                let rules: hoh_protocol::SettingsRulesSetParams =
                    serde_json::from_value(params.clone()).map_err(|_| "invalid setup rules")?;
                let internet = match rules.internet {
                    hoh_protocol::SetupInternetMode::Ask => "с запросом разрешения",
                    hoh_protocol::SetupInternetMode::Public => "к общедоступным сайтам",
                };
                Ok(format!(
                    "Разрешить работу в папке {} и доступ в интернет {internet}?",
                    quoted(&rules.folder)
                ))
            }
            "hoh/model/key/set" => {
                let address = params["baseUrl"]
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .ok_or("model provider address missing")?;
                Ok(format!(
                    "Сохранить API-ключ для провайдера {}?",
                    quoted(address)
                ))
            }
            "hoh/model/probe" => {
                let probe: hoh_protocol::ModelProbeParams =
                    serde_json::from_value(params.clone()).map_err(|_| "invalid model probe")?;
                Ok(format!(
                    "Разрешить проверку модели {} у провайдера {}?",
                    quoted(&probe.model),
                    quoted(&probe.base_url)
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

/// An executor's name as a word in a sentence: `claude` → `Claude`.
fn executor_name(executor: &str) -> String {
    let mut chars = executor.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

fn quoted(value: &str) -> String {
    format!("«{value}»")
}

fn quoted_list(values: &[String]) -> String {
    if values.is_empty() {
        "нет".into()
    } else {
        values
            .iter()
            .map(|value| quoted(value))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn permissions_text(permissions: &hoh_protocol::Permissions) -> String {
    let internet = match &permissions.internet {
        hoh_protocol::InternetMode::Ask => "с запросом разрешения",
        hoh_protocol::InternetMode::Public => "общедоступные сайты",
        hoh_protocol::InternetMode::Deny => "запрещён",
    };
    format!(
        "интернет — {internet}; сайты — {}; папки — {}; изменение файлов — {}; запуск программ — {}; сеть для программ — {}; действия от вашего имени — {}",
        quoted_list(&permissions.domains),
        quoted_list(&permissions.folders),
        allowed(permissions.write_files),
        allowed(permissions.programs),
        quoted_list(&permissions.program_network),
        allowed(permissions.act_as_user)
    )
}

fn allowed(value: bool) -> &'static str {
    if value {
        "разрешено"
    } else {
        "запрещено"
    }
}

fn theme_text(theme: &hoh_protocol::Theme) -> &'static str {
    match theme {
        hoh_protocol::Theme::Light => "светлая тема",
        hoh_protocol::Theme::Dark => "тёмная тема",
        hoh_protocol::Theme::System => "как в системе",
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
    fn plain_engine() -> Engine<Vec<u8>> {
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
        engine
            .request(
                json!(1),
                "initialize",
                json!({"cwd":"/w","provider":"p","model":"m","capabilities":{}}),
            )
            .unwrap();
        engine
    }
    /// `access/get` of one conversation, as the window reads it: the `session` part, and the mode of
    /// the process the answer's `effective` shows.
    fn session_access(engine: &mut Engine<Vec<u8>>, session: &str) -> (Value, Value) {
        engine
            .request(
                json!(60),
                "hoh/executor/access/get",
                json!({"executor":"claude","sessionId":session}),
            )
            .unwrap();
        let old = std::mem::replace(&mut engine.writer, FrameWriter::new(Vec::new()));
        let out = String::from_utf8(old.into_inner()).unwrap();
        let result =
            serde_json::from_str::<Value>(out.lines().last().unwrap()).unwrap()["result"].clone();
        (
            result["session"].clone(),
            result["effective"]["mode"].clone(),
        )
    }
    #[test]
    fn each_conversation_keeps_its_own_executor_model_and_mode() {
        let mut engine = plain_engine();
        engine.set_local_approvals(true);
        let set = |engine: &mut Engine<Vec<u8>>, executor: Value, mode: Option<&str>| {
            match mode {
                Some(mode) => engine.request(
                    json!(61),
                    "hoh/executor/access/set",
                    json!({"executor":"claude","mode":mode}),
                ),
                None => engine.request(
                    json!(62),
                    "hoh/executor/select",
                    json!({"executor":executor,"model":"opus"}),
                ),
            }
            .unwrap();
        };
        let prompt = |engine: &mut Engine<Vec<u8>>, session: &str| {
            engine
                .request(
                    json!(63),
                    "session/prompt",
                    json!({"sessionId":session,"contentBlocks":[{"type":"text","text":"hi"}]}),
                )
                .unwrap();
            // Confirm asks a question only when the stub is set to; answer nothing here.
            engine.pending = None;
            engine.claude_turn = None;
        };
        set(&mut engine, json!("claude"), None);
        prompt(&mut engine, "a");
        let (session, effective) = session_access(&mut engine, "a");
        assert_eq!(
            session,
            json!({"executor":"claude","model":"opus","privacy":"remote","mode":"confirm","running":"confirm","pending":false})
        );
        assert_eq!(effective, "confirm");

        // The choice is made between two turns: the next turn will run in the new mode, the live
        // process still runs in the old one.
        set(&mut engine, Value::Null, Some("full"));
        let (session, effective) = session_access(&mut engine, "a");
        assert_eq!(
            (
                session["mode"].clone(),
                session["running"].clone(),
                session["pending"].clone()
            ),
            (json!("full"), json!("confirm"), json!(true))
        );
        assert_eq!(effective, "confirm");
        prompt(&mut engine, "a");
        let (session, effective) = session_access(&mut engine, "a");
        assert_eq!(
            (
                session["mode"].clone(),
                session["running"].clone(),
                session["pending"].clone()
            ),
            (json!("full"), json!("full"), json!(false))
        );
        assert_eq!(effective, "full");

        // Back again.
        set(&mut engine, Value::Null, Some("confirm"));
        assert_eq!(session_access(&mut engine, "a").0["pending"], true);
        prompt(&mut engine, "a");
        assert_eq!(session_access(&mut engine, "a").0["running"], "confirm");

        // A second chat does not overwrite the first, and the first stays on Claude when new chats
        // return to the engine's own model.
        set(&mut engine, Value::Null, Some("full"));
        prompt(&mut engine, "b");
        assert_eq!(session_access(&mut engine, "b").0["running"], "full");
        assert_eq!(session_access(&mut engine, "a").0["running"], "confirm");
        engine
            .request(
                json!(64),
                "hoh/executor/select",
                json!({"executor":null,"model":"default"}),
            )
            .unwrap();
        let (session, _) = session_access(&mut engine, "a");
        assert_eq!(
            (
                session["executor"].clone(),
                session["privacy"].clone(),
                session["model"].clone()
            ),
            (json!("claude"), json!("remote"), json!("opus"))
        );
        // A conversation the stub does not know to be Claude's is not.
        assert_eq!(session_access(&mut engine, "c").0["executor"], Value::Null);
    }
    #[test]
    fn claude_executor_is_discovered_selected_and_runs_new_chats() {
        let mut engine = plain_engine();
        let file = std::env::temp_dir().join(format!("hoh-stub-claude-{}", std::process::id()));
        engine.claude_file = Some(file.clone());
        let discover = |engine: &mut Engine<Vec<u8>>, state: &str| {
            fs::write(&file, state).unwrap();
            engine
                .request(
                    json!(20),
                    "hoh/executor/discover",
                    json!({"executor":"claude"}),
                )
                .unwrap();
            let old = std::mem::replace(&mut engine.writer, FrameWriter::new(Vec::new()));
            let out = String::from_utf8(old.into_inner()).unwrap();
            serde_json::from_str::<Value>(out.lines().last().unwrap()).unwrap()["result"].clone()
        };
        let found = discover(&mut engine, "found");
        assert_eq!(
            (found["installed"].clone(), found["auth"].clone()),
            (json!(true), json!("authenticated"))
        );
        assert_eq!(found["models"][0]["id"], "default");
        let noauth = discover(&mut engine, "noauth");
        assert_eq!(
            (noauth["installed"].clone(), noauth["auth"].clone()),
            (json!(true), json!("unauthenticated"))
        );
        let missing = discover(&mut engine, "missing");
        assert_eq!(missing["installed"], json!(false));
        let unknown = discover(&mut engine, "unknown");
        assert_eq!(
            (unknown["versionStatus"].clone(), unknown["auth"].clone()),
            (json!("unknown"), json!("unknown"))
        );
        let unsupported = discover(&mut engine, "unsupported");
        assert_eq!(
            (
                unsupported["versionStatus"].clone(),
                unsupported["auth"].clone()
            ),
            (json!("unsupported"), json!("unknown"))
        );
        fs::remove_file(&file).unwrap();

        let select = json!({"executor":"claude","model":"opus"});
        assert!(
            engine
                .request(json!(21), "hoh/executor/select", select.clone())
                .is_err(),
            "privileged: no host, refused"
        );
        assert!(engine.selected.is_none());
        engine.set_local_approvals(true);
        assert!(
            engine
                .request(
                    json!(22),
                    "hoh/executor/select",
                    json!({"executor":"other","model":"x"})
                )
                .is_err()
        );
        engine
            .request(json!(23), "hoh/executor/select", select)
            .unwrap();
        assert_eq!(engine.selected.as_deref(), Some("opus"));

        engine.questions = true;
        let prompt = |engine: &mut Engine<Vec<u8>>, session: &str| {
            engine.request(
                json!(30),
                "session/prompt",
                json!({"sessionId":session,"contentBlocks":[{"type":"text","text":"привет"}]}),
            )
        };
        prompt(&mut engine, "s1").unwrap();
        assert!(
            engine.pending.is_some() && engine.claude_turn.is_some(),
            "confirm: the turn waits for a question"
        );
        assert_eq!(engine.claude_calls[0]["mode"], "confirm");
        engine.pending = None;
        engine.claude_turn = None;
        engine.access_mode = "full";
        prompt(&mut engine, "s2").unwrap();
        assert!(
            engine.pending.is_none() && engine.claude_turn.is_none(),
            "full: no question"
        );
        assert_eq!(engine.claude_calls[1]["model"], "opus");

        engine
            .request(
                json!(40),
                "hoh/executor/select",
                json!({"executor":null,"model":"default"}),
            )
            .unwrap();
        assert!(engine.selected.is_none());
    }
    #[test]
    fn approval_texts_name_the_executor_and_tell_full_from_confirm() {
        let engine = plain_engine();
        let text = |method: &str, params: Value| engine.approval_text(method, &params).unwrap();
        let full = text(
            "hoh/executor/access/set",
            json!({"executor":"claude","mode":"full"}),
        );
        let confirm = text(
            "hoh/executor/access/set",
            json!({"executor":"claude","mode":"confirm"}),
        );
        assert_ne!(full, confirm);
        assert!(
            full.starts_with("Включить полный доступ для Claude?")
                && full.contains("Сделанное нельзя отменить.")
        );
        assert_eq!(confirm, "Вернуть режим «Спрашивать разрешение» для Claude?");
        assert_eq!(
            text(
                "hoh/executor/select",
                json!({"executor":"claude","model":"opus"})
            ),
            "Отвечать будет Claude (модель opus)?"
        );
        assert_eq!(
            text(
                "hoh/executor/select",
                json!({"executor":null,"model":"default"})
            ),
            "Отвечать будет модель на этом компьютере?"
        );
    }
    #[test]
    fn claude_access_mode_defaults_to_confirm_and_changes_only_through_set() {
        let mut engine = plain_engine();
        let get = |engine: &mut Engine<Vec<u8>>| {
            engine
                .request(
                    json!(9),
                    "hoh/executor/access/get",
                    json!({"executor":"claude"}),
                )
                .unwrap();
            engine.access_mode
        };
        assert_eq!(get(&mut engine), "confirm");
        assert!(
            engine
                .request(
                    json!(10),
                    "hoh/executor/access/set",
                    json!({"executor":"claude","mode":"full"})
                )
                .is_err(),
            "no host, no local approvals: refused"
        );
        assert_eq!(get(&mut engine), "confirm");
        engine.set_local_approvals(true);
        assert!(
            engine
                .request(
                    json!(11),
                    "hoh/executor/access/set",
                    json!({"executor":"claude","mode":"root"})
                )
                .is_err()
        );
        assert!(
            engine
                .request(
                    json!(12),
                    "hoh/executor/access/set",
                    json!({"executor":"other","mode":"full"})
                )
                .is_err()
        );
        engine
            .request(
                json!(13),
                "hoh/executor/access/set",
                json!({"executor":"claude","mode":"full"}),
            )
            .unwrap();
        assert_eq!(get(&mut engine), "full");
        engine
            .request(
                json!(14),
                "hoh/executor/access/set",
                json!({"executor":"claude","mode":"confirm"}),
            )
            .unwrap();
        assert_eq!(get(&mut engine), "confirm");
    }
    #[test]
    fn without_a_host_privileged_methods_are_refused_unless_local_approvals_are_on() {
        let mut engine = plain_engine();
        let key = json!({"apiKey":"k","baseUrl":"https://api.example.test/v1"});
        let probe = json!({"baseUrl":"https://api.example.test/v1","model":"local"});
        assert_eq!(
            engine
                .request(json!(2), "hoh/model/key/set", key.clone())
                .unwrap_err(),
            "host approval required"
        );
        assert!(
            engine
                .request(json!(3), "hoh/model/probe", probe.clone())
                .is_err()
        );
        engine.set_local_approvals(true);
        assert!(!engine.credentials.probe_authenticated().unwrap());
        engine.request(json!(4), "hoh/model/key/set", key).unwrap();
        assert!(
            engine.credentials.probe_authenticated().unwrap(),
            "the key is stored once applied"
        );
        engine.request(json!(5), "hoh/model/probe", probe).unwrap();
    }
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
