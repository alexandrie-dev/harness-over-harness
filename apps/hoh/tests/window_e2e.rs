//! End-to-end milestone scenario of the window engine: the built `hoh` binary with the window
//! profile, driven over stdio JSON-RPC exactly as the Tauri core drives it, with the scenario
//! mock (`hoh-llm-openai-mock`) as the OpenAI-compatible model.
//!
//! Contract sources: `docs/window.md` §4, `hoh-protocol` (types, `PRIVILEGED_METHODS`),
//! `apps/hoh-desktop/src/{engine,approval}.rs` (handshake and approval MAC).
//!
//! Tests marked `#[ignore]` are red for a known engine defect named in their reason; they are
//! the acceptance checks for that fix. Run them with
//! `cargo test -p hoh --test window_e2e -- --ignored`.

use hoh_llm_openai_mock::{
    MockOptions, MockServer, Scenario, ScenarioResponse, ScenarioToolCall, ScenarioUsage,
};
use hoh_protocol::{HostApprovalRequest, approval};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines};

const MODEL: &str = "window-mock";
const API_KEY: &str = "sk-window-e2e-0123456789abcdef";
const FRAME_TIMEOUT: Duration = Duration::from_secs(60);
const SESSION: &str = "window-main";

fn approval_key() -> [u8; 32] {
    std::array::from_fn(|index| u8::try_from(index).expect("index fits a byte") + 1)
}

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_millis(),
    )
    .expect("milliseconds fit u64")
}

// ---------------------------------------------------------------- scenario construction

fn text(parts: &[&str]) -> ScenarioResponse {
    ScenarioResponse {
        text: parts.iter().map(|part| (*part).to_owned()).collect(),
        tool_calls: vec![],
        usage: ScenarioUsage {
            prompt_tokens: 10,
            completion_tokens: 4,
        },
    }
}

fn call(id: &str, name: &str, arguments: Value) -> ScenarioResponse {
    ScenarioResponse {
        text: vec![],
        tool_calls: vec![ScenarioToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments,
        }],
        usage: ScenarioUsage {
            prompt_tokens: 20,
            completion_tokens: 6,
        },
    }
}

fn dangerous_bash(id: &str, directory: &Path) -> ScenarioResponse {
    call(
        id,
        "bash",
        json!({
            "command": format!("rm -rf '{}'", directory.display()),
            "description": "Remove the generated folder",
            "sandbox_permissions": "danger-full-access",
            "justification": "Удалить папку со сгенерированными файлами.",
        }),
    )
}

async fn start_mock(responses: Vec<ScenarioResponse>) -> MockServer {
    start_mock_with_delays(responses, &[]).await
}

/// `delays`: (response index, pause in ms before its first delta).
async fn start_mock_with_delays(
    responses: Vec<ScenarioResponse>,
    delays: &[(usize, u64)],
) -> MockServer {
    let mut options = MockOptions::new(Scenario {
        model: MODEL.to_owned(),
        responses,
    });
    options.api_key = Some(API_KEY.to_owned());
    options.delays_ms.extend(delays.iter().copied());
    MockServer::start(options).await.expect("start model mock")
}

// ---------------------------------------------------------------- workspace and profile

struct Workspace {
    root: tempfile::TempDir,
    /// Canonical working folder of the session.
    folder: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temporary root");
        let folder = root.path().join("workspace");
        std::fs::create_dir_all(&folder).expect("workspace folder");
        let folder = folder.canonicalize().expect("canonical workspace");
        std::fs::write(folder.join("note.txt"), "заметка окна\n").expect("note");
        std::fs::create_dir_all(folder.join("out")).expect("out folder");
        std::fs::create_dir_all(folder.join("generated")).expect("generated folder");
        std::fs::write(folder.join("generated").join("leftover.txt"), "лишнее\n")
            .expect("generated file");
        // A first run: the profile has no model address worth using, the window sets it.
        let profile = root.path().join("home").join("profiles").join("window");
        std::fs::create_dir_all(&profile).expect("profile folder");
        std::fs::write(
            profile.join("package.json"),
            json!({
                "name": "dsh-profile-window",
                "private": true,
                "dependencies": {},
                "dsh": {"profile": {"bundles": ["@hoh/window"]}}
            })
            .to_string(),
        )
        .expect("profile package");
        std::fs::write(
            profile.join("cordis.patch.yml"),
            "- id: model-host\n  config:\n    baseUrl: http://127.0.0.1:1/v1\n    model: unset",
        )
        .expect("profile patch");
        // The credentials store refuses a harness home that other users can enter.
        std::fs::set_permissions(
            root.path().join("home"),
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .expect("private harness home");
        Self { root, folder }
    }

    fn home(&self) -> PathBuf {
        self.root.path().join("home")
    }
}

// ---------------------------------------------------------------- host side of the protocol

#[derive(Clone, Copy, Debug)]
enum Verdict {
    /// Valid MAC over `allowed-once`.
    Allow,
    /// Valid MAC over `rejected`.
    Reject,
    /// Claims `allowed-once` with a MAC computed for `rejected`: outcome substitution.
    Forge,
}

struct PlannedApproval {
    tool_name: &'static str,
    verdict: Verdict,
}

struct Resolved {
    request: HostApprovalRequest,
}

