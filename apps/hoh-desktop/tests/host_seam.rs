//! Production T3 host -> desktop bridge/Broker -> real `hoh` engine seam.
//! `real-engine-seam.mjs` drives the production HohHost; this test owns every native boundary.

use hoh_desktop::{
    approval::{Begun, Broker, Resolution, now_ms},
    bridge,
    engine::{Engine, HostEvent, ProcessConfig},
};
use hoh_protocol::{ApprovalOutcome, framing};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant, SystemTime},
};

const KEY: [u8; 32] = [0x73; 32];

const FAKE_CLAUDE: &str = r#"#!/usr/bin/python3 -I
import json, os, sys

args = sys.argv[1:]
if "--version" in args:
    print("2.1.329 (Claude Code)")
    sys.exit(0)

def value(flag):
    return args[args.index(flag) + 1] if flag in args[:-1] else None

session = value("--session-id") or value("--resume") or "real-seam"
counter = 0

def write(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

def read():
    line = sys.stdin.readline()
    if not line:
        sys.exit(0)
    return json.loads(line)

def control(message):
    request = message.get("request") or {}
    response = {}
    if request.get("subtype") == "initialize":
        response = {"models": [{"value": "default", "displayName": "Default"}],
                    "account": {"email": "seam@example.invalid", "tokenSource": "claude.ai"},
                    "commands": []}
    if request.get("subtype") == "reload_skills":
        response = {"skills": []}
    write({"type": "control_response", "response": {"subtype": "success",
           "request_id": message.get("request_id"), "response": response}})

while True:
    message = read()
    if message.get("type") == "control_request":
        control(message)
        continue
    if message.get("type") != "user":
        continue
    counter += 1
    write({"type": "system", "subtype": "init", "session_id": session,
           "model": "claude-seam", "cwd": os.getcwd(), "tools": ["Bash"],
           "permissionMode": value("--permission-mode")})
    tool_id = "toolu-%d" % counter
    write({"type": "assistant", "session_id": session, "parent_tool_use_id": None,
           "message": {"id": "assistant-%d" % counter, "role": "assistant", "model": "claude-seam",
                       "content": [{"type": "tool_use", "id": tool_id, "name": "Bash",
                                    "input": {"command": "echo real-seam", "description": "Run the seam command"}}]}})
    request_id = "permission-%d" % counter
    write({"type": "control_request", "request_id": request_id,
           "request": {"subtype": "can_use_tool", "tool_name": "Bash",
                       "input": {"command": "echo real-seam", "description": "Run the seam command"},
                       "tool_use_id": tool_id}})
    while True:
        answer = read()
        if answer.get("type") == "control_request":
            control(answer)
            continue
        response = answer.get("response") or {}
        if answer.get("type") == "control_response" and response.get("request_id") == request_id:
            allowed = (response.get("response") or {}).get("behavior") == "allow"
            break
    write({"type": "user", "session_id": session, "parent_tool_use_id": None,
           "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": tool_id,
                       "content": "allowed" if allowed else "denied", "is_error": not allowed}]}})
    write({"type": "assistant", "session_id": session, "parent_tool_use_id": None,
           "message": {"id": "done-%d" % counter, "role": "assistant", "model": "claude-seam",
                       "content": [{"type": "text", "text": "seam complete"}]}})
    write({"type": "result", "subtype": "success", "is_error": False,
           "result": "seam complete", "session_id": session, "duration_ms": 1, "num_turns": 1})
"#;

struct TestRoot(PathBuf);
impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn root() -> TestRoot {
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("hoh-host-seam-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    TestRoot(root)
}

fn prepare_profile(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let work = root.join("work");
    let home = root.join("home");
    let profile = home.join("profiles/window");
    let bin = root.join("bin/claude");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&profile)
        .unwrap();
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(&bin, FAKE_CLAUDE).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        profile.join("package.json"),
        json!({"name":"dsh-profile-window","private":true,"dependencies":{},"dsh":{"profile":{"bundles":["@hoh/window"]}}}).to_string(),
    ).unwrap();
    std::fs::write(
        profile.join("cordis.patch.yml"),
        format!(
            "- id: harness-claude\n  config:\n    executable: {}\n",
            bin.display()
        ),
    )
    .unwrap();
    (work, home, root.join("engine-stderr.log"))
}

