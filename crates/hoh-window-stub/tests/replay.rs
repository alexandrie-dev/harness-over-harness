use hoh_protocol::framing;

fn privileged_case(method: &str) {
    privileged_params_case(method, None);
}
fn privileged_params_case(method: &str, custom: Option<Value>) {
    let mut client = Client::new("text-turn", vec![], true);
    let stem = method.replace('/', "-");
    let request: Value = serde_json::from_str(
        &fs::read_to_string(root().join(format!("protocol/fixtures/{stem}.request.json"))).unwrap(),
    )
    .unwrap();
    let create: Value = serde_json::from_str(
        &fs::read_to_string(root().join("protocol/fixtures/hoh-employee-create.request.json"))
            .unwrap(),
    )
    .unwrap();
    let mut params = request["params"].clone();
    if method.ends_with("employee/update") {
        params["patch"]["permissions"] = create["params"]["draft"]["permissions"].clone();
    }
    if method.ends_with("draft/decide") {
        params["decision"] = json!("yes");
    }
    if method == "hoh/settings/set" {
        params["security"] = create["params"]["draft"]["permissions"].clone();
    }
    if let Some(custom) = custom {
        params = custom;
    }
    assert!(hoh_protocol::PRIVILEGED_METHODS.contains(&method));
    let duplicate = client.call("initialize",json!({"cwd":CWD,"provider":"p","model":"m","capabilities":{"approvals":"host"},"hoh":{"protocolVersion":1,"approvalKey":"11".repeat(32)}}));
    assert!(duplicate.get("error").is_some());
    let mut previous = None;
    for attempt in 0..3 {
        let id = json!(format!("privileged-{attempt}"));
        client.send(framing::request(id.clone(), method, params.clone()));
        let request = loop {
            let f = client.receive();
            assert_ne!(
                f["id"], id,
                "privileged method replied without approval: {method}"
            );
            if f["method"] == "approval/request" {
                break f;
            }
        };
        let native: hoh_protocol::HostApprovalRequest =
            serde_json::from_value(request["params"].clone()).unwrap();
        let expected = if method == "hoh/model/key/set" {
            json!({"keyHash":approval::action_hash(&json!({"apiKey":params["apiKey"]})),"baseUrl":params["baseUrl"]})
        } else {
            params.clone()
        };
        assert_eq!(native.action, json!({"method":method,"params":expected}));
        let valid = json!({"approvalId":native.approval_id,"outcome":"allowed-once","mac":approval::mac(&[0x42;32],&native,"allowed-once")});
        let answer = match attempt {
            0 => json!({"outcome":"allowed-once"}),
            1 => previous.clone().unwrap(),
            _ => valid.clone(),
        };
        client.send(framing::success(request["id"].clone(), answer));
        let reply = loop {
            let f = client.receive();
            if f["id"] == id {
                break f;
            }
        };
        if attempt < 2 {
            assert!(reply.get("error").is_some(), "{method}");
        } else {
            assert!(reply.get("result").is_some(), "{method}: {reply}");
        }
        previous = Some(valid);
    }
    client.shutdown();
}
macro_rules! privileged_test {
    ($name:ident,$method:literal) => {
        #[test]
        fn $name() {
            privileged_case($method)
        }
    };
}
privileged_test!(employee_create_requires_mac, "hoh/employee/create");
privileged_test!(
    employee_update_permissions_requires_mac,
    "hoh/employee/update"
);
privileged_test!(task_consent_requires_mac, "hoh/task/draft/decide");
privileged_test!(task_rollback_requires_mac, "hoh/task/rollback");
privileged_test!(settings_security_requires_mac, "hoh/settings/set");