struct Host {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: Lines<BufReader<tokio::process::ChildStdout>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    key: [u8; 32],
    next_id: u64,
    /// Every frame the engine wrote, in order.
    frames: Vec<Value>,
    responses: HashMap<String, Value>,
    approval_plan: VecDeque<PlannedApproval>,
    resolved: Vec<Resolved>,
    answers: VecDeque<Value>,
    questions: Vec<Value>,
}

impl Host {
    async fn start(workspace: &Workspace, key: [u8; 32]) -> Self {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_hoh"));
        command
            .args(["--profile", "window"])
            .env_clear()
            .env("HOME", workspace.root.path())
            .env("DSH_HOME", workspace.home())
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .current_dir(&workspace.folder)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().expect("spawn hoh");
        let stdin = child.stdin.take().expect("engine stdin");
        let stdout = BufReader::new(child.stdout.take().expect("engine stdout")).lines();
        let mut stderr_pipe = child.stderr.take().expect("engine stderr");
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&stderr);
        tokio::spawn(async move {
            let mut chunk = [0_u8; 4096];
            while let Ok(count) = stderr_pipe.read(&mut chunk).await {
                if count == 0 {
                    break;
                }
                sink.lock()
                    .expect("stderr buffer")
                    .extend_from_slice(&chunk[..count]);
            }
        });
        Self {
            child,
            stdin,
            stdout,
            stderr,
            key,
            next_id: 0,
            frames: Vec::new(),
            responses: HashMap::new(),
            approval_plan: VecDeque::new(),
            resolved: Vec::new(),
            answers: VecDeque::new(),
            questions: Vec::new(),
        }
    }

    fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr.lock().expect("stderr buffer")).into_owned()
    }

    async fn write(&mut self, frame: Value) {
        self.stdin
            .write_all(format!("{frame}\n").as_bytes())
            .await
            .expect("write engine stdin");
        self.stdin.flush().await.expect("flush engine stdin");
    }

    /// Send a request and return its id without waiting.
    async fn send(&mut self, method: &str, params: Value) -> String {
        self.next_id += 1;
        let id = format!("host-{}", self.next_id);
        self.write(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await;
        id
    }

    /// Read one frame, answering engine-initiated requests on the way.
    async fn pump_one(&mut self) {
        let line = tokio::time::timeout(FRAME_TIMEOUT, self.stdout.next_line())
            .await
            .unwrap_or_else(|_| panic!("engine silent for {FRAME_TIMEOUT:?}\n{}", self.context()))
            .expect("read engine stdout")
            .unwrap_or_else(|| panic!("engine stdout closed\n{}", self.context()));
        self.handle_line(&line).await;
    }

    /// Keep reading (and answering) for a fixed window; used to see what does NOT happen.
    async fn drain_for(&mut self, window: Duration) {
        let end = tokio::time::Instant::now() + window;
        while let Ok(read) = tokio::time::timeout_at(end, self.stdout.next_line()).await {
            let line = read
                .expect("read engine stdout")
                .unwrap_or_else(|| panic!("engine stdout closed\n{}", self.context()));
            self.handle_line(&line).await;
        }
    }

    async fn handle_line(&mut self, line: &str) {
        let frame: Value = serde_json::from_str(line)
            .unwrap_or_else(|error| panic!("engine wrote a non-JSON line {line:?}: {error}"));
        assert_eq!(frame["jsonrpc"], "2.0", "{frame}");
        self.frames.push(frame.clone());
        match (frame.get("method").and_then(Value::as_str), frame.get("id")) {
            (Some(method), Some(id)) => self.answer_reverse(method, id.clone(), &frame).await,
            (Some(method), None) => {
                hoh_protocol::validate_notification(method, &frame["params"]).unwrap_or_else(
                    |error| panic!("invalid notification {method}: {error}\n{frame}"),
                );
            }
            (None, Some(id)) => {
                self.responses.insert(key_of(id), frame);
            }
            (None, None) => panic!("frame without method or id: {frame}"),
        }
    }

    fn context(&self) -> String {
        let tail = self
            .frames
            .iter()
            .rev()
            .take(8)
            .rev()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        format!("last frames:\n{tail}\nstderr: {}", self.stderr_text())
    }

    async fn answer_reverse(&mut self, method: &str, id: Value, frame: &Value) {
        let params = &frame["params"];
        match method {
            "approval/request" => {
                hoh_protocol::validate_params_host(method, params)
                    .unwrap_or_else(|error| panic!("invalid approval/request: {error}\n{frame}"));
                let request: HostApprovalRequest =
                    serde_json::from_value(params.clone()).expect("host approval request");
                let planned = self.approval_plan.pop_front().unwrap_or_else(|| {
                    panic!("unplanned approval request: {frame}\n{}", self.context())
                });
                assert_eq!(
                    request.tool_name, planned.tool_name,
                    "approval for another action than planned: {frame}"
                );
                let lifetime = request.expires_at.saturating_sub(now_ms());
                assert!(
                    (1..=300_000).contains(&lifetime),
                    "approval must expire within five minutes, left {lifetime} ms"
                );
                let (outcome, mac_over) = match planned.verdict {
                    Verdict::Allow => ("allowed-once", "allowed-once"),
                    Verdict::Reject => ("rejected", "rejected"),
                    Verdict::Forge => ("allowed-once", "rejected"),
                };
                let mac = approval::mac(&self.key, &request, mac_over);
                self.resolved.push(Resolved {
                    request: request.clone(),
                });
                self.write(json!({
                    "jsonrpc":"2.0","id":id,
                    "result":{"approvalId":request.approval_id,"outcome":outcome,"mac":mac}
                }))
                .await;
            }
            "user-question/ask" => {
                hoh_protocol::validate_params(method, params)
                    .unwrap_or_else(|error| panic!("invalid user-question/ask: {error}\n{frame}"));
                self.questions.push(params.clone());
                let answer = self
                    .answers
                    .pop_front()
                    .unwrap_or_else(|| panic!("unplanned question: {frame}"));
                hoh_protocol::validate_result(method, &answer).expect("planned answer is valid");
                self.write(json!({"jsonrpc":"2.0","id":id,"result":answer}))
                    .await;
            }
            other => panic!("unexpected request from the engine: {other}\n{frame}"),
        }
    }

    /// Wait for the response of a previously sent request.
    async fn response(&mut self, id: &str) -> Value {
        loop {
            if let Some(frame) = self.responses.remove(id) {
                return frame;
            }
            self.pump_one().await;
        }
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params).await;
        self.response(&id).await
    }

    /// A call that must succeed; the result is checked against the protocol schema.
    async fn ok(&mut self, method: &str, params: Value) -> Value {
        let frame = self.call(method, params).await;
        let result = frame
            .get("result")
            .unwrap_or_else(|| panic!("{method} failed: {frame}\n{}", self.context()))
            .clone();
        hoh_protocol::validate_result(method, &result)
            .unwrap_or_else(|error| panic!("{method} result is invalid: {error}\n{result}"));
        result
    }

    async fn error(&mut self, method: &str, params: Value) -> Value {
        let frame = self.call(method, params).await;
        assert!(frame.get("error").is_some(), "{method} must fail: {frame}");
        frame["error"].clone()
    }

    /// Pump until a notification at or after frame `from` passes `predicate`; it is returned.
    async fn until(&mut self, from: usize, mut predicate: impl FnMut(&Value) -> bool) -> Value {
        let mut index = from;
        loop {
            while index < self.frames.len() {
                let frame = &self.frames[index];
                index += 1;
                if frame.get("method").is_some() && predicate(frame) {
                    return frame.clone();
                }
            }
            self.pump_one().await;
        }
    }

    /// Prompt a session and wait until it is idle again; returns the notification position
    /// where this prompt started, so assertions look only at its frames.
    async fn prompt(&mut self, session: &str, message: &str) -> usize {
        let start = self.frames.len();
        let result = self
            .ok(
                "session/prompt",
                json!({"sessionId":session,"contentBlocks":[{"type":"text","text":message}]}),
            )
            .await;
        assert!(result["messageId"].is_string(), "{result}");
        self.until_idle(session, start).await;
        start
    }

    async fn until_idle(&mut self, session: &str, from: usize) {
        // `session.status running` precedes the final `idle`; take the first idle after it.
        let mut running = false;
        let mut index = from;
        loop {
            while index < self.frames.len() {
                let frame = &self.frames[index];
                index += 1;
                if frame["method"] == "session.status" && frame["params"]["sessionId"] == session {
                    match frame["params"]["status"].as_str() {
                        Some("running") => running = true,
                        Some("idle") if running => return,
                        _ => {}
                    }
                }
            }
            self.pump_one().await;
        }
    }

    fn session_events(&self, session: &str, from: usize) -> Vec<Value> {
        self.frames[from..]
            .iter()
            .filter(|frame| {
                frame["method"] == "session.event" && frame["params"]["sessionId"] == session
            })
            .map(|frame| frame["params"]["event"].clone())
            .collect()
    }

    fn notifications(&self, method: &str) -> Vec<Value> {
        self.frames
            .iter()
            .filter(|frame| frame["method"] == method)
            .map(|frame| frame["params"].clone())
            .collect()
    }

    async fn initialize(&mut self, workspace: &Workspace) -> Value {
        let result = self
            .ok(
                "initialize",
                json!({
                    "cwd": workspace.folder,
                    "provider": "openai-compatible",
                    "model": MODEL,
                    "hoh": {"protocolVersion": 1, "approvalKey": approval::hex(&self.key)},
                    "capabilities": {"approvals": "host", "userQuestions": true}
                }),
            )
            .await;
        assert_eq!(result["hoh"]["protocolVersion"], 1, "{result}");
        assert_eq!(result["hoh"]["approvals"], "host", "{result}");
        result
    }

    fn plan(&mut self, tool_name: &'static str, verdict: Verdict) {
        self.approval_plan
            .push_back(PlannedApproval { tool_name, verdict });
    }

    async fn shutdown(mut self) -> String {
        self.ok("shutdown", json!({})).await;
        let Self {
            mut child,
            stdin,
            stderr,
            approval_plan,
            ..
        } = self;
        drop(stdin);
        let status = tokio::time::timeout(FRAME_TIMEOUT, child.wait())
            .await
            .expect("engine exits after shutdown")
            .expect("wait for engine");
        let stderr = String::from_utf8_lossy(&stderr.lock().expect("stderr buffer")).into_owned();
        assert_eq!(status.code(), Some(0), "stderr: {stderr}");
        assert!(
            approval_plan.is_empty(),
            "planned approvals never asked: {:?}",
            approval_plan
                .iter()
                .map(|planned| planned.tool_name)
                .collect::<Vec<_>>()
        );
        stderr
    }
}

