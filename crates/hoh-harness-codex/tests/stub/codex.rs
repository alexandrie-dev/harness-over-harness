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
            json!({"method":"item/reasoning/summaryTextDelta","params":{"threadId":"thread-stub","turnId":"turn-stub","itemId":"reasoning-1","delta":"checking "}}),
        );
        send(
            json!({"method":"item/reasoning/textDelta","params":{"threadId":"thread-stub","turnId":"turn-stub","itemId":"reasoning-1","delta":"the result"}}),
        );
        send(
            json!({"method":"item/completed","params":{"threadId":"thread-stub","turnId":"turn-stub","item":{"type":"reasoning","id":"reasoning-1","summary":[{"text":"checking the result"}],"content":[]}}}),
        );
        send(
            json!({"method":"item/started","params":{"threadId":"thread-stub","turnId":"turn-stub","item":{"type":"mcpToolCall","id":"tool-1","server":"stub","tool":"inspect","arguments":{"path":"src/lib.rs"},"status":"inProgress"}}}),
        );
        send(
            json!({"method":"item/mcpToolCall/outputDelta","params":{"threadId":"thread-stub","turnId":"turn-stub","itemId":"tool-1","delta":"inspected src/lib.rs"}}),
        );
        send(
            json!({"method":"item/completed","params":{"threadId":"thread-stub","turnId":"turn-stub","item":{"type":"mcpToolCall","id":"tool-1","server":"stub","tool":"inspect","arguments":{"path":"src/lib.rs"},"status":"completed","output":"inspected src/lib.rs"}}}),
        );
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
    let mut duplicate_after_answer: Option<Value> = None;
    let mut turn_count = 0_u8;
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
                let returned = if scenario == "resume-mismatch" {
                    "different-thread"
                } else {
                    thread
                };
                send(json!({"id":id,"result":{"thread":{"id":returned}}}));
            }
            Some("turn/start") => {
                turn_count += 1;
                if scenario == "stale-resolved" || scenario == "cross-turn-request-id" {
                    let turn = if turn_count == 1 {
                        "resolved-first"
                    } else {
                        "resolved-second"
                    };
                    send(json!({"id":id,"result":{"turn":{"id":turn,"status":"inProgress","startedAt":1}}}));
                    send(json!({"method":"turn/started","params":{"threadId":"thread-stub","turn":{"id":turn,"status":"inProgress","startedAt":1}}}));
                    if turn_count == 1 {
                        send(json!({"method":"item/started","params":{"threadId":"thread-stub","turnId":"resolved-first","item":{"type":"commandExecution","id":"cmd-resolved","command":"printf resolved","cwd":"/work","status":"inProgress","aggregatedOutput":""}}}));
                        send(json!({"id":"resolved-old","method":"item/commandExecution/requestApproval","params":{"threadId":"thread-stub","turnId":"resolved-first","itemId":"cmd-resolved","command":"printf resolved","cwd":"/work","reason":"old request"}}));
                        send(json!({"method":"serverRequest/resolved","params":{"threadId":"thread-stub","requestId":"resolved-old"}}));
                        send(json!({"method":"item/completed","params":{"threadId":"thread-stub","turnId":"resolved-first","item":{"type":"commandExecution","id":"cmd-resolved","command":"printf resolved","cwd":"/work","status":"completed","aggregatedOutput":"resolved","exitCode":0}}}));
                        send(json!({"method":"turn/completed","params":{"threadId":"thread-stub","turn":{"id":"resolved-first","status":"completed"}}}));
                    } else if scenario == "stale-resolved" {
                        send(json!({"method":"serverRequest/resolved","params":{"threadId":"thread-stub","requestId":"resolved-old"}}));
                    } else {
                        send(json!({"method":"item/started","params":{"threadId":"thread-stub","turnId":"resolved-second","item":{"type":"commandExecution","id":"cmd-current","command":"printf current","cwd":"/work","status":"inProgress","aggregatedOutput":""}}}));
                        send(json!({"id":"resolved-old","method":"item/commandExecution/requestApproval","params":{"threadId":"thread-stub","turnId":"resolved-second","itemId":"cmd-current","command":"printf current","cwd":"/work","reason":"reused native id"}}));
                    }
                    continue;
                }
                if scenario == "stale-prior-turn" {
                    let turn = if turn_count == 1 { "turn-first" } else { "turn-second" };
                    send(json!({"id":id,"result":{"turn":{"id":turn,"status":"inProgress","startedAt":1}}}));
                    send(json!({"method":"turn/started","params":{"threadId":"thread-stub","turn":{"id":turn,"status":"inProgress","startedAt":1}}}));
                    if turn_count == 1 {
                        send(json!({"method":"item/started","params":{"threadId":"thread-stub","turnId":"turn-first","item":{"type":"commandExecution","id":"cmd-reused","command":"printf old","cwd":"/old","status":"inProgress","aggregatedOutput":""}}}));
                        send(json!({"method":"item/completed","params":{"threadId":"thread-stub","turnId":"turn-first","item":{"type":"commandExecution","id":"cmd-reused","command":"printf old","cwd":"/old","status":"completed","aggregatedOutput":"old","exitCode":0}}}));
                        send(json!({"method":"turn/completed","params":{"threadId":"thread-stub","turn":{"id":"turn-first","status":"completed"}}}));
                    } else {
                        send(json!({"method":"item/started","params":{"threadId":"thread-stub","turnId":"turn-second","item":{"type":"commandExecution","id":"cmd-reused","command":"printf current","cwd":"/current","status":"inProgress","aggregatedOutput":""}}}));
                        send(json!({"id":"stale-approval","method":"item/commandExecution/requestApproval","params":{"threadId":"thread-stub","turnId":"turn-first","itemId":"cmd-reused","command":"printf old","cwd":"/old","reason":"old turn action"}}));
                    }
                    continue;
                }
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
                } else if scenario == "duplicate-string" || scenario == "duplicate-number" {
                    let duplicate_id = if scenario == "duplicate-string" {
                        json!("duplicate-approval")
                    } else {
                        json!(404)
                    };
                    log(&json!({"stubEvent":"first-duplicate-action","id":duplicate_id.clone()}));
                    send(json!({"id":duplicate_id.clone(),"method":"item/commandExecution/requestApproval","params":{"threadId":"thread-stub","turnId":"turn-stub","itemId":"cmd-1","approvalId":"first","command":"printf first","cwd":"/work","reason":"first action"}}));
                    duplicate_after_answer = Some(duplicate_id);
                } else if scenario == "foreign-thread-request" {
                    let spawned = Command::new("sh")
                        .args(["-c", "sleep 120"])
                        .spawn()
                        .unwrap();
                    log(&json!({"childPid":spawned.id()}));
                    child = Some(spawned);
                    send(json!({"id":"foreign-approval","method":"item/commandExecution/requestApproval","params":{"threadId":"thread-foreign","turnId":"turn-stub","itemId":"cmd-1","command":"printf foreign","cwd":"/work","reason":"foreign thread"}}));
                } else if scenario == "foreign-resolved" {
                    send(json!({"method":"serverRequest/resolved","params":{"threadId":"thread-foreign","requestId":"ghost"}}));
                } else if scenario == "unknown-resolved" {
                    send(json!({"method":"serverRequest/resolved","params":{"threadId":"thread-stub","requestId":"ghost"}}));
                } else if scenario == "untyped-resolved" {
                    send(json!({"method":"serverRequest/resolved","params":{"threadId":"thread-stub","requestId":{"not":"an id"}}}));
                } else if scenario == "canonical-resolved" {
                    send(json!({"id":"canonical-resolved","method":"item/commandExecution/requestApproval","params":{"threadId":"thread-stub","turnId":"turn-stub","itemId":"cmd-1","command":"printf hello","cwd":"/work","reason":"canonical resolved"}}));
                    send(json!({"method":"serverRequest/resolved","params":{"threadId":"thread-stub","requestId":"canonical-resolved"}}));
                } else if scenario == "mcp-no-active" {
                    finish_turn("completed");
                    send(json!({"id":"mcp-no-active","method":"mcpServer/elicitation/request","params":{"serverName":"fixture","threadId":"thread-stub","message":"No active turn","mode":"form","requestedSchema":{"type":"object","properties":{}}}}));
                } else if scenario == "mcp-omitted" {
                    send(json!({"id":"mcp-omitted","method":"mcpServer/elicitation/request","params":{"serverName":"fixture","threadId":"thread-stub","message":"Allow fixture?","mode":"form","requestedSchema":{"type":"object","properties":{}}}}));
                } else if scenario == "mcp-null" {
                    send(json!({"id":909,"method":"mcpServer/elicitation/request","params":{"serverName":"fixture","threadId":"thread-stub","turnId":null,"message":"Allow fixture?","mode":"form","requestedSchema":{"type":"object","properties":{}}}}));
                } else if scenario == "mcp-foreign" {
                    send(json!({"id":"mcp-foreign","method":"mcpServer/elicitation/request","params":{"serverName":"fixture","threadId":"thread-foreign","message":"Foreign fixture?","mode":"form","requestedSchema":{"type":"object","properties":{}}}}));
                } else if scenario == "mcp-stale-turn" {
                    send(json!({"id":"mcp-stale","method":"mcpServer/elicitation/request","params":{"serverName":"fixture","threadId":"thread-stub","turnId":"turn-old","message":"Old fixture?","mode":"form","requestedSchema":{"type":"object","properties":{}}}}));
                } else if scenario == "reused-item" {
                    send(json!({"method":"item/started","params":{"threadId":"thread-stub","turnId":"turn-stub","item":{"type":"commandExecution","id":"cmd-1","command":"printf replacement","cwd":"/other","status":"inProgress","aggregatedOutput":""}}}));
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
            None
                if duplicate_after_answer
                    .as_ref()
                    .is_some_and(|duplicate| message.get("id") == Some(duplicate)) =>
            {
                let duplicate_id = duplicate_after_answer.take().unwrap();
                log(&json!({"stubEvent":"second-duplicate-action","id":duplicate_id.clone()}));
                send(json!({"method":"item/started","params":{"threadId":"thread-stub","turnId":"turn-stub","item":{"type":"fileChange","id":"file-duplicate","status":"inProgress","changes":[{"path":"second.txt","kind":"update","diff":"+second"}]}}}));
                send(json!({"id":duplicate_id,"method":"item/fileChange/requestApproval","params":{"threadId":"thread-stub","turnId":"turn-stub","itemId":"file-duplicate","reason":"second action"}}));
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
            None
                if message.get("id") == Some(&json!("mcp-omitted"))
                    || message.get("id") == Some(&json!(909)) =>
            {
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