privileged_test!(
    working_directory_requires_mac,
    "session/working-directory/set"
);
privileged_test!(probe_requires_mac, "hoh/model/probe");
privileged_test!(model_key_requires_mac, "hoh/model/key/set");
privileged_test!(setup_rules_requires_mac, "hoh/settings/rules/set");
#[test]
fn model_route_requires_mac() {
    privileged_params_case(
        "hoh/settings/set",
        Some(json!({"model":{"baseUrl":"https://new.example","model":"m"}})),
    )
}
#[test]
fn employee_task_assignment_requires_mac() {
    privileged_params_case(
        "hoh/employee/update",
        Some(json!({"id":"employee-1","rev":1,"patch":{"taskIds":["task-1"]}})),
    )
}
#[test]
fn saved_key_is_consumed_by_probe_without_leaking_into_frames() {
    let mut client = Client::new(
        "text-turn",
        vec![json!({"outcome":"allowed-once"}); 4],
        true,
    );
    let probe = json!({"baseUrl":"https://example.test","model":"m"});
    assert_eq!(
        client.call("hoh/model/probe", probe.clone())["result"]["ok"],
        false
    );
    let secret = "test-provider-key-not-for-logs";
    assert_eq!(
        client.call(
            "hoh/model/key/set",
            json!({"apiKey":secret,"baseUrl":"https://example.test"})
        )["result"],
        json!({})
    );
    assert_eq!(client.call("hoh/model/probe", probe)["result"]["ok"], true);
    assert_eq!(
        client.call(
            "hoh/settings/rules/set",
            json!({"internet":"ask","folder":"/workspace"})
        )["result"],
        json!({})
    );
    assert_eq!(
        client.call("hoh/settings/get", json!({}))["result"]["security"]["folders"],
        json!(["/workspace"])
    );
    assert!(
        !serde_json::to_string(&client.frames)
            .unwrap()
            .contains(secret)
    );
    client.shutdown();
}

#[test]
fn setup_approval_text_names_the_folder_internet_policy_and_provider() {
    let secret = "sk-private-not-in-the-dialog";
    let mut client = Client::new(
        "text-turn",
        vec![json!({"outcome":"allowed-once"}); 6],
        true,
    );
    let cases = [
        (
            "hoh/model/key/set",
            json!({"apiKey":secret,"baseUrl":"https://new.example.test/v1"}),
            "Сохранить API-ключ для провайдера \"https://new.example.test/v1\"?",
        ),
        (
            "hoh/settings/rules/set",
            json!({"internet":"ask","folder":"/home/Документы/Отчёты"}),
            "Разрешить работу в папке \"/home/Документы/Отчёты\" и доступ в интернет с запросом разрешения?",
        ),
        (
            "hoh/settings/rules/set",
            json!({"internet":"public","folder":"/team/shared"}),
            "Разрешить работу в папке \"/team/shared\" и доступ в интернет к общедоступным сайтам?",
        ),
        (
            "hoh/model/probe",
            json!({"baseUrl":"https://models.example.test/v1","model":"example-chat"}),
            "Разрешить проверку модели \"example-chat\" у провайдера \"https://models.example.test/v1\"?",
        ),
    ];
    for (method, params, expected_text) in cases {
        let reply = client.call(method, params);
        assert!(reply.get("error").is_none(), "{reply}");
        assert_eq!(
            client.reverse.last().unwrap()["params"]["humanText"],
            expected_text
        );
        assert_eq!(
            client
                .notifications()
                .iter()
                .rev()
                .find(|f| f["method"] == "approval.pending")
                .unwrap()["params"]["humanText"],
            expected_text
        );
        if method == "hoh/model/probe" {
            assert_eq!(
                reply["result"]["models"],
                json!(["local", "fixture-reasoner"])
            );
        }
    }
    let model = json!({"baseUrl":"https://saved.example.test/v1","model":"saved-chat"});
    assert_eq!(
        client.call("hoh/settings/set", json!({"model":model}))["result"]["model"],
        model
    );
    assert_eq!(
        client.call("hoh/settings/get", json!({}))["result"]["model"],
        model
    );
    assert_eq!(
        client.call(
            "hoh/model/key/set",
            json!({"apiKey":secret,"baseUrl":"https://saved.example.test/v1"})
        )["result"],
        json!({})
    );
    assert_eq!(
        client.reverse.last().unwrap()["params"]["humanText"],
        "Сохранить API-ключ для провайдера \"https://saved.example.test/v1\"?"
    );
    assert!(
        !serde_json::to_string(&client.frames)
            .unwrap()
            .contains(secret)
    );
    client.shutdown();
}