fn key_of(id: &Value) -> String {
    id.as_str().map_or_else(|| id.to_string(), str::to_owned)
}

// ---------------------------------------------------------------- assertions on the session log

fn kind(event: &Value) -> &str {
    event["type"].as_str().unwrap_or("")
}

fn tool_calls(events: &[Value]) -> Vec<(String, Value)> {
    events
        .iter()
        .filter(|event| kind(event) == "tool/call")
        .map(|event| {
            let arguments = event["data"]["arguments"]
                .as_str()
                .and_then(|text| serde_json::from_str(text).ok())
                .unwrap_or_else(|| event["data"]["arguments"].clone());
            (
                event["data"]["name"].as_str().unwrap_or("").to_owned(),
                arguments,
            )
        })
        .collect()
}

/// Text of the result recorded for the call with this id.
fn tool_result_text(events: &[Value], call_id: &str) -> String {
    events
        .iter()
        .find(|event| {
            kind(event) == "tool/result" && event["data"]["message"]["toolCallId"] == call_id
        })
        .unwrap_or_else(|| panic!("no tool/result for {call_id}: {events:#?}"))["data"]["message"]
        .to_string()
}

/// One line per event: enough to read a failure without the whole log.
fn brief(events: &[Value]) -> String {
    events
        .iter()
        .map(|event| {
            let data = &event["data"];
            let detail = match kind(event) {
                "tool/call" => format!("{} {}", data["name"], data["arguments"]),
                "tool/result" => data["message"].to_string(),
                "turn/end" => data["reason"].to_string(),
                "assistant/attempt" => data["stream"].to_string(),
                _ => String::new(),
            };
            format!(
                "#{} {} {}",
                event["seq"],
                kind(event),
                detail.chars().take(400).collect::<String>()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn mentions(events: &[Value], needle: &str) -> bool {
    events
        .iter()
        .any(|event| event.to_string().contains(needle))
}

// ---------------------------------------------------------------- the milestone scenario

#[tokio::test]
async fn window_milestone_core_runs_end_to_end_on_the_mock_model() {
    let workspace = Workspace::new();
    let folder = workspace.folder.clone();
    let mock = start_mock(vec![
        // 0: plain streamed text
        text(&["Привет! ", "Я на связи."]),
        // 1..5: one prompt, five model steps through the file and shell tools
        call(
            "call-bash",
            "bash",
            json!({"command":"pwd","description":"Show the current folder"}),
        ),
        call(
            "call-read",
            "read",
            json!({"file_path": folder.join("note.txt")}),
        ),
        call(
            "call-write",
            "write",
            json!({"file_path": folder.join("out").join("result.txt"), "content":"готово\n"}),
        ),
        call(
            "call-cwd",
            "working_directory",
            json!({"cd": folder.join("out")}),
        ),
        text(&["Работа ", "завершена."]),
    ])
    .await;
    let base_url = mock.base_url.clone();

    let mut host = Host::start(&workspace, approval_key()).await;
    host.initialize(&workspace).await;

    // The key is installed once; a second handshake is refused and changes nothing.
    let error = host
        .error(
            "initialize",
            json!({
                "cwd": folder, "provider": "openai-compatible", "model": MODEL,
                "hoh": {"protocolVersion": 1, "approvalKey": approval::hex(&[7; 32])},
                "capabilities": {"approvals": "host"}
            }),
        )
        .await;
    assert!(error["message"].is_string(), "{error}");

    // --- first run: probe, key, settings; each privileged call waits for a host approval
    host.plan("hoh/model/probe", Verdict::Allow);
    let probe = host
        .ok(
            "hoh/model/probe",
            json!({"baseUrl": base_url, "model": MODEL}),
        )
        .await;
    assert_eq!(probe["ok"], true, "{probe}");
    assert!(
        probe["models"]
            .as_array()
            .is_some_and(|models| models.contains(&json!(MODEL))),
        "{probe}"
    );

    host.plan("hoh/model/key/set", Verdict::Allow);
    host.ok(
        "hoh/model/key/set",
        json!({"apiKey": API_KEY, "baseUrl": base_url}),
    )
    .await;
    let key_approval = &host.resolved.last().expect("key approval").request;
    assert!(
        !key_approval.action.to_string().contains(API_KEY)
            && !key_approval.human_text.contains(API_KEY),
        "the approval request must never carry the key: {key_approval:?}"
    );

    host.plan("hoh/settings/set", Verdict::Allow);
    host.ok(
        "hoh/settings/set",
        json!({"model": {"baseUrl": base_url, "model": MODEL}}),
    )
    .await;
    let settings = host.ok("hoh/settings/get", json!({})).await;
    assert_eq!(settings["model"]["model"], MODEL, "{settings}");
    assert!(
        !settings.to_string().contains(API_KEY),
        "settings are open data, the key lives in credentials: {settings}"
    );

    // --- a streamed answer
    let start = host.prompt(SESSION, "Привет").await;
    let events = host.session_events(SESSION, start);
    assert!(mentions(&events, "Привет"), "user message is in the feed");
    assert!(
        mentions(&events, "Я на связи."),
        "assistant answer is in the feed:\n{}",
        brief(&events)
    );
    assert!(events.iter().any(|event| kind(event) == "turn/end"));

    // --- tools: bash, read, write, working-directory
    let start = host.prompt(SESSION, "Сделай работу в папке").await;
    let events = host.session_events(SESSION, start);

    let names = tool_calls(&events)
        .into_iter()
        .map(|(name, _)| name)
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        ["bash", "read", "write", "working_directory"],
        "tool steps of the feed:\n{}",
        brief(&events)
    );
    assert!(tool_result_text(&events, "call-bash").contains(folder.to_str().expect("utf-8")));
    assert!(tool_result_text(&events, "call-read").contains("заметка окна"));
    assert_eq!(
        std::fs::read_to_string(folder.join("out").join("result.txt")).expect("written file"),
        "готово\n"
    );
    assert!(mentions(&events, "working-directory/change"));
    assert!(mentions(&events, "Работа "), "final answer is in the feed");

    // --- working directory of the session: reading is free, changing needs approval
    let cwd = host
        .ok(
            "session/working-directory/get",
            json!({"sessionId": SESSION}),
        )
        .await;
    assert_eq!(cwd["cwd"], json!(folder.join("out")), "{cwd}");
    host.plan("session/working-directory/set", Verdict::Allow);
    let cwd = host
        .ok(
            "session/working-directory/set",
            json!({"sessionId": SESSION, "path": folder}),
        )
        .await;
    assert_eq!(cwd["cwd"], json!(folder), "{cwd}");

    // --- history and list
    let history = host
        .ok(
            "session/events",
            json!({"sessionId": SESSION, "afterSeq": -1, "limit": 1000}),
        )
        .await;
    let log = history["events"].as_array().expect("events").clone();
    assert!(!log.is_empty());
    let seqs = log
        .iter()
        .map(|event| event["seq"].as_i64().expect("seq"))
        .collect::<Vec<_>>();
    assert!(seqs.windows(2).all(|pair| pair[0] < pair[1]), "{seqs:?}");
    assert_eq!(history["nextSeq"], json!(seqs.last()), "{history}");
    assert_eq!(tool_calls(&log).len(), 4, "history has the same tool steps");
    let page = host
        .ok(
            "session/events",
            json!({"sessionId": SESSION, "afterSeq": seqs[2], "limit": 2}),
        )
        .await;
    assert_eq!(page["events"].as_array().expect("page").len(), 2);
    assert_eq!(page["events"][0]["seq"], json!(seqs[3]), "{page}");
    let beyond = host
        .ok(
            "session/events",
            json!({"sessionId": SESSION, "afterSeq": history["nextSeq"], "limit": 10}),
        )
        .await;
    assert_eq!(beyond["events"], json!([]), "{beyond}");
    host.error("session/events", json!({"sessionId": SESSION, "limit": 0}))
        .await;
    let list = host.ok("session/list", json!({})).await;
    let listed = list["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|session| session["id"] == SESSION)
        .unwrap_or_else(|| panic!("session is listed: {list}"));
    assert!(
        listed["title"]
            .as_str()
            .is_some_and(|title| !title.is_empty()),
        "{listed}"
    );

    // --- what the model server saw
    let journal = mock.requests().await;
    let completions = journal
        .iter()
        .filter(|record| {
            record.path == "/v1/chat/completions"
                && record
                    .body
                    .as_ref()
                    .is_some_and(|body| body["stream"] == true)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        completions.len(),
        6,
        "one model request per scenario step, journal paths: {:?}",
        journal
            .iter()
            .map(|record| &record.path)
            .collect::<Vec<_>>()
    );
    assert!(
        completions.iter().all(|record| record.authorized),
        "every model request carries the stored key"
    );
    let first = completions[0].body.as_ref().expect("body");
    assert_eq!(first["model"], MODEL);
    let advertised = first["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap_or("").to_owned())
        .collect::<Vec<_>>();
    for tool in ["bash", "read", "write", "working_directory"] {
        assert!(
            advertised.contains(&tool.to_owned()),
            "{tool}: {advertised:?}"
        );
    }
    let last = completions[5].body.as_ref().expect("body");
    assert!(
        last["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .any(|message| message["role"] == "tool"),
        "tool results are sent back to the model: {last}"
    );
    assert!(
        journal.iter().any(|record| record.path == "/v1/models"),
        "the probe asked for the model list"
    );

    // --- shutdown, then the secrets audit and a second start over the same data
    let frames_text = host
        .frames
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !frames_text.contains(API_KEY),
        "the key never leaves the engine"
    );
    let stderr = host.shutdown().await;
    assert!(stderr.is_empty(), "stderr: {stderr}");
    assert_key_only_in_credentials(workspace.root.path(), &workspace.home());

    let mut again = Host::start(&workspace, [9; 32]).await;
    again.initialize(&workspace).await;
    let list = again.ok("session/list", json!({})).await;
    assert!(
        list["sessions"]
            .as_array()
            .expect("sessions")
            .iter()
            .any(|session| session["id"] == SESSION),
        "session survives a restart: {list}"
    );
    let stored = again
        .ok(
            "session/events",
            json!({"sessionId": SESSION, "afterSeq": -1, "limit": 1000}),
        )
        .await;
    let stored_events = stored["events"].as_array().expect("stored events");
    // Shutdown may close the log with events of its own; nothing logged before may be lost.
    assert!(
        stored_events.len() >= log.len(),
        "history lost events after a restart: {} stored, {} before\nstored tail:\n{}",
        stored_events.len(),
        log.len(),
        brief(&stored_events[stored_events.len().saturating_sub(3)..])
    );
    for (before, after) in log.iter().zip(stored_events) {
        assert_eq!(
            before, after,
            "history differs after a restart at seq {}",
            before["seq"]
        );
    }
    again.shutdown().await;
}

/// The key is saved by the credentials service in `$DSH_HOME/.credentials.yaml` and nowhere else.
fn assert_key_only_in_credentials(root: &Path, home: &Path) {
    let credentials = home.join(".credentials.yaml");
    let mut stack = vec![root.to_path_buf()];
    let mut found_in_credentials = false;
    while let Some(directory) = stack.pop() {
        for entry in std::fs::read_dir(&directory).expect("read folder") {
            let path = entry.expect("folder entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            if !String::from_utf8_lossy(&bytes).contains(API_KEY) {
                continue;
            }
            assert_eq!(path, credentials, "the key leaked into {}", path.display());
            found_in_credentials = true;
        }
    }
    assert!(
        found_in_credentials,
        "the key was stored in the credentials file"
    );
}

// ---------------------------------------------------------------- the safety scenarios

/// A rejected action does not run, and the model is told so.
#[tokio::test]
async fn rejected_dangerous_action_does_not_run() {
    let workspace = Workspace::new();
    let mock = start_mock(vec![
        dangerous_bash("call-danger", &workspace.folder.join("generated")),
        text(&["Не стал удалять."]),
    ])
    .await;
    let mut host = Host::start(&workspace, approval_key()).await;
    host.initialize(&workspace).await;
    configure_model(&mut host, &mock.base_url).await;

    host.plan("bash", Verdict::Reject);
    let start = host.prompt(SESSION, "Удали generated").await;
    let events = host.session_events(SESSION, start);
    assert!(
        workspace
            .folder
            .join("generated")
            .join("leftover.txt")
            .exists(),
        "a rejected command must not run"
    );
    assert!(
        host.notifications("approval.resolved")
            .iter()
            .any(|note| note["outcome"] == "rejected"),
        "{:#?}",
        host.notifications("approval.resolved")
    );
    assert!(mentions(&events, "Не стал удалять."), "{}", brief(&events));
    host.shutdown().await;
}

/// A decision whose MAC does not belong to its outcome is not a decision; cancelling the turn
/// leaves the action undone and the approval closed as cancelled.
#[tokio::test]
async fn forged_mac_does_not_run_the_action_and_cancel_closes_the_approval() {
    let workspace = Workspace::new();
    let mock = start_mock(vec![dangerous_bash(
        "call-danger",
        &workspace.folder.join("generated"),
    )])
    .await;
    let mut host = Host::start(&workspace, approval_key()).await;
    host.initialize(&workspace).await;
    configure_model(&mut host, &mock.base_url).await;

    host.plan("bash", Verdict::Forge);
    let answered_before = host.resolved.len();
    let start = host.frames.len();
    let result = host
        .ok(
            "session/prompt",
            json!({"sessionId":SESSION,"contentBlocks":[{"type":"text","text":"Удали generated"}]}),
        )
        .await;
    assert!(result["messageId"].is_string());
    // The engine either refuses the decision or keeps waiting; either way the turn must
    // still be open or already failed, and the command must not have run.
    let asked = host
        .until(start, |frame| frame["method"] == "approval.pending")
        .await;
    assert!(asked["params"]["approvalId"].is_string());
    while host.resolved.len() == answered_before {
        host.pump_one().await;
    }
    host.drain_for(Duration::from_millis(500)).await;
    let forged_id = host
        .resolved
        .last()
        .expect("forged decision was sent")
        .request
        .approval_id
        .clone();
    assert!(
        host.notifications("approval.resolved")
            .iter()
            .all(|note| note["approvalId"] != forged_id.as_str()
                || note["outcome"] != "allowed-once"),
        "a forged decision resolved as allowed: {:#?}",
        host.notifications("approval.resolved")
    );
    assert!(
        workspace
            .folder
            .join("generated")
            .join("leftover.txt")
            .exists(),
        "the command ran on a forged decision"
    );

    host.ok("session/cancel", json!({"sessionId": SESSION}))
        .await;
    host.until_idle_or_already(SESSION, start).await;
    host.drain_for(Duration::from_millis(300)).await;
    assert!(
        workspace
            .folder
            .join("generated")
            .join("leftover.txt")
            .exists()
    );
    let closed = host.notifications("approval.resolved");
    assert!(
        closed
            .iter()
            .any(|note| note["approvalId"] == forged_id.as_str()
                && matches!(
                    note["outcome"].as_str(),
                    Some("rejected" | "cancelled" | "unavailable")
                )),
        "the approval was left open: {closed:#?}"
    );
    host.shutdown().await;
}

impl Host {
    /// After a cancel the status may already have turned idle before the call returned.
    async fn until_idle_or_already(&mut self, session: &str, from: usize) {
        let already = self.frames[from..].iter().rev().find(|frame| {
            frame["method"] == "session.status" && frame["params"]["sessionId"] == session
        });
        if already.is_some_and(|frame| frame["params"]["status"] == "idle") {
            return;
        }
        self.until_idle(session, from).await;
    }
}

/// First-run model setup shared by the safety scenarios.
async fn configure_model(host: &mut Host, base_url: &str) {
    host.plan("hoh/model/key/set", Verdict::Allow);
    host.ok(
        "hoh/model/key/set",
        json!({"apiKey": API_KEY, "baseUrl": base_url}),
    )
    .await;
    host.plan("hoh/settings/set", Verdict::Allow);
    host.ok(
        "hoh/settings/set",
        json!({"model": {"baseUrl": base_url, "model": MODEL}}),
    )
    .await;
}

/// Prompt, wait until the engine is answering, cancel; returns the frame index of the prompt.
async fn start_and_cancel_a_slow_turn(host: &mut Host, mock: &MockServer) -> usize {
    let start = host.frames.len();
    host.ok(
        "session/prompt",
        json!({"sessionId":SESSION,"contentBlocks":[{"type":"text","text":"Думай долго"}]}),
    )
    .await;
    host.until(start, |frame| {
        frame["method"] == "session.status" && frame["params"]["status"] == "running"
    })
    .await;
    // Cancel only once the model request is on its way: a turn cancelled earlier never makes one.
    for _ in 0..500 {
        if !mock.requests().await.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        !mock.requests().await.is_empty(),
        "the slow turn never reached the model"
    );
    host.ok("session/cancel", json!({"sessionId": SESSION}))
        .await;
    host.until_idle_or_already(SESSION, start).await;
    start
}

/// `session/cancel` closes a turn that is still being answered: status idle, turn logged as ended.
#[tokio::test]
async fn cancel_closes_a_running_turn() {
    let workspace = Workspace::new();
    let mock = start_mock_with_delays(vec![text(&["Долгий ответ."])], &[(0, 10_000)]).await;
    let mut host = Host::start(&workspace, approval_key()).await;
    host.initialize(&workspace).await;
    configure_model(&mut host, &mock.base_url).await;

    let start = start_and_cancel_a_slow_turn(&mut host, &mock).await;
    let events = host.session_events(SESSION, start);
    let ended = events
        .iter()
        .find(|event| kind(event) == "turn/end")
        .unwrap_or_else(|| panic!("a cancelled turn is closed in the log:\n{}", brief(&events)));
    assert_ne!(
        ended["data"]["reason"]["kind"],
        "completed",
        "{}",
        brief(&events)
    );
    assert!(!mentions(&events, "Долгий ответ."), "{}", brief(&events));
    host.shutdown().await;
}

/// After a cancel the model request is abandoned and the next prompt is a turn of its own.
#[tokio::test]
async fn cancel_aborts_the_model_request_and_the_next_turn_is_independent() {
    let workspace = Workspace::new();
    let mock = start_mock_with_delays(
        vec![text(&["Долгий ответ."]), text(&["Снова на связи."])],
        &[(0, 6_000)],
    )
    .await;
    let mut host = Host::start(&workspace, approval_key()).await;
    host.initialize(&workspace).await;
    configure_model(&mut host, &mock.base_url).await;

    start_and_cancel_a_slow_turn(&mut host, &mock).await;
    let start = host.prompt(SESSION, "Ты здесь?").await;
    let events = host.session_events(SESSION, start);
    assert!(
        mentions(&events, "Снова на связи.") && !mentions(&events, "Долгий ответ."),
        "the new turn got the cancelled request's text:\n{}",
        brief(&events)
    );
    assert_eq!(
        mock.requests().await.len(),
        2,
        "each prompt makes its own model request"
    );
    host.shutdown().await;
}

/// The model plans with the todo tool and the list reaches the feed.
#[tokio::test]
async fn todo_list_reaches_the_feed() {
    let workspace = Workspace::new();
    let mock = start_mock(vec![
        call(
            "call-todo",
            "todo_write",
            json!({"todos":[
                {"content":"Прочитать заметку","status":"in_progress"},
                {"content":"Записать результат","status":"pending"}
            ]}),
        ),
        text(&["План записан."]),
    ])
    .await;
    let mut host = Host::start(&workspace, approval_key()).await;
    host.initialize(&workspace).await;
    configure_model(&mut host, &mock.base_url).await;

    let start = host.prompt(SESSION, "Составь план").await;
    let events = host.session_events(SESSION, start);
    assert_eq!(
        tool_calls(&events)
            .into_iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>(),
        ["todo_write"],
        "{}",
        brief(&events)
    );
    assert!(
        events.iter().any(|event| kind(event) == "todo/write"),
        "{}",
        brief(&events)
    );
    assert!(mentions(&events, "Прочитать заметку"));
    assert!(!tool_result_text(&events, "call-todo").contains("\"isError\":true"));
    host.shutdown().await;
}

/// The question travels to the host as `user-question/ask`; the answer goes back to the model.
#[tokio::test]
async fn ask_user_question_round_trips_through_the_host() {
    let workspace = Workspace::new();
    let mock = start_mock(vec![
        call(
            "call-ask",
            "ask_user_question",
            json!({"questions":[{
                "id":"q1","question":"Удалить папку generated?",
                "options":[{"label":"Да"},{"label":"Нет"}]
            }]}),
        ),
        text(&["Принято."]),
    ])
    .await;
    let mut host = Host::start(&workspace, approval_key()).await;
    host.initialize(&workspace).await;
    configure_model(&mut host, &mock.base_url).await;

    host.answers
        .push_back(json!({"answers":[{"id":"q1","selected":["Да"]}]}));
    let start = host.prompt(SESSION, "Спроси меня").await;
    let events = host.session_events(SESSION, start);
    assert_eq!(
        host.questions.len(),
        1,
        "one ask-user round trip:\n{}",
        brief(&events)
    );
    assert_eq!(host.questions[0]["questions"][0]["id"], "q1");
    assert_eq!(host.questions[0]["sessionId"], SESSION);
    let result = tool_result_text(&events, "call-ask");
    assert!(
        result.contains("Да") && !result.contains("\"isError\":true"),
        "the answer reached the model: {result}"
    );
    host.shutdown().await;
}

/// An allowed dangerous action asks the host first, runs after a valid MAC, and is logged once.
#[tokio::test]
async fn allowed_dangerous_action_runs_after_a_valid_decision() {
    let workspace = Workspace::new();
    let mock = start_mock(vec![
        dangerous_bash("call-danger", &workspace.folder.join("generated")),
        text(&["Папка удалена."]),
    ])
    .await;
    let mut host = Host::start(&workspace, approval_key()).await;
    host.initialize(&workspace).await;
    configure_model(&mut host, &mock.base_url).await;

    host.plan("bash", Verdict::Allow);
    let start = host.prompt(SESSION, "Удали generated").await;
    let events = host.session_events(SESSION, start);
    assert!(
        !workspace.folder.join("generated").exists(),
        "an allowed dangerous command ran:\n{}",
        brief(&events)
    );
    let danger = host.resolved.last().expect("dangerous approval");
    assert_eq!(danger.request.tool_name, "bash");
    assert!(!danger.request.human_text.is_empty());
    let id = &danger.request.approval_id;
    let pending = host.notifications("approval.pending");
    let resolved = host.notifications("approval.resolved");
    assert!(
        pending.iter().any(|note| note["approvalId"] == *id),
        "approval.pending for {id}: {pending:#?}"
    );
    assert_eq!(
        resolved
            .iter()
            .filter(|note| note["approvalId"] == *id && note["outcome"] == "allowed-once")
            .count(),
        1,
        "approval.resolved once for {id}: {resolved:#?}"
    );
    host.shutdown().await;
}

// ---------------------------------------------------------------- deleting inside the folder

fn delete_in_folder(id: &str, command: &str) -> ScenarioResponse {
    call(
        id,
        "bash",
        json!({"command": command, "description": "Remove old files in the folder"}),
    )
}

fn seed_old_files(workspace: &Workspace) -> Vec<PathBuf> {
    ["old1.txt", "old2.txt", "old3.txt"]
        .iter()
        .map(|name| {
            let path = workspace.folder.join(name);
            std::fs::write(&path, "старое\n").expect("old file");
            path
        })
        .collect()
}

/// `rm` of files inside the session folder (no sandbox escalation) is put to the host: rejected,
/// the files stay and the model is told; the question is worded for a person, not a shell.
#[tokio::test]
async fn deleting_files_in_the_folder_is_rejected_and_nothing_is_removed() {
    let workspace = Workspace::new();
    let old = seed_old_files(&workspace);
    let mock = start_mock(vec![
        delete_in_folder("call-rm", "rm old1.txt old2.txt old3.txt"),
        text(&["Файлы оставил."]),
    ])
    .await;
    let mut host = Host::start(&workspace, approval_key()).await;
    host.initialize(&workspace).await;
    configure_model(&mut host, &mock.base_url).await;

    host.plan("bash", Verdict::Reject);
    let start = host.prompt(SESSION, "Убери старые файлы").await;
    let events = host.session_events(SESSION, start);
    assert!(
        old.iter().all(|path| path.exists()),
        "a rejected delete removed files:\n{}",
        brief(&events)
    );
    let asked = &host
        .resolved
        .last()
        .expect("the delete was put to the host")
        .request;
    assert!(
        asked
            .human_text
            .contains("удалить 3 файла: old1.txt, old2.txt, old3.txt")
            && asked.human_text.contains("Файлы не попадут в корзину"),
        "{}",
        asked.human_text
    );
    // A-03: `reason` is the Russian explanation with the files; the raw command is not in it.
    let reason = asked.reason.as_deref().unwrap_or_default();
    assert!(
        reason.contains("удалить 3 файла: old1.txt, old2.txt, old3.txt")
            && reason.contains("Файлы не попадут в корзину")
            && !reason.contains("rm old1.txt")
            && !reason.contains("Команда:"),
        "{reason}"
    );
    assert_eq!(
        asked.details.as_deref(),
        Some("Команда: rm old1.txt old2.txt old3.txt"),
        "the command belongs in the folded details"
    );
    assert!(mentions(&events, "Файлы оставил."), "{}", brief(&events));
    host.shutdown().await;
}

/// `allowed-once` removes the files, and the next identical delete asks again.
#[tokio::test]
async fn an_allowed_delete_in_the_folder_runs_once_and_the_next_asks_again() {
    let workspace = Workspace::new();
    let old = seed_old_files(&workspace);
    let command = "rm old1.txt old2.txt old3.txt";
    let mock = start_mock(vec![
        delete_in_folder("call-rm-1", command),
        delete_in_folder("call-rm-2", command),
        text(&["Готово."]),
    ])
    .await;
    let mut host = Host::start(&workspace, approval_key()).await;
    host.initialize(&workspace).await;
    configure_model(&mut host, &mock.base_url).await;

    host.plan("bash", Verdict::Allow);
    host.plan("bash", Verdict::Reject);
    let before = host.resolved.len();
    let start = host.prompt(SESSION, "Убери старые файлы").await;
    let events = host.session_events(SESSION, start);
    assert!(
        old.iter().all(|path| !path.exists()),
        "the allowed delete did not run:\n{}",
        brief(&events)
    );
    assert_eq!(
        host.resolved.len() - before,
        2,
        "the second identical call must ask again"
    );
    assert!(mentions(&events, "Готово."), "{}", brief(&events));
    host.shutdown().await;
}
