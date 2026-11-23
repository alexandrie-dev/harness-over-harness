use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command};

use serde_json::{Value, json};

fn send(value: Value) {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{value}").unwrap();
    stdout.flush().unwrap();
}

fn log(value: &Value) {
    let Some(path) = std::env::var_os("CODEX_STUB_LOG") else {
        return;
    };
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(file, "{value}").unwrap();
}

fn finish_turn(status: &str) {
    if status == "completed" {
        send(
            json!({"method":"item/commandExecution/outputDelta","params":{"threadId":"thread-stub","turnId":"turn-stub","itemId":"cmd-1","delta":"hello\n"}}),
        );
        send(
            json!({"method":"item/completed","params":{"threadId":"thread-stub","turnId":"turn-stub","item":{"type":"commandExecution","id":"cmd-1","command":"printf hello","cwd":"/work","status":"completed","aggregatedOutput":"hello\n","exitCode":0}}}),
        );
        send(
            json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-stub","turnId":"turn-stub","itemId":"message-1","delta":"part "}}),
        );
        send(
            json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-stub","turnId":"turn-stub","itemId":"message-1","delta":"done"}}),
        );
        send(
            json!({"method":"item/completed","params":{"threadId":"thread-stub","turnId":"turn-stub","item":{"type":"agentMessage","id":"message-1","text":"part done","phase":"final_answer"}}}),
        );
    }
    send(
        json!({"method":"turn/completed","params":{"threadId":"thread-stub","turn":{"id":"turn-stub","status":status}}}),
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--version") {
        println!(
            "{}",
            std::env::var("CODEX_STUB_VERSION").unwrap_or_else(|_| "codex-cli 0.166.0".into())
        );
        return;
    }
    assert_eq!(args.get(1).map(String::as_str), Some("app-server"));
    let scenario = std::env::var("CODEX_STUB_SCENARIO").unwrap_or_else(|_| "stream".into());
    let stdin = std::io::stdin();
    let mut approval = 0_u8;
    let mut child: Option<Child> = None;
    for line in BufReader::new(stdin.lock()).lines() {
        let line = line.unwrap();
        let message: Value = serde_json::from_str(&line).unwrap();
        log(&message);
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id").cloned();
        match method {
            Some("initialize") => send(
                json!({"id":id,"result":{"serverInfo":{"name":"codex-stub","version":"0.166.0"}}}),
            ),
            Some("initialized") => {}
            Some("model/list") => send(json!({"id":id,"result":{"data":[
                {"id":"gpt-stub","model":"gpt-stub","displayName":"GPT Stub","hidden":false,"isDefault":true,"defaultReasoningEffort":"medium","supportedReasoningEfforts":[{"reasoningEffort":"low","description":"Fast"},{"reasoningEffort":"medium","description":"Balanced"},{"reasoningEffort":"high","description":"Deep"}]},
                {"id":"hidden","model":"hidden","displayName":"Hidden","hidden":true,"isDefault":false,"defaultReasoningEffort":"medium","supportedReasoningEfforts":[]}
            ],"nextCursor":null}})),
            Some("account/read") => send(
                json!({"id":id,"result":{"account":{"type":"chatgpt","email":"founder@example.test","planType":"pro"},"requiresOpenaiAuth":true}}),
            ),
            Some("thread/start") => send(json!({"id":id,"result":{"thread":{"id":"thread-stub"}}})),
            Some("thread/resume") => {
                let thread = message
                    .pointer("/params/threadId")
                    .and_then(Value::as_str)
                    .unwrap();
                send(json!({"id":id,"result":{"thread":{"id":thread}}}));
            }
            Some("turn/start") => {
                send(
                    json!({"id":id,"result":{"turn":{"id":"turn-stub","status":"inProgress","startedAt":1}}}),
                );
                send(
                    json!({"method":"turn/started","params":{"threadId":"thread-stub","turn":{"id":"turn-stub","status":"inProgress","startedAt":1}}}),
                );
                send(
                    json!({"method":"item/started","params":{"threadId":"thread-stub","turnId":"turn-stub","item":{"type":"commandExecution","id":"cmd-1","command":"printf hello","cwd":"/work","status":"inProgress","aggregatedOutput":""}}}),
                );
                if scenario == "interrupt" {
                    let spawned = Command::new("sh")
                        .args(["-c", "sleep 120"])
                        .spawn()
                        .unwrap();
                    log(&json!({"childPid":spawned.id()}));
                    child = Some(spawned);
                } else {
                    send(
                        json!({"id":"approval-command","method":"item/commandExecution/requestApproval","params":{"threadId":"thread-stub","turnId":"turn-stub","itemId":"cmd-1","approvalId":"approval-command","command":"printf hello","cwd":"/work","reason":"run it"}}),
                    );
                }
            }
            Some("turn/interrupt") => {
                send(json!({"id":id,"result":{}}));
                finish_turn("interrupted");
            }
            None if message.get("id") == Some(&json!("approval-command")) => {
                approval = 1;
                if scenario == "approvals" {
                    send(
                        json!({"method":"item/started","params":{"threadId":"thread-stub","turnId":"turn-stub","item":{"type":"fileChange","id":"file-1","status":"inProgress","changes":[{"path":"src/lib.rs","kind":"update","diff":"+ok"}]}}}),
                    );
                    send(
                        json!({"id":71,"method":"item/fileChange/requestApproval","params":{"threadId":"thread-stub","turnId":"turn-stub","itemId":"file-1","reason":"edit it"}}),
                    );
                } else {
                    finish_turn("completed");
                }
            }
            None if message.get("id") == Some(&json!(71)) => {
                approval = 2;
                send(
                    json!({"method":"item/completed","params":{"threadId":"thread-stub","turnId":"turn-stub","item":{"type":"fileChange","id":"file-1","status":"declined","changes":[{"path":"src/lib.rs","kind":"update","diff":"+ok"}]}}}),
                );
                finish_turn("completed");
            }
            _ => {}
        }
    }
    let _ = approval;
    // Deliberately do not kill the child: the harness supervisor must remove the whole tree.
    if let Some(mut child) = child.take() {
        let _ = child.wait();
    }
}