#[test]
fn signed_rejection_is_published_without_storing_the_key() {
    let mut client = Client::new(
        "text-turn",
        vec![
            json!({"outcome":"rejected"}),
            json!({"outcome":"allowed-once"}),
        ],
        true,
    );
    let reply = client.call(
        "hoh/model/key/set",
        json!({"apiKey":"sk-rejected","baseUrl":"https://rejected.example.test/v1"}),
    );
    assert!(reply.get("error").is_some());
    let resolved: Vec<_> = client
        .notifications()
        .into_iter()
        .filter(|f| f["method"] == "approval.resolved")
        .collect();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0]["params"]["outcome"], "rejected");
    assert_eq!(
        client.call(
            "hoh/model/probe",
            json!({"baseUrl":"https://example.test","model":"m"})
        )["result"]["ok"],
        false
    );
    client.shutdown();
}

use hoh_window_stub::approval;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    fs,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

const SESSION: &str = "fixture-root-session";
const CWD: &str = "/recording-workspace";
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn source() -> PathBuf {
    std::env::var_os("HOH_DSH_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness"))
}
fn rows(scenario: &str) -> Vec<Value> {
    fs::read_to_string(
        source()
            .join("snapshots/sdk")
            .join(scenario)
            .join("session.v4.jsonl"),
    )
    .unwrap()
    .lines()
    .map(|s| serde_json::from_str(s).unwrap())
    .collect()
}
struct Client {
    child: Child,
    input: ChildStdin,
    rx: mpsc::Receiver<Value>,
    frames: Vec<Value>,
    answers: VecDeque<Value>,
    host: bool,
    reverse: Vec<Value>,
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Client {
    fn new(scenario: &str, answers: Vec<Value>, host: bool) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hoh-window-stub"))
            .args(["--scenario", scenario, "--source-root"])
            .arg(source())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else { break };
                let frame = serde_json::from_str(&line).expect("binary emits JSON only");
                if tx.send(frame).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            input,
            rx,
            frames: Vec::new(),
            answers: answers.into(),
            host,
            reverse: Vec::new(),
        };
        let mut params = json!({"cwd":CWD,"approvalRequests":true,"provider":"deepseek-official","model":"deepseek-v4-flash","capabilities":{"userQuestions":true}});
        if host {
            params["capabilities"]["approvals"] = json!("host");
            params["hoh"] = json!({"protocolVersion":1,"approvalKey":"42".repeat(32)});
        }
        let result = client.call("initialize", params);
        assert_eq!(
            result["result"]["serverInfo"]["name"],
            "deepseek-harness-sdk-runtime"
        );
        if host {
            assert_eq!(
                result["result"]["hoh"],
                json!({"protocolVersion":1,"approvals":"host"})
            );
        }
        client
    }
    fn send(&mut self, frame: Value) {
        writeln!(self.input, "{}", frame).unwrap();
        self.input.flush().unwrap();
    }
    fn receive(&mut self) -> Value {
        let frame = self
            .rx
            .recv_timeout(Duration::from_secs(15))
            .expect("stub frame within 15s");
        assert_eq!(frame["jsonrpc"], "2.0");
        self.frames.push(frame.clone());
        frame
    }
    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = json!(format!("client-{}", self.frames.len()));
        self.send(framing::request(id.clone(), method, params));
        loop {
            let frame = self.receive();
            if frame.get("method").is_some() && frame.get("id").is_some() {
                self.answer(frame);
            } else if frame["id"] == id {
                return frame;
            }
        }
    }
    fn answer(&mut self, request: Value) {
        self.reverse.push(request.clone());
        assert!(request["params"]["sessionId"].is_string());
        let mut answer = self.answers.pop_front().expect("expected reverse request");
        if self.host && request["method"] == "approval/request" {
            let params = &request["params"];
            answer["approvalId"] = params["approvalId"].clone();
            answer["mac"] = json!(approval::mac(
                &[0x42; 32],
                &serde_json::from_value(params.clone()).unwrap(),
                answer["outcome"].as_str().unwrap()
            ));
        }
        let frame = framing::success(request["id"].clone(), answer);
        thread::scope(|scope| {
            let input = &mut self.input;
            scope.spawn(move || {
                thread::sleep(Duration::from_millis(150));
                writeln!(input, "{}", frame).unwrap();
                input.flush().unwrap();
            });
            assert!(matches!(
                self.rx.recv_timeout(Duration::from_millis(75)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
        });
    }
    fn run(&mut self, method: &str, content: Value) {
        let response = self.call(method, json!({"sessionId":SESSION,"contentBlocks":content}));
        assert!(response.get("error").is_none(), "{response}");
        let wait = self.call("session/wait", json!({"sessionId":SESSION}));
        assert_eq!(wait["result"], json!({}), "{wait}");
    }
    fn notifications(&self) -> Vec<Value> {
        self.frames
            .iter()
            .filter(|f| f.get("method").is_some() && f.get("id").is_none())
            .map(|f| json!({"method":f["method"],"params":f["params"]}))
            .collect()
    }
    fn events(&self) -> Vec<Value> {
        self.notifications()
            .into_iter()
            .filter(|n| n["method"] == "session.event")
            .map(|n| n["params"]["event"].clone())
            .collect()
    }
    fn shutdown(&mut self) {
        assert!(self.answers.is_empty());
        assert_eq!(self.call("shutdown", json!({}))["result"], json!({}));
        assert!(self.child.wait().unwrap().success());
    }
}
fn materialize(v: &mut Value) {
    match v {
        Value::String(s) => *s = s.replace("{{session:1}}", SESSION).replace("{{cwd}}", CWD),
        Value::Array(a) => {
            for v in a {
                materialize(v)
            }
        }
        Value::Object(o) => {
            for (k, v) in o {
                if k == "time" || k == "time0" {
                    *v = json!(0)
                } else {
                    materialize(v)
                }
            }
        }
        _ => {}
    }
}
fn user_inputs(rows: &[Value]) -> Vec<Value> {
    rows.iter()
        .filter(|r| r["type"] == "user/message" && r["data"]["source"]["kind"] == "user")
        .map(|r| r["data"]["content"].clone())
        .collect()
}
fn assert_recorded_events(client: &Client, rows: &[Value]) {
    // Independent projection: source persistence omits seq/time and header is not an event.
    let mut expected = Vec::new();
    let first = rows[1..]
        .iter()
        .position(|e| e["type"] == "agent/inbox/spliced" || e["type"] == "command/run")
        .unwrap();
    for (seq, row) in rows[1..].iter().enumerate().skip(first) {
        let mut event = json!({"type":row["type"],"seq":seq,"time":0,"data":row["data"]});
        for key in ["ignorable", "sourceEventSeqs", "surfaceOp"] {
            if let Some(value) = row.get(key) {
                event[key] = value.clone();
            }
        }
        materialize(&mut event);
        expected.push(event);
    }
    assert_eq!(client.events(), expected);
    for frame in client.notifications() {
        hoh_protocol::validate_notification(frame["method"].as_str().unwrap(), &frame["params"])
            .unwrap();
    }
}
#[test]
fn text_turn_matches_sdk_notification_golden_values() {
    let rows = rows("text-turn");
    let mut client = Client::new("text-turn", vec![], false);
    client.run("session/prompt", user_inputs(&rows)[0].clone());
    let mut expected: Vec<Value> =
        fs::read_to_string(source().join("snapshots/sdk/text-turn/notifications.expected.jsonl"))
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
    for frame in &mut expected {
        materialize(frame);
    }
    assert_eq!(client.notifications(), expected);
    assert_recorded_events(&client, &rows);
    let expected_result: Value = serde_json::from_str(
        &fs::read_to_string(source().join("snapshots/sdk/text-turn/result.expected.json")).unwrap(),
    )
    .unwrap();
    let last = client
        .events()
        .into_iter()
        .rfind(|e| e["type"] == "assistant/message")
        .unwrap();
    assert_eq!(
        last["data"]["message"]["content"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["type"] == "text")
            .unwrap()["text"],
        expected_result["finalResponse"]
    );
    client.shutdown();
}
#[test]
fn approval_waits_for_both_sdk_decisions() {
    let rows = rows("approval");
    let mut client = Client::new(
        "approval",
        vec![
            json!({"outcome":"allowed-once"}),
            json!({"outcome":"rejected"}),
        ],
        false,
    );
    for content in user_inputs(&rows) {
        client.run("session/prompt", content);
    }
    assert_eq!(client.reverse.len(), 2);
    for request in &client.reverse {
        assert_eq!(request["method"], "approval/request");
        assert_eq!(
            request["params"],
            json!({"sessionId":SESSION,"toolName":"fixture-write","reason":"write approval marker"})
        );
    }
    assert_recorded_events(&client, &rows);
    client.shutdown();
}
#[test]
fn plan_review_matches_harness_choices_and_command_completions() {
    let rows = rows("plan-review");
    let mut client = Client::new(
        "plan-review",
        vec![
            json!({"answers":[{"id":"plan-review","selected":["Keep planning"],"custom":"Add rollback."}]}),
            json!({"answers":[{"id":"plan-review","selected":["Approve"]}]}),
            json!({"cancelled":true}),
        ],
        false,
    );
    let inputs = user_inputs(&rows);
    let commands: Vec<_> = rows.iter().filter(|r|r["type"] == "command/run").map(|r|json!([{"type":"text","text":format!("/{}{}",r["data"]["name"].as_str().unwrap(),r["data"]["args"].as_str().unwrap())}])).collect();
    client.run("session/prompt", inputs[0].clone());
    client.run("session/query", commands[0].clone());
    client.run("session/query", commands[1].clone());
    client.run("session/prompt", inputs[2].clone());
    client.run("session/query", commands[2].clone());
    client.run("session/prompt", inputs[3].clone());
    assert_eq!(client.reverse.len(), 3);
    let plans: Vec<_> = rows
        .iter()
        .filter(|r| r["type"] == "tool/call" && r["data"]["name"] == "exit_plan_mode")
        .map(|r| {
            let args: Value =
                serde_json::from_str(r["data"]["arguments"].as_str().unwrap()).unwrap();
            (r["data"]["callId"].clone(), args["plan"].clone())
        })
        .collect();
    for (request, (call_id, plan)) in client.reverse.iter().zip(plans) {
        assert_eq!(request["method"], "user-question/ask");
        let q = &request["params"]["questions"][0];
        assert_eq!(q["id"], "plan-review");
        assert_eq!(q["header"], "Plan review");
        assert_eq!(q["detail"], plan);
        assert_eq!(
            q["intent"],
            json!({"kind":"plan-review","approve":"Approve","callId":call_id})
        );
        assert_eq!(q["options"][0]["label"], "Approve");
        assert_eq!(q["options"][1]["label"], "Keep planning");
    }
    assert_recorded_events(&client, &rows);
    let events = client.events();
    assert_eq!(
        events.iter().filter(|e| e["type"] == "turn/start").count(),
        4
    );
    assert_eq!(
        events.iter().filter(|e| e["type"] == "step/start").count(),
        6
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e["type"] == "plan/mode")
            .map(|e| e["data"]["active"].clone())
            .collect::<Vec<_>>(),
        vec![json!(true), json!(false), json!(true), json!(false)]
    );
    let completions: Vec<_> = client
        .notifications()
        .into_iter()
        .filter(|n| n["method"] == "session.query.completed")
        .collect();
    assert_eq!(completions.len(), 3);
    assert_eq!(
        completions[0]["params"]["finalResponse"],
        "SDK_PLAN_APPROVED"
    );
    for (index, completion) in completions.iter().enumerate() {
        assert_eq!(
            completion["params"]["commandId"],
            format!("{{{{command:{}}}}}", index + 1)
        );
        assert_eq!(completion["params"]["finishReason"], "completed");
    }
    // Every command's durable command/done notification precedes its query response.
    for frame in &client.frames {
        if frame["result"]["kind"] == "command" {
            let id = &frame["result"]["execution"]["commandId"];
            let done = client
                .frames
                .iter()
                .position(|f| {
                    f["params"]["event"]["type"] == "command/done"
                        && f["params"]["event"]["data"]["commandId"] == *id
                })
                .unwrap();
            let response = client
                .frames
                .iter()
                .position(|f| std::ptr::eq(f, frame))
                .unwrap();
            assert!(done < response);
        }
    }
    client.shutdown();
}
#[test]
fn every_hoh_method_uses_fixture_result() {
    let mut client = Client::new("text-turn", vec![], false);
    for method in hoh_protocol::REQUEST_METHODS
        .iter()
        .filter(|m| m.starts_with("hoh/"))
    {
        let stem = method.replace('/', "-");
        let request: Value = serde_json::from_str(
            &fs::read_to_string(root().join(format!("protocol/fixtures/{stem}.request.json")))
                .unwrap(),
        )
        .unwrap();
        let response: Value = serde_json::from_str(
            &fs::read_to_string(root().join(format!("protocol/fixtures/{stem}.response.json")))
                .unwrap(),
        )
        .unwrap();
        let reply = client.call(method, request["params"].clone());
        if hoh_protocol::requires_host_approval(method, &request["params"]) {
            assert!(reply.get("error").is_some());
        } else {
            assert_eq!(reply["result"], response["result"]);
        }
    }
    client.shutdown();
}
#[test]
fn host_approval_mac_is_required_and_valid_mac_replays() {
    let rows = rows("approval");
    let mut client = Client::new(
        "approval",
        vec![
            json!({"outcome":"allowed-once"}),
            json!({"outcome":"rejected"}),
        ],
        true,
    );
    for content in user_inputs(&rows) {
        client.run("session/prompt", content);
    }
    assert_recorded_events(&client, &rows);
    assert_eq!(
        client
            .notifications()
            .iter()
            .filter(|n| n["method"] == "approval.resolved")
            .count(),
        2
    );
    client.shutdown();

    for failure in [
        "missing-mac",
        "invalid-mac",
        "invalid-rejection",
        "signed-rejection",
    ] {
        let mut client = Client::new("approval", vec![], true);
        client.send(framing::request(
            json!("prompt"),
            "session/prompt",
            json!({"sessionId":SESSION,"contentBlocks":user_inputs(&rows)[0]}),
        ));
        let request = loop {
            let f = client.receive();
            if f["method"] == "approval/request" {
                break f;
            }
        };
        let mut answer = json!({"outcome":"allowed-once"});
        if failure != "missing-mac" {
            let params = &request["params"];
            answer["approvalId"] = params["approvalId"].clone();
            answer["mac"] = json!("f".repeat(64));
            if failure == "invalid-rejection" || failure == "signed-rejection" {
                answer["outcome"] = json!("rejected");
            }
            if failure == "signed-rejection" {
                answer["mac"] = json!(approval::mac(
                    &[0x42; 32],
                    &serde_json::from_value(params.clone()).unwrap(),
                    "rejected"
                ));
            }
        }
        client.send(framing::success(request["id"].clone(), answer));
        let wait = client.call("session/wait", json!({"sessionId":SESSION}));
        if failure == "signed-rejection" {
            assert_eq!(wait["result"], json!({}));
        } else {
            assert!(wait.get("error").is_some());
        }
        assert!(
            !client
                .events()
                .iter()
                .any(|e| e["type"] == "approval/decided")
        );
        assert!(client.events().iter().any(|e| e["data"]["reason"]["kind"]
            == if failure == "signed-rejection" {
                "aborted"
            } else {
                "error"
            }));
        assert!(
            client
                .notifications()
                .iter()
                .any(|n| n["method"] == "approval.resolved"
                    && n["params"]["outcome"]
                        == if failure == "signed-rejection" {
                            "rejected"
                        } else {
                            "cancelled"
                        })
        );
        client.shutdown();
    }
}
#[test]
fn cancel_pending_question_ignores_late_answer_and_shutdown_does_not_hang() {
    let rows = rows("plan-review");
    let mut client = Client::new("plan-review", vec![], false);
    client.run("session/prompt", user_inputs(&rows)[0].clone());
    let command = rows.iter().find(|r| r["type"] == "command/run").unwrap();
    client.send(framing::request(json!("command"),"session/query",json!({"sessionId":SESSION,"contentBlocks":[{"type":"text","text":format!("/plan{}",command["data"]["args"].as_str().unwrap())}]})));
    let reverse = loop {
        let f = client.receive();
        if f["method"] == "user-question/ask" {
            break f;
        }
    };
    assert_eq!(
        client.call("session/cancel", json!({"sessionId":SESSION}))["result"],
        json!({})
    );
    assert!(
        client
            .notifications()
            .iter()
            .any(|n| n["method"] == "user-question.cancelled")
    );
    client.send(framing::success(
        reverse["id"].clone(),
        json!({"answers":[{"id":"plan-review","selected":["Approve"]}]}),
    ));
    assert_eq!(
        client.call("session/wait", json!({"sessionId":SESSION}))["result"],
        json!({})
    );
    client.shutdown();
}