fn respond(engine: &Arc<Engine>, resolution: Resolution) {
    let Resolution::Engine {
        generation,
        id,
        decision,
    } = resolution
    else {
        panic!("unexpected preflight resolution")
    };
    let frame = match decision {
        Ok(decision) => framing::success(id, serde_json::to_value(decision).unwrap()),
        Err(error) => framing::error(id, -32000, error),
    };
    engine.respond(generation, frame).unwrap();
}

fn request_from_main(
    engine: &Arc<Engine>,
    broker: &Arc<Broker>,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let sent = Arc::new(std::sync::Mutex::new(None::<(u64, String)>));
    let arm = if bridge::needs_approval(method, &params) {
        let (broker, sent, method, params) = (
            broker.clone(),
            sent.clone(),
            method.to_owned(),
            params.clone(),
        );
        Some(Box::new(move |generation: u64, id: &str| {
            broker.arm_in_flight(&method, &params, generation, id, now_ms());
            *sent.lock().unwrap() = Some((generation, id.to_owned()));
        }) as hoh_desktop::engine::OnSend)
    } else {
        None
    };
    let result = bridge::request_with_approval_armed(
        engine,
        "main",
        method,
        params,
        |params| broker.confirm_from_main_window(method, params, None, now_ms()),
        arm,
    );
    if let Some((generation, id)) = sent.lock().unwrap().take() {
        broker.disarm(generation, &id);
    }
    result
}

