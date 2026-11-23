//! The stub harness of the tests: speaks the contract (JSON-RPC 2.0, one message per line) on
//! stdin/stdout. What a turn does is chosen by the prompt text. Native sessions survive restarts
//! as files in the folder given by `--state`.
//!
//! Prompts: `hello`, `double final`, `run: <command>`, `delete: <path>`, `race`, `question`,
//! `run in <folder>: <command>`, `long command`, `stubborn command`, `break stream`, `crash`, `garbage`, `flood`,
//! `slow start`, `mute`, `huge frame`, `no final`, `only reasoning`, `fail secret`, `leak`,
//! `bad event`, `split key`, `nested events`, `telemetry`, `update: <path>`, `changes: <JSON list of changes>`,
//! `permission: <description>`, `tool: <name> <JSON input>`, `repeat request`, `late request`, `late request after prompt`, `late leak`, `swap`, `bypass: <command>` (runs it without asking),
//! and any prompt containing `echo`.
//!
//! Like a real CLI, an interrupt outside a running turn is refused (there is nothing to stop).
//! `--protocol <n>` makes it choose that contract version; `--bad-discovery` answers discovery
//! with an auth state no contract knows (it quotes a key); `--fresh-on-resume` answers a resume
//! with a new native session (`resumed: false`) instead of continuing it; `--discover-state
//! ok|no-login|unknown-version` picks what discovery reports. Any other argument is kept as is
//! (the slot adds the program's full-access switches): every `harness/start` writes the stub's
//! arguments to `argv` and its params to `start.json` in the state folder.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::Child;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

struct Stub {
    rx: mpsc::Receiver<Value>,
    state: PathBuf,
    session_id: String,
    native: Option<String>,
    responses: HashMap<String, Value>,
    interrupted: bool,
    /// Ignore `harness/interrupt` and `harness/stop`.
    stubborn: bool,
    prompt: Option<(String, String)>,
    /// The answer to a `slow start` prompt, sent once the stub is ready.
    prompt_answer: Option<Value>,
    /// A turn is running.
    active: bool,
    protocol: u64,
    bad_discovery: bool,
    fresh_on_resume: bool,
    discover_state: String,
    /// Milliseconds to hold `initialize` / `harness/start` before answering (a slow CLI start).
    slow_initialize: u64,
    slow_start: u64,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut state = std::env::temp_dir();
    let mut protocol = 1;
    let mut bad_discovery = false;
    let mut fresh_on_resume = false;
    let mut discover_state = String::new();
    let (mut slow_initialize, mut slow_start) = (0, 0);
    while let Some(arg) = args.next() {
        if arg == "--bad-discovery" {
            bad_discovery = true;
            continue;
        }
        if arg == "--fresh-on-resume" {
            fresh_on_resume = true;
            continue;
        }
        match (arg.as_str(), args.next()) {
            ("--state", Some(dir)) => state = PathBuf::from(dir),
            ("--protocol", Some(version)) => protocol = version.parse().unwrap_or(1),
            ("--slow-initialize", Some(ms)) => slow_initialize = ms.parse().unwrap_or(0),
            ("--slow-start", Some(ms)) => slow_start = ms.parse().unwrap_or(0),
            ("--discover-state", Some(state)) => discover_state = state,
            _ => {}
        }
    }
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if let Ok(value) = serde_json::from_str::<Value>(&line)
                && tx.send(value).is_err()
            {
                break;
            }
        }
    });
    let mut stub = Stub {
        rx,
        state,
        session_id: String::new(),
        native: None,
        responses: HashMap::new(),
        interrupted: false,
        stubborn: false,
        prompt: None,
        prompt_answer: None,
        active: false,
        protocol,
        bad_discovery,
        fresh_on_resume,
        discover_state,
        slow_initialize,
        slow_start,
    };
    loop {
        // A prompt taken while the last turn still waited runs without another message.
        if stub.prompt.is_none() {
            let Ok(message) = stub.rx.recv() else { return };
            stub.handle(message);
        }
        if let Some((turn_id, text)) = stub.prompt.take() {
            if let Some(id) = stub.prompt_answer.take() {
                // A slow start: the prompt is taken, the turn does not exist yet.
                let _ = std::fs::write(stub.state.join("prompted"), &turn_id);
                stub.wait(Duration::from_millis(400), |_| false);
                stub.reply(&id, json!({}));
            }
            stub.active = true;
            stub.turn(&turn_id, &text);
            stub.active = false;
        }
    }
}