#[test]
fn production_host_crosses_main_window_and_chat_approval_on_the_real_engine() {
    let root = root();
    let (work, home, stderr) = prepare_profile(&root.0);
    let engine_bin =
        PathBuf::from(std::env::var_os("HOH_REAL_ENGINE").expect("set HOH_REAL_ENGINE"));
    let (events, event_rx) = mpsc::channel();
    let engine = Engine::start(
        Arc::new(ProcessConfig {
            path: engine_bin,
            args: vec!["--profile".into(), "window".into()],
            env: vec![("DSH_HOME".into(), home.display().to_string())],
            stderr_file: Some(stderr.clone()),
            cwd: work.clone(),
            provider: "openai-compatible".into(),
            model: "local".into(),
        }),
        json!({
            "cwd": work, "provider": "openai-compatible", "model": "local",
            "capabilities": {"approvals": "host", "userQuestions": true},
            "hoh": {"protocolVersion": 1, "approvalKey": hoh_protocol::approval::hex(&KEY)}
        }),
        Arc::new(move |event| {
            let _ = events.send(event);
        }),
    );
    let broker = Arc::new(
        Broker::new(KEY)
            .unwrap()
            .with_audit_file(root.0.join("approvals.log")),
    );

    let driver = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../ui/tests/hoh-host/real-engine-seam.mjs");
    std::fs::create_dir_all(root.0.join("tmp")).unwrap();
    let mut child = Command::new("node")
        .arg(driver)
        .current_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .env("TMPDIR", root.0.join("tmp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let node_in = child.stdin.take().unwrap();
    let node_out = child.stdout.take().unwrap();
    let node_err = child.stderr.take().unwrap();
    let (to_node, from_native) = mpsc::channel::<Value>();
    thread::spawn(move || {
        let mut input = node_in;
        while let Ok(frame) = from_native.recv() {
            if writeln!(input, "{frame}")
                .and_then(|()| input.flush())
                .is_err()
            {
                break;
            }
        }
    });

    {
        let (engine, broker, to_node) = (engine.clone(), broker.clone(), to_node.clone());
        thread::spawn(move || {
            while let Ok(event) = event_rx.recv() {
                match event {
                    HostEvent::Notification(frame) => {
                        let _ = to_node.send(json!({"type":"notification","method":frame["method"],"params":frame["params"]}));
                    }
                    HostEvent::Status(status) => {
                        let _ = to_node.send(json!({"type":"status","status":status}));
                    }
                    HostEvent::Request {
                        generation,
                        id,
                        method,
                        params,
                    } => {
                        match broker
                            .begin_engine_or_grant(generation, id, &method, params, now_ms())
                            .unwrap()
                        {
                            Begun::Signed(resolution) => respond(&engine, resolution),
                            Begun::Window(open) => {
                                assert!(broker.show_in_chat(&open.label, Instant::now()))
                            }
                        }
                    }
                }
            }
        });
    }

    let mut result = None;
    for line in BufReader::new(node_out).lines() {
        let frame: Value = serde_json::from_str(&line.unwrap()).unwrap();
        match frame["type"].as_str() {
            Some("request") => {
                let id = frame["id"].clone();
                let answer = request_from_main(
                    &engine,
                    &broker,
                    frame["method"].as_str().unwrap(),
                    frame["params"].clone(),
                );
                let response = match answer {
                    Ok(value) => json!({"type":"response","id":id,"result":value}),
                    Err(error) => json!({"type":"response","id":id,"error":error}),
                };
                to_node.send(response).unwrap();
            }
            Some("decide") => {
                let id = frame["id"].clone();
                let outcome = match frame["outcome"].as_str() {
                    Some("allowed-once") => ApprovalOutcome::AllowedOnce,
                    Some("rejected") => ApprovalOutcome::Rejected,
                    other => panic!("bad outcome: {other:?}"),
                };
                let answer = broker.decide_in_chat(
                    frame["approvalId"].as_str().unwrap(),
                    outcome,
                    now_ms(),
                    Instant::now(),
                );
                let response = match answer {
                    Ok(decision) => {
                        respond(&engine, decision.resolution);
                        match decision.command_result {
                            Ok(()) => json!({"type":"response","id":id,"result":null}),
                            Err(error) => json!({"type":"response","id":id,"error":error}),
                        }
                    }
                    Err(error) => json!({"type":"response","id":id,"error":error}),
                };
                to_node.send(response).unwrap();
            }
            Some("result") => {
                result = Some(frame);
                break;
            }
            other => panic!("unknown node frame: {other:?} {frame}"),
        }
    }

    let status = child.wait_timeout(Duration::from_secs(20));
    if status.is_none() {
        let _ = child.kill();
    }
    let status = status.or_else(|| child.wait().ok()).unwrap();
    let stderr_text = BufReader::new(node_err)
        .lines()
        .map_while(Result::ok)
        .collect::<Vec<_>>()
        .join("\n");
    let result = result.unwrap_or_else(|| {
        panic!(
            "node returned no result; status={status}; stderr={stderr_text}; engine={}",
            std::fs::read_to_string(stderr).unwrap_or_default()
        )
    });
    assert_eq!(result["ok"], true, "{result}; node stderr={stderr_text}");
    assert_eq!(result["mode"], "auto", "{result}");
    assert!(
        result["approvalId"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "{result}"
    );
    assert!(
        result["humanText"]
            .as_str()
            .is_some_and(|text| !text.is_empty()),
        "{result}"
    );
    let audit = broker.audit_log();
    assert!(
        audit
            .iter()
            .any(|entry| entry.via == "main-window" && entry.method == "hoh/executor/access/set"),
        "{audit:?}"
    );
    assert!(
        audit
            .iter()
            .any(|entry| entry.via == "chat" && entry.human_text == result["humanText"]),
        "{audit:?}"
    );
    assert!(
        status.success(),
        "node status={status}; stderr={stderr_text}"
    );
    engine.stop();
}

trait WaitTimeout {
    fn wait_timeout(&mut self, timeout: Duration) -> Option<std::process::ExitStatus>;
}
impl WaitTimeout for std::process::Child {
    fn wait_timeout(&mut self, timeout: Duration) -> Option<std::process::ExitStatus> {
        let until = Instant::now() + timeout;
        loop {
            if let Some(status) = self.try_wait().unwrap() {
                return Some(status);
            }
            if Instant::now() >= until {
                return None;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}