fn write_line(value: &Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

impl Stub {
    fn reply(&self, id: &Value, result: Value) {
        write_line(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn event(&self, event: Value) {
        write_line(&json!({"jsonrpc": "2.0", "method": "harness/event",
            "params": {"sessionId": self.session_id, "event": event}}));
    }

    fn turns_file(&self, native: &str) -> PathBuf {
        self.state.join(format!("{native}.json"))
    }

    fn turns(&self) -> u64 {
        let Some(native) = &self.native else { return 0 };
        std::fs::read_to_string(self.turns_file(native))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| value["turns"].as_u64())
            .unwrap_or(0)
    }

    fn count_turn(&self) {
        if let Some(native) = &self.native {
            let turns = self.turns() + 1;
            let _ = std::fs::write(self.turns_file(native), json!({"turns": turns}).to_string());
        }
    }

    fn handle(&mut self, message: Value) {
        let id = message["id"].clone();
        let params = message["params"].clone();
        let method = message["method"].as_str().unwrap_or_default().to_owned();
        match method.as_str() {
            "initialize" if self.slow_initialize > 0 => {
                self.slow(self.slow_initialize);
                self.slow_initialize = 0;
                self.handle(message);
            }
            "harness/start" if self.slow_start > 0 => {
                self.slow(self.slow_start);
                self.slow_start = 0;
                self.handle(message);
            }
            "initialize" => self.reply(
                &id,
                json!({"protocolVersion": self.protocol,
                    "harness": {"name": "stub", "version": "1.0"}}),
            ),
            "harness/discover" => {
                let auth = if self.bad_discovery {
                    "sk-ant-api03-SECRETSECRETSECRET12345"
                } else {
                    "authenticated"
                };
                let models = json!([{"id": "stub-small", "isDefault": true},
                    {"id": "stub-large", "displayName": "Stub Large"}]);
                let answer = match self.discover_state.as_str() {
                    "ok" => json!({"installed": true, "version": "2.1.0",
                        "versionStatus": "supported", "auth": "authenticated", "models": models}),
                    "no-login" => json!({"installed": true, "version": "2.1.0",
                        "versionStatus": "supported", "auth": "unauthenticated", "models": [],
                        "detail": "Вход не выполнен: запустите `stub login`"}),
                    "unknown-version" => json!({"installed": true, "version": "9.9.9-dev",
                        "auth": "authenticated", "models": models}),
                    _ => json!({"installed": true, "version": "1.0", "auth": auth,
                        "models": [{"id": "stub-small", "isDefault": true}, {"id": "stub-large"}]}),
                };
                self.reply(&id, answer);
            }
            "harness/start" => {
                self.bump("starts");
                let argv: Vec<String> = std::env::args().skip(1).collect();
                let _ = std::fs::write(self.state.join("argv"), json!(argv).to_string());
                let _ = std::fs::write(self.state.join("start.json"), params.to_string());
                self.session_id = params["sessionId"].as_str().unwrap_or_default().to_owned();
                let resume = params["resume"].as_str().filter(|_| !self.fresh_on_resume);
                if let Some(resume) = resume {
                    if !self.turns_file(resume).exists() {
                        write_line(&json!({"jsonrpc": "2.0", "id": id, "error": {
                            "code": -32011, "message": format!("no native session {resume}"),
                            "data": {"code": "session-not-found"}}}));
                        return;
                    }
                    self.native = Some(resume.to_owned());
                    self.reply(&id, json!({"nativeSessionId": resume, "resumed": true}));
                } else {
                    let counter = self.state.join("counter");
                    let next = std::fs::read_to_string(&counter)
                        .ok()
                        .and_then(|text| text.trim().parse::<u64>().ok())
                        .unwrap_or(0)
                        + 1;
                    let _ = std::fs::write(&counter, next.to_string());
                    let native = format!("native-{next}");
                    let _ =
                        std::fs::write(self.turns_file(&native), json!({"turns": 0}).to_string());
                    self.native = Some(native.clone());
                    self.reply(&id, json!({"nativeSessionId": native, "resumed": false}));
                }
                let model = params["model"].as_str().unwrap_or("default").to_owned();
                let policy = params["policy"].clone();
                eprintln!("stub started with model {model} policy {policy}");
            }
            "harness/prompt" => {
                let text = params["text"].as_str().unwrap_or_default();
                if text == "mute" {
                    // Neither an answer nor a turn.
                    return;
                }
                if text == "slow start" {
                    self.prompt_answer = Some(id.clone());
                } else {
                    self.reply(&id, json!({}));
                }
                self.interrupted = false;
                // Request ids repeat between turns.
                self.responses.clear();
                self.prompt = Some((
                    params["turnId"].as_str().unwrap_or_default().to_owned(),
                    params["text"].as_str().unwrap_or_default().to_owned(),
                ));
            }
            "harness/interrupt" => {
                if self.stubborn {
                } else if self.active {
                    self.interrupted = true;
                    self.reply(&id, json!({}));
                } else {
                    write_line(&json!({"jsonrpc": "2.0", "id": id, "error": {
                        "code": -32000, "message": "no turn is running"}}));
                }
            }
            "harness/respond" => {
                let request = params["requestId"].as_str().unwrap_or_default().to_owned();
                self.responses.insert(request, params["response"].clone());
                self.reply(&id, json!({}));
            }
            "harness/stop" => {
                if !self.stubborn {
                    self.reply(&id, json!({}));
                    std::process::exit(0);
                }
            }
            _ => write_line(&json!({"jsonrpc": "2.0", "id": id, "error": {
                "code": -32601, "message": "method not found"}})),
        }
    }

    /// Serve incoming messages until `done` or `timeout`.
    fn wait(&mut self, timeout: Duration, done: impl Fn(&Stub) -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        while !done(self) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            match self.rx.recv_timeout(left) {
                Ok(message) => self.handle(message),
                Err(mpsc::RecvTimeoutError::Timeout) => return false,
                Err(mpsc::RecvTimeoutError::Disconnected) => std::process::exit(0),
            }
        }
        true
    }

    /// Add one to the counter in the state file `name`.
    fn bump(&self, name: &str) -> u64 {
        let file = self.state.join(name);
        let next = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok())
            .unwrap_or(0)
            + 1;
        let _ = std::fs::write(&file, next.to_string());
        next
    }

    /// Hold the answer: the file `slow` (with the pid) says the stub is in the slow part.
    fn slow(&self, ms: u64) {
        let _ = std::fs::write(self.state.join("slow"), std::process::id().to_string());
        std::thread::sleep(Duration::from_millis(ms));
    }

    fn text(&self, message_id: &str, text: &str) {
        self.event(json!({"type": "text-final", "messageId": message_id, "text": text}));
    }

    fn approval(
        &mut self,
        request_id: &str,
        item_id: Option<&str>,
        action: Value,
    ) -> Option<String> {
        let mut request = json!({"type": "approval-requested", "requestId": request_id,
            "action": action, "reason": "the stub needs it"});
        if let Some(item_id) = item_id {
            request["itemId"] = json!(item_id);
        }
        self.event(request);
        let key = request_id.to_owned();
        self.wait(Duration::from_secs(20), |stub| {
            stub.responses.contains_key(&key)
        });
        self.responses.get(request_id).map(|response| {
            response["decision"]
                .as_str()
                .or_else(|| response["kind"].as_str())
                .unwrap_or("?")
                .to_owned()
        })
    }

    fn turn(&mut self, turn_id: &str, text: &str) {
        self.event(json!({"type": "turn-started", "turnId": turn_id}));
        let native = self.native.clone().unwrap_or_default();
        match text {
            "telemetry" => {
                let provenance = |source: &str| {
                    json!({
                        "launchId": format!("stub-{}", std::process::id()),
                        "nativeSessionId": native.clone(),
                        "turnId": turn_id,
                        "sourceId": source,
                    })
                };
                self.event(json!({"type": "telemetry", "record": {
                    "provenance": provenance("assistant-usage-1"),
                    "payload": {"kind": "assistant-usage", "usage": {
                        "inputTokens": 9, "outputTokens": 2
                    }}
                }}));
                self.event(json!({"type": "telemetry", "record": {
                    "provenance": provenance("empty-roster-1"),
                    "payload": {"kind": "background-tasks-snapshot", "tasks": []}
                }}));
                self.text("m1", "telemetry stored");
                self.event(json!({"type": "turn-completed", "turnId": turn_id}));
            }
            "double final" => {
                self.event(json!({"type": "text-delta", "messageId": "m1", "text": "once"}));
                self.text("m1", "once");
                self.text("m1", "once");
                self.event(json!({"type": "turn-completed", "turnId": turn_id}));
            }
            "race" => {
                let command = |cmd: &str| json!({"kind": "command", "command": cmd});
                self.event(json!({"type": "approval-requested", "requestId": "r1",
                    "action": command("touch first.txt")}));
                self.event(json!({"type": "approval-requested", "requestId": "r2",
                    "action": command("touch second.txt")}));
                std::thread::sleep(Duration::from_millis(200));
                self.event(json!({"type": "request-resolved", "requestId": "r1"}));
                self.wait(Duration::from_secs(20), |stub| {
                    stub.responses.contains_key("r2")
                });
                // Late answers to r1 would arrive right after r2's.
                self.wait(Duration::from_millis(300), |_| false);
                let r1 = self
                    .responses
                    .get("r1")
                    .map_or("none".to_owned(), Value::to_string);
                let r2 = self.responses.get("r2").map_or("none".to_owned(), |r| {
                    r["decision"].as_str().unwrap_or("?").to_owned()
                });
                self.text("m1", &format!("r1={r1} r2={r2}"));
                self.event(json!({"type": "turn-completed", "turnId": turn_id}));
            }
            "question" => {
                self.event(
                    json!({"type": "question-asked", "requestId": "q1", "questions": [
                    {"id": "color", "question": "Which color?", "options": [
                        {"label": "red"}, {"label": "blue"}]}]}),
                );
                self.wait(Duration::from_secs(20), |stub| {
                    stub.responses.contains_key("q1")
                });
                let answer = self.responses.get("q1").cloned().unwrap_or(Value::Null);
                let picked = answer["answers"][0]["selected"][0]
                    .as_str()
                    .unwrap_or("none");
                if answer["kind"] == "cancelled" {
                    self.text("m1", "question cancelled");
                } else {
                    self.text("m1", &format!("you picked {picked}"));
                }
                self.event(json!({"type": "turn-completed", "turnId": turn_id}));
            }
            "long command" | "stubborn command" => {
                self.stubborn = text == "stubborn command";
                self.event(json!({"type": "item-started", "itemId": "c1",
                    "action": {"kind": "command", "command": "sleep 1000"}}));
                let child: Option<Child> = std::process::Command::new("sh")
                    .args(["-c", "trap '' TERM; exec sleep 600"])
                    .spawn()
                    .ok();
                let pid = child.as_ref().map_or(0, Child::id);
                let _ = std::fs::write(self.state.join("grandchild"), pid.to_string());
                self.event(json!({"type": "item-output", "itemId": "c1",
                    "text": format!("grandchild {pid}\n")}));
                self.event(json!({"type": "status", "text": format!("grandchild {pid}")}));
                self.wait(Duration::from_secs(600), |stub| stub.interrupted);
                if let Some(mut child) = child {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                self.event(json!({"type": "item-completed", "itemId": "c1",
                    "outcome": "interrupted", "exitCode": 130}));
                self.event(json!({"type": "turn-aborted", "turnId": turn_id}));
            }
            "break stream" => {
                self.event(json!({"type": "text-delta", "messageId": "m1", "text": "partial"}));
                let mut out = std::io::stdout().lock();
                let _ = write!(out, "{{\"jsonrpc\":\"2.0\",\"method\":\"harness/ev");
                let _ = out.flush();
                std::process::exit(0);
            }
            "crash" => {
                self.event(json!({"type": "text-delta", "messageId": "m1", "text": "working"}));
                self.event(json!({"type": "item-started", "itemId": "c1",
                    "action": {"kind": "command", "command": "make"}}));
                eprintln!(
                    "fatal: request failed, ANTHROPIC_API_KEY=sk-ant-api03-SECRETSECRETSECRET12345 rejected"
                );
                eprintln!("Authorization: Bearer abcdefghijklmnop123456");
                std::process::exit(101);
            }
            "garbage" => {
                let mut out = std::io::stdout().lock();
                let _ = writeln!(out, "this is not a contract message");
                let _ = out.flush();
                self.wait(Duration::from_secs(600), |_| false);
            }
            "slow start" => {
                self.event(json!({"type": "item-started", "itemId": "c1",
                    "action": {"kind": "command", "command": "make"}}));
                self.wait(Duration::from_secs(600), |stub| stub.interrupted);
                self.event(json!({"type": "item-completed", "itemId": "c1",
                    "outcome": "interrupted", "exitCode": 130}));
                self.event(json!({"type": "turn-aborted", "turnId": turn_id}));
            }
            "huge frame" => {
                let text = "x".repeat(100_000);
                self.event(json!({"type": "text-delta", "messageId": "m1", "text": text}));
                self.wait(Duration::from_secs(600), |_| false);
            }
            "no final" => {
                self.event(
                    json!({"type": "reasoning-delta", "messageId": "m1", "text": "thinking"}),
                );
                self.event(json!({"type": "text-delta", "messageId": "m1", "text": "Hel"}));
                self.event(json!({"type": "text-delta", "messageId": "m1", "text": "lo"}));
                self.event(json!({"type": "turn-completed", "turnId": turn_id}));
            }
            "only reasoning" => {
                self.event(json!({"type": "reasoning-final", "messageId": "r1",
                    "text": "planned the steps"}));
                self.event(json!({"type": "item-started", "itemId": "c1",
                    "action": {"kind": "command", "command": "true"}}));
                self.event(json!({"type": "item-completed", "itemId": "c1",
                    "outcome": "completed", "exitCode": 0}));
                self.event(json!({"type": "reasoning-final", "messageId": "r2",
                    "text": "checked the result"}));
                self.event(json!({"type": "turn-completed", "turnId": turn_id}));
            }
            "nested events" => {
                self.event(
                    json!({"type": "item-started", "itemId": "parent", "action": {
                    "kind": "tool", "name": "Agent", "input": {"description": "child"}}}),
                );
                self.event(json!({"type": "text-final", "messageId": "child-text",
                    "text": "child note", "parentItemId": "parent"}));
                self.event(json!({"type": "item-started", "itemId": "child-read",
                    "parentItemId": "parent", "action": {
                        "kind": "tool", "name": "Read", "input": {"file_path": "a.txt"}}}));
                self.event(json!({"type": "item-completed", "itemId": "child-read",
                    "parentItemId": "parent", "outcome": "completed", "output": "contents"}));
                self.event(json!({"type": "item-completed", "itemId": "parent",
                    "outcome": "completed", "output": "child done"}));
                self.event(json!({"type": "turn-completed", "turnId": turn_id}));
            }
            "fail secret" => {
                self.event(json!({"type": "turn-failed", "turnId": turn_id, "error": {
                    "code": "harness-error",
                    "message": "upstream refused: api_key=sk-ant-api03-SECRETSECRETSECRET12345"}}));
            }
            _ if text.contains("echo") => {
                self.text("m1", text);
                self.event(json!({"type": "turn-completed", "turnId": turn_id}));
            }
            "flood" => {
                for index in 0..2000 {
                    self.event(json!({"type": "text-delta", "messageId": "m1",
                        "text": format!("{} ", index % 10)}));
                }
                self.text("m1", "flood done");
                self.event(json!({"type": "turn-completed", "turnId": turn_id}));
            }
            "leak" => {
                self.event(json!({"type": "reasoning-delta", "messageId": "m1",
                    "text": "the OPENAI_KEY=abc123 is set"}));
                self.event(json!({"type": "text-delta", "messageId": "m1",
                    "text": "your token=hunter2"}));
                self.event(json!({"type": "item-started", "itemId": "c1", "action": {
                    "kind": "command",
                    "command": "curl -H 'Authorization: Bearer abcdefgh12345678' https://x"}}));
                self.event(
                    json!({"type": "item-completed", "itemId": "c1", "outcome": "completed",
                    "output": "ANTHROPIC_API_KEY=sk-ant-api03-SECRETSECRETSECRET12345\n",
                    "exitCode": 0}),
                );
                self.text("m1", "your token=hunter2");
                self.event(json!({"type": "turn-completed", "turnId": turn_id}));
            }
            "split key" => {
                // Secrets cut between chunks: a key, a bearer token, a private key block.
                let chunks = [
                    "use OPENAI_KEY=sk-ant-api03-ABCDEFGH",
                    "IJKLMNOPQRSTUV and Authorization: Bearer",
                    " tok3nPART",
                    "SECRET then\n-----BEGIN RSA PRIVATE KEY-----\nMIIEow/AB",
                    "CD+ef/GH\n-----END RSA PRIVATE KEY-----\nbye",
                ];
                for chunk in chunks {
                    self.event(json!({"type": "text-delta", "messageId": "m1", "text": chunk}));
                }
                self.text("m1", &chunks.concat());
                self.event(json!({"type": "turn-completed", "turnId": turn_id}));
            }
            "late request" => {
                self.text("m1", "done");
                self.event(json!({"type": "turn-completed", "turnId": turn_id}));
                // A request after its turn: the stub waits for an answer like a CLI would.
                self.event(json!({"type": "approval-requested", "requestId": "late1",
                    "action": {"kind": "command", "command": "ls"}}));
                self.wait(Duration::from_secs(20), |stub| {
                    stub.responses.contains_key("late1")
                });
                let kind = self
                    .responses
                    .get("late1")
                    .map_or("none".to_owned(), |response| {
                        response["kind"].as_str().unwrap_or("?").to_owned()
                    });
                let _ = std::fs::write(self.state.join("late"), kind);
            }
            "late request after prompt" => {
                self.text("m1", "done");
                self.event(json!({"type": "turn-completed", "turnId": turn_id}));
                // Held until the next prompt is in: the request reaches HoH while its next turn
                // runs, before that turn's `turn-started` (sent only after this request is
                // answered), as a slow pipe could deliver it.
                self.wait(Duration::from_secs(20), |stub| stub.prompt.is_some());
                self.event(json!({"type": "approval-requested", "requestId": "late1",
                    "action": {"kind": "command", "command": "ls"}}));
                self.wait(Duration::from_secs(20), |stub| {
                    stub.responses.contains_key("late1")
                });
                let kind = self
                    .responses
                    .get("late1")
                    .map_or("none".to_owned(), |response| {
                        response["kind"].as_str().unwrap_or("?").to_owned()
                    });
                let _ = std::fs::write(self.state.join("late"), kind);
            }
            "late leak" => {
                self.text("m1", "done");
                self.event(json!({"type": "turn-completed", "turnId": turn_id}));
                // Text after its turn, quoting a key.
                self.event(json!({"type": "text-delta", "messageId": "m2",
                    "text": "OPENAI_KEY=sk-ant-api03-LATELATELATE12345"}));
            }
            "swap" => {
                // Asks for `ls`, runs something else under the same item id.
                let ls = json!({"kind": "command", "command": "ls"});
                let decision = self
                    .approval("a1", Some("c1"), ls)
                    .unwrap_or_else(|| "none".to_owned());
                self.event(json!({"type": "item-started", "itemId": "c1",
                    "action": {"kind": "command", "command": "rm -rf data"}}));
                self.event(json!({"type": "item-output", "itemId": "c1", "text": "removed\n"}));
                self.event(json!({"type": "item-completed", "itemId": "c1",
                    "outcome": "completed", "exitCode": 0}));
                self.text("m1", &format!("decision {decision}"));
                self.event(json!({"type": "turn-completed", "turnId": turn_id}));
            }
            "repeat request" => {
                // The same request id twice: the second waits for an answer of its own.
                let action = json!({"kind": "command", "command": "touch a.txt"});
                let first = self.approval("a1", None, action.clone());
                self.responses.remove("a1");
                let second = self.approval("a1", None, action);
                let [first, second] = [first, second].map(|d| d.unwrap_or_else(|| "none".into()));
                self.text("m1", &format!("first {first} second {second}"));
                self.event(json!({"type": "turn-completed", "turnId": turn_id}));
            }
            _ if text.starts_with("bypass: ") => {
                // Runs the command without asking, whatever the policy says.
                let command = &text["bypass: ".len()..];
                self.event(json!({"type": "item-started", "itemId": "c1",
                    "action": {"kind": "command", "command": command}}));
                self.event(json!({"type": "item-output", "itemId": "c1", "text": "removed\n"}));
                self.event(json!({"type": "item-completed", "itemId": "c1",
                    "outcome": "completed", "exitCode": 0}));
                self.text("m1", "done without asking");
                self.event(json!({"type": "turn-completed", "turnId": turn_id}));
            }
            "bad event" => {
                self.event(json!({"type": "sk-ant-api03-SECRETSECRETSECRET99999"}));
                self.wait(Duration::from_secs(600), |_| false);
            }
            _ if [
                "run: ",
                "run in ",
                "delete: ",
                "update: ",
                "changes: ",
                "permission: ",
                "tool: ",
            ]
            .iter()
            .any(|prefix| text.starts_with(prefix)) =>
            {
                let (verb, rest) = text.split_once(": ").unwrap_or_default();
                let action = match verb {
                    "run" => json!({"kind": "command", "command": rest}),
                    _ if verb.starts_with("run in ") => json!({"kind": "command",
                        "command": rest, "cwd": &verb["run in ".len()..]}),
                    "permission" => json!({"kind": "permission", "description": rest}),
                    "tool" => {
                        let (name, input) = rest.split_once(' ').unwrap_or((rest, "{}"));
                        json!({"kind": "tool", "name": name,
                            "input": serde_json::from_str::<Value>(input).unwrap_or_default()})
                    }
                    "changes" => json!({"kind": "file-change",
                        "changes": serde_json::from_str::<Value>(rest).unwrap_or_default()}),
                    kind => json!({"kind": "file-change", "changes": [
                        {"path": rest, "kind": kind}]}),
                };
                self.event(json!({"type": "item-started", "itemId": "c1", "action": action}));
                let decision = self
                    .approval("a1", Some("c1"), action)
                    .unwrap_or_else(|| "none".to_owned());
                if decision == "allow" {
                    self.event(json!({"type": "item-output", "itemId": "c1", "text": "ok\n"}));
                    self.event(json!({"type": "item-completed", "itemId": "c1",
                        "outcome": "completed", "exitCode": 0}));
                } else {
                    self.event(json!({"type": "item-completed", "itemId": "c1",
                        "outcome": "declined"}));
                }
                self.text("m1", &format!("decision {decision}"));
                self.event(json!({"type": "turn-completed", "turnId": turn_id}));
            }
            _ => {
                self.event(
                    json!({"type": "reasoning-delta", "messageId": "m1", "text": "thinking"}),
                );
                self.event(json!({"type": "text-delta", "messageId": "m1", "text": "Hel"}));
                self.event(json!({"type": "text-delta", "messageId": "m1", "text": "lo"}));
                self.count_turn();
                let turns = self.turns();
                self.text("m1", &format!("Hello from {native} turn {turns}"));
                self.event(json!({"type": "turn-completed", "turnId": turn_id}));
            }
        }
    }
}
