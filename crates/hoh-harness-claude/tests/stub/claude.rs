//! A stand-in for the `claude` CLI: speaks its stream-json control protocol from canned
//! scenarios. The real CLI is never run by the tests.
//!
//! Environment: `STUB_VERSION` (answer of `--version`), `STUB_STATE` (folder of known sessions),
//! `STUB_ARGS_FILE` (where the argv of the last start is written), `STUB_MODE` (`garbage-start`,
//! `crash-start`, `silent`, `init-error`, `signed-out`, `no-skills` — `reload_skills` is not known).
//! `initialize` lists five commands and `reload_skills` two skills, some of them odd (HOH-41).
//! `rename` makes `system/init` report a new session id; the session id of each user line is
//! appended to `$STUB_STATE/prompt-sessions`.
//! Prompts: `withdraw`, `toolfail`, `subagent`, `run-reason`, `hello`, `telemetry`, `telemetry-parent`, `telemetry-late-parent`, `telemetry-tasks`, `run: <command>`, `edit: <path>`, `question`, `long`, `garbage`, `stderr`,
//! `crash`, `failure`, `write: <path>`, `kill-self` (SIGABRT), `ask-abort`, `echo-decision`,
//! `subagent-tool`, `parallel-subagents`, `unknown-request`, `withdraw-question`, `leave-open`, `subagent-stream`,
//! `truncate`, `read: <path>`, `close-stdin`, `notebook: <path>`, `login-expired`, `forbidden`, `question-body`, `question-odd`, `question-bare`, `two-open`, `silent-interrupt`; STUB_MODE `version-fails`, `root-refusal`, `secret-start`; prompts `protected: <path>`, `dangerous-rm`, `secret-crash`.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::mpsc;

use serde_json::{Value, json};

fn write_line(value: &Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

struct Stub {
    rx: mpsc::Receiver<Value>,
    session: String,
    next: u64,
    /// `bypassPermissions`: the CLI approves by itself what it can; only what even bypass
    /// refuses (protected paths, removal safety) is asked, by the scenarios that say so.
    no_callback: bool,
    /// The uuid the last prompt came with.
    prompt_uuid: Value,
    /// The real personal-profile session sees project commands and skills; the safe pre-turn
    /// probe (`user`) and the isolated profile do not.
    project_sources: bool,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--version") {
        println!(
            "{}",
            env("STUB_VERSION").unwrap_or_else(|| "2.0.31 (Claude Code)".into())
        );
        if env("STUB_MODE").as_deref() == Some("version-fails") {
            std::process::exit(1);
        }
        return;
    }
    if let Some(file) = env("STUB_ARGS_FILE") {
        let _ = std::fs::write(file, args.join("\n"));
    }
    let value_of = |flag: &str| {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|at| args.get(at + 1).cloned())
    };
    let persist = !args.iter().any(|arg| arg == "--no-session-persistence");
    let state = env("STUB_STATE").map(PathBuf::from);
    let mut session = value_of("--session-id").unwrap_or_default();
    if let Some(id) = value_of("--resume") {
        let known = state.as_ref().is_some_and(|dir| dir.join(&id).exists());
        if !known {
            eprintln!("No conversation found with session ID: {id}");
            std::process::exit(1);
        }
        session = id;
    } else if persist && let Some(dir) = &state {
        let _ = std::fs::write(dir.join(&session), b"");
    }
    match env("STUB_MODE").as_deref() {
        Some("garbage-start") => {
            println!("Welcome to the stub, this is not JSON");
            std::thread::sleep(std::time::Duration::from_secs(5));
            return;
        }
        Some("root-refusal") => {
            eprintln!(
                "--dangerously-skip-permissions cannot be used with root/sudo privileges for security reasons"
            );
            std::process::exit(1);
        }
        Some("secret-start") => {
            eprintln!(
                "boot failure ANTHROPIC_API_KEY=sk-ant-api03-Zx9Qw8Er7Ty6Ui5Op4As3Df2Gh1Jk0Lm"
            );
            std::process::exit(3);
        }
        Some("crash-start") => {
            eprintln!("boot failure");
            std::process::exit(3);
        }
        _ => {}
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
        session,
        next: 0,
        no_callback: value_of("--permission-mode").as_deref() == Some("bypassPermissions"),
        prompt_uuid: Value::Null,
        project_sources: value_of("--setting-sources")
            .is_some_and(|sources| sources.split(',').any(|source| source == "project")),
    };
    while let Ok(message) = stub.rx.recv() {
        stub.handle(&message);
    }
}

impl Stub {
    fn id(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}-{}", self.next)
    }

    fn ok(&self, request_id: &str, response: Value) {
        write_line(&json!({
            "type": "control_response",
            "response": { "subtype": "success", "request_id": request_id, "response": response },
        }));
    }

    fn handle(&mut self, message: &Value) {
        match message["type"].as_str() {
            Some("control_request") => {
                let id = message["request_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                match message["request"]["subtype"].as_str() {
                    Some("initialize") => self.initialize(&id),
                    Some("interrupt") => self.ok(&id, json!({})),
                    Some("reload_skills") if env("STUB_MODE").as_deref() == Some("no-skills") => {
                        write_line(&json!({
                            "type": "control_response",
                            "response": { "subtype": "error", "request_id": id, "error": "Unknown subtype: reload_skills" },
                        }));
                    }
                    Some("reload_skills") => {
                        let mut skills = vec![
                            json!({ "name": "release-notes", "description": "Write the release notes", "argumentHint": "" }),
                            json!({ "name": "  ", "description": "nameless" }),
                        ];
                        if self.project_sources {
                            skills.push(json!({ "name": "project-skill", "description": "From .claude/skills" }));
                        }
                        self.ok(&id, json!({ "skills": skills }));
                    }
                    _ => {}
                }
            }
            Some("user") => {
                if let Some(dir) = env("STUB_STATE") {
                    let mut log = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(PathBuf::from(dir).join("prompt-sessions"))
                        .unwrap();
                    let _ = writeln!(
                        log,
                        "{}",
                        message["session_id"].as_str().unwrap_or_default()
                    );
                }
                let text = message["message"]["content"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                self.prompt_uuid = message["uuid"].clone();
                self.turn(&text);
            }
            _ => {}
        }
    }

    fn initialize(&mut self, id: &str) {
        match env("STUB_MODE").as_deref() {
            Some("silent") => {}
            Some("init-error") => write_line(&json!({
                "type": "control_response",
                "response": { "subtype": "error", "request_id": id, "error": "Unknown subtype: initialize" },
            })),
            mode => {
                let account = if mode == Some("signed-out") {
                    json!({ "tokenSource": "none", "apiKeySource": "none" })
                } else {
                    json!({ "email": "user@example.com", "subscriptionType": "max", "tokenSource": "claude.ai" })
                };
                let mut commands = vec![
                    json!({ "name": "review", "description": "Review a pull request", "argumentHint": "[pr]" }),
                    json!({ "name": "Review", "description": "again", "argumentHint": "[other]" }),
                    json!({ "name": "init", "description": " " }),
                    json!({ "name": "", "description": "nameless" }),
                    json!({ "name": "release-notes", "description": "Write the release notes" }),
                ];
                if self.project_sources {
                    commands.push(json!({ "name": "project-command", "description": "From .claude/commands" }));
                }
                self.ok(
                    id,
                    json!({
                        "commands": commands,
                        "output_style": "default",
                        "models": [
                            { "value": "default", "displayName": "Default", "description": "x" },
                            { "value": "sonnet", "displayName": "Sonnet", "description": "x" },
                            { "value": "opus", "displayName": "Opus", "description": "x" }
                        ],
                        "account": account,
                    }),
                );
            }
        }
    }

    fn system_init(&mut self) {
        if env("STUB_MODE").as_deref() == Some("rename") {
            self.session = "renamed-session".into();
        }
        write_line(&json!({
            "type": "system", "subtype": "init", "session_id": self.session,
            "model": "stub", "permissionMode": "default", "cwd": "/",
            "tools": ["Bash"], "claude_code_version": "2.0.31",
        }));
    }

    fn stream(&self, event: Value) {
        write_line(&json!({
            "type": "stream_event", "session_id": self.session, "parent_tool_use_id": null,
            "event": event,
        }));
    }

    fn assistant(&self, message_id: &str, block: Value) {
        write_line(&json!({
            "type": "assistant", "session_id": self.session, "parent_tool_use_id": null,
            "message": { "id": message_id, "role": "assistant", "content": [block] },
        }));
    }

    fn tool_result(&self, tool_id: &str, content: &str, is_error: bool) {
        write_line(&json!({
            "type": "user", "session_id": self.session, "parent_tool_use_id": null,
            "message": { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": tool_id, "content": content, "is_error": is_error }
            ] },
        }));
    }

    fn result(&self, subtype: &str, is_error: bool, text: &str) {
        write_line(&json!({
            "type": "result", "subtype": subtype, "is_error": is_error, "result": text,
            "session_id": self.session, "duration_ms": 1, "num_turns": 1,
        }));
    }

    fn say(&self, message_id: &str, index: u64, parts: &[&str]) {
        self.stream(json!({ "type": "content_block_start", "index": index, "content_block": { "type": "text", "text": "" } }));
        for part in parts {
            self.stream(json!({ "type": "content_block_delta", "index": index, "delta": { "type": "text_delta", "text": part } }));
        }
        self.stream(json!({ "type": "content_block_stop", "index": index }));
        self.assistant(
            message_id,
            json!({ "type": "text", "text": parts.concat() }),
        );
    }

    fn start_message(&self, message_id: &str) {
        self.stream(json!({ "type": "message_start", "message": { "id": message_id, "role": "assistant" } }));
    }

    /// Ask for permission and wait for the answer; `true` when allowed.
    fn ask(&mut self, tool: &str, tool_id: &str, input: &Value) -> Option<Value> {
        if self.no_callback {
            return Some(json!({ "behavior": "allow", "updatedInput": input }));
        }
        let request = self.id("perm");
        write_line(&json!({
            "type": "control_request", "request_id": request,
            "request": { "subtype": "can_use_tool", "tool_name": tool, "input": input, "tool_use_id": tool_id },
        }));
        loop {
            let message = self.rx.recv().ok()?;
            if message["type"] == "control_response"
                && message["response"]["request_id"] == request.as_str()
            {
                return Some(message["response"]["response"].clone());
            }
        }
    }

    fn turn(&mut self, text: &str) {
        if text == "wake-first" {
            // A CLI resumed after its background command was lost answers that command's
            // notification first: a result of no turn, then the prompt's own.
            write_line(&json!({
                "type": "result", "subtype": "success", "is_error": false, "result": "",
                "num_turns": 0, "origin": { "kind": "task-notification" },
                "uuid": "wake-result-telemetry", "session_id": self.session,
                "usage": { "input_tokens": 1 }, "modelUsage": {},
            }));
            self.system_init();
            let message_id = self.id("msg");
            self.start_message(&message_id);
            self.say(&message_id, 0, &["Hello"]);
            self.result("success", false, "Hello");
            return;
        }
        if text == "wake-turn-first" {
            // The CLI takes the prompt (it echoes prompts), runs a turn of its own first, then the
            // prompt's; only the second result names the prompt.
            write_line(&json!({
                "type": "command_lifecycle", "command_uuid": self.prompt_uuid,
                "state": "queued", "session_id": self.session,
            }));
            self.system_init();
            let wake = self.id("msg");
            self.start_message(&wake);
            write_line(&json!({
                "type": "result", "subtype": "success", "is_error": false,
                "result": "The background command ended.", "num_turns": 1,
                "origin": { "kind": "task-notification" }, "session_id": self.session,
                "uuid": "wake-turn-result-telemetry", "usage": { "input_tokens": 2 },
                "modelUsage": {},
            }));
            write_line(&json!({
                "type": "result", "subtype": "success", "is_error": false, "result": "other",
                "num_turns": 1, "user_message_uuids": ["00000000-0000-4000-8000-0000000000aa"],
                "session_id": self.session,
            }));
            self.system_init();
            let message_id = self.id("msg");
            self.start_message(&message_id);
            self.say(&message_id, 0, &["Hello"]);
            write_line(&json!({
                "type": "result", "subtype": "success", "is_error": false, "result": "Hello",
                "num_turns": 1, "user_message_uuids": [self.prompt_uuid],
                "session_id": self.session,
            }));
            return;
        }
        if text == "ask-first" {
            // The very first line of the turn is the permission request (nothing before it).
            let request = self.id("perm");
            write_line(&json!({
                "type": "control_request", "request_id": request,
                "request": { "subtype": "can_use_tool", "tool_name": "Bash",
                    "input": { "command": "ls" }, "tool_use_id": "toolu_first" },
            }));
            while let Ok(message) = self.rx.recv() {
                if message["type"] == "control_response" {
                    break;
                }
            }
            self.result("success", false, "done");
            return;
        }
        if text == "telemetry-parent" {
            write_line(&json!({
                "type": "assistant", "session_id": self.session, "parent_tool_use_id": null,
                "message": { "id": "parent-message", "role": "assistant",
                    "usage": { "input_tokens": 3 }, "content": [{
                        "type": "tool_use", "id": "toolu-old-parent", "name": "Agent",
                        "input": { "description": "background child" }
                    }] }
            }));
            self.result("success", false, "parent started");
            return;
        }
        if text == "telemetry-late-parent" {
            for (id, parent, tokens) in [
                ("late-known", "toolu-old-parent", 4),
                ("late-unknown", "toolu-never-seen", 5),
            ] {
                write_line(&json!({
                    "type": "assistant", "session_id": self.session,
                    "parent_tool_use_id": parent,
                    "message": { "id": id, "role": "assistant",
                        "usage": { "input_tokens": tokens }, "content": [] }
                }));
            }
            self.result("success", false, "late child settled");
            return;
        }
        if text == "telemetry-tasks" {
            for frame in [
                json!({ "type": "system", "subtype": "task_started",
                    "session_id": self.session, "uuid": "run-1-start", "task_id": "same-task",
                    "run_id": "run-1", "description": "first run" }),
                json!({ "type": "system", "subtype": "task_notification",
                    "session_id": self.session, "uuid": "run-1-end", "task_id": "same-task",
                    "run_id": "run-1", "status": "completed" }),
                json!({ "type": "system", "subtype": "task_started",
                    "session_id": self.session, "uuid": "run-2-start", "task_id": "same-task",
                    "run_id": "run-2", "description": "resumed run" }),
                // A late frame of the old run remains distinguishable, not reconciled here.
                json!({ "type": "system", "subtype": "task_updated",
                    "session_id": self.session, "uuid": "run-1-late", "task_id": "same-task",
                    "run_id": "run-1", "patch": { "status": "completed", "end_time": 17 } }),
                json!({ "type": "system", "subtype": "task_updated",
                    "session_id": self.session, "uuid": "run-2-foreground", "task_id": "same-task",
                    "run_id": "run-2", "patch": { "status": "running", "is_backgrounded": false } }),
                json!({ "type": "system", "subtype": "task_updated",
                    "session_id": self.session, "uuid": "run-2-background", "task_id": "same-task",
                    "run_id": "run-2", "patch": { "background": true } }),
            ] {
                write_line(&frame);
            }
            self.result("success", false, "tasks recorded");
            return;
        }
        if text == "telemetry" {
            write_line(&json!({
                "type": "system", "subtype": "init", "session_id": self.session,
                "uuid": "init-telemetry", "model": "stub", "permissionMode": "default",
                "mcp_servers": [
                    { "name": "codex", "status": "connected", "command": "secret-command",
                      "env": { "TOKEN": "secret-token" } },
                    { "name": "offline", "status": "failed", "args": ["secret-arg"] }
                ]
            }));
            write_line(&json!({
                "type": "assistant", "uuid": "assistant-telemetry",
                "session_id": self.session, "parent_tool_use_id": null,
                "message": { "id": "msg-telemetry", "role": "assistant",
                    "usage": { "input_tokens": 11, "output_tokens": 3,
                        "cache_read_input_tokens": 5 },
                    "content": [{ "type": "text", "text": "telemetry done" }] }
            }));
            for tokens in [1, 2] {
                write_line(&json!({
                    "type": "assistant", "session_id": self.session, "parent_tool_use_id": null,
                    "message": { "role": "assistant", "usage": { "input_tokens": tokens },
                        "content": [] }
                }));
            }
            write_line(&json!({
                "type": "system", "subtype": "task_started", "session_id": self.session,
                "uuid": "task-start-telemetry", "task_id": "task-1", "run_id": "run-main",
                "tool_use_id": "toolu-background", "task_type": "local_bash",
                "description": "Background build", "is_backgrounded": true
            }));
            write_line(&json!({
                "type": "system", "subtype": "task_progress", "session_id": self.session,
                "uuid": "task-progress-telemetry", "task_id": "task-1", "run_id": "run-main",
                "tool_use_id": "toolu-background", "task_type": "local_workflow",
                "description": "Build phase", "workflow_progress": [
                    { "type": "workflow_phase", "index": 1, "title": "Build" },
                    { "type": "workflow_agent", "index": 1, "label": "compile",
                      "agentId": "agent-1", "state": "start", "phaseIndex": 1,
                      "phaseTitle": "Build", "model": "stub-model", "startedAt": 7 }
                ]
            }));
            write_line(&json!({
                "type": "system", "subtype": "task_notification", "session_id": self.session,
                "uuid": "task-end-telemetry", "task_id": "task-1", "run_id": "run-main",
                "tool_use_id": "toolu-background", "task_type": "local_bash",
                "status": "completed", "summary": "Build complete",
                "output_file": "/tmp/secret-output"
            }));
            write_line(&json!({
                "type": "system", "subtype": "background_tasks_changed",
                "session_id": self.session, "uuid": "tasks-mixed-telemetry", "tasks": [
                    { "task_id": "valid-task", "description": "valid" },
                    { "description": "missing id" }
                ]
            }));
            write_line(&json!({
                "type": "system", "subtype": "background_tasks_changed",
                "session_id": self.session, "uuid": "tasks-malformed-telemetry",
                "tasks": [{ "not_a_task": true }]
            }));
            write_line(&json!({
                "type": "system", "subtype": "background_tasks_changed",
                "session_id": self.session, "uuid": "tasks-empty-telemetry", "tasks": []
            }));
            write_line(&json!({
                "type": "result", "subtype": "success", "is_error": false,
                "uuid": "result-telemetry", "result": "telemetry done",
                "session_id": self.session, "duration_ms": 1, "num_turns": 1,
                "user_message_uuids": [self.prompt_uuid],
                "usage": { "input_tokens": 13, "output_tokens": 4,
                    "cache_creation_input_tokens": 2 },
                "modelUsage": { "stub-model": { "inputTokens": 20, "outputTokens": 6,
                    "cacheReadInputTokens": 5, "cacheCreationInputTokens": 2,
                    "contextWindow": 200000, "maxOutputTokens": 32000,
                    "webSearchRequests": 1, "costUSD": 0.125 } },
                "total_cost_usd": 0.25
            }));
            return;
        }
        self.system_init();
        let message_id = self.id("msg");
        self.start_message(&message_id);
        if text == "hello" {
            self.stream(json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "" } }));
            self.stream(json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "thinking_delta", "thinking": "pondering" } }));
            self.assistant(
                &message_id,
                json!({ "type": "thinking", "thinking": "pondering" }),
            );
            self.say(&message_id, 1, &["Hel", "lo"]);
            self.result("success", false, "Hello");
        } else if let Some(command) = text.strip_prefix("run: ") {
            self.tool_turn(&message_id, "Bash", &json!({ "command": command }), command);
        } else if let Some(path) = text.strip_prefix("edit: ") {
            self.tool_turn(
                &message_id,
                "Edit",
                &json!({ "file_path": path, "old_string": "a", "new_string": "b" }),
                path,
            );
        } else if text == "question" {
            let input = json!({ "questions": [{
                "question": "Which?", "header": "Pick",
                "options": [{ "label": "A", "description": "first" }, { "label": "B" }],
                "multiSelect": false,
            }] });
            let answer = self.ask("AskUserQuestion", "toolu_q", &input);
            let shown = answer
                .and_then(|a| a["updatedInput"]["answers"].as_object().cloned())
                .map(|a| Value::Object(a).to_string())
                .unwrap_or_default();
            self.say(&message_id, 0, &[&format!("answers={shown}")]);
            self.result("success", false, "done");
        } else if text == "run-reason" {
            let input = json!({ "command": "make build" });
            self.assistant(
                &message_id,
                json!({ "type": "tool_use", "id": "toolu_r", "name": "Bash", "input": input }),
            );
            let request = self.id("perm");
            write_line(&json!({
                "type": "control_request", "request_id": request,
                "request": { "subtype": "can_use_tool", "tool_name": "Bash", "input": input,
                    "tool_use_id": "toolu_r", "title": "Run build",
                    "decision_reason": "Not in allowlist" },
            }));
            while let Ok(message) = self.rx.recv() {
                if message["type"] == "control_response" {
                    break;
                }
            }
            self.result("success", false, "done");
        } else if text == "withdraw" {
            let request = self.id("perm");
            write_line(&json!({
                "type": "control_request", "request_id": request,
                "request": { "subtype": "can_use_tool", "tool_name": "Bash",
                    "input": { "command": "ls" }, "tool_use_id": "toolu_w" },
            }));
            write_line(&json!({ "type": "control_cancel_request", "request_id": request }));
            self.result("success", false, "done");
        } else if text == "toolfail" {
            self.assistant(
                &message_id,
                json!({ "type": "tool_use", "id": "toolu_f", "name": "Bash", "input": { "command": "false" } }),
            );
            self.tool_result("toolu_f", "exit 1", true);
            self.result("success", false, "done");
        } else if text == "subagent" {
            write_line(&json!({
                "type": "assistant", "session_id": self.session, "parent_tool_use_id": "toolu_p",
                "message": { "id": "sub-1", "role": "assistant",
                    "content": [{ "type": "text", "text": "hidden" }] },
            }));
            self.say(&message_id, 0, &["shown"]);
            self.result("success", false, "done");
        } else if text == "long" {
            self.assistant(
                &message_id,
                json!({ "type": "tool_use", "id": "toolu_long", "name": "Bash", "input": { "command": "sleep 100" } }),
            );
            // Waits for the interrupt, which the main loop answers; then the turn ends.
            while let Ok(message) = self.rx.recv() {
                if message["request"]["subtype"] == "interrupt" {
                    let id = message["request_id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                    self.ok(&id, json!({}));
                    write_line(&json!({
                        "type": "user", "session_id": self.session, "parent_tool_use_id": null,
                        "message": { "role": "user", "content": [
                            { "type": "tool_result", "tool_use_id": "toolu_long",
                              "content": "[Request interrupted by user]", "is_error": true }
                        ] },
                        "tool_result_meta": [{ "id": "toolu_long", "non_execution_kind": "interrupted" }],
                    }));
                    self.result("error_during_execution", true, "interrupted");
                    return;
                }
            }
        } else if let Some(path) = text.strip_prefix("write: ") {
            self.tool_turn(
                &message_id,
                "Write",
                &json!({ "file_path": path, "content": "x" }),
                path,
            );
        } else if text == "kill-self" {
            self.say(&message_id, 0, &["partial"]);
            // SIGABRT, as in the T3 test of a killed process.
            std::process::abort();
        } else if text == "ask-abort" {
            let request = self.id("perm");
            write_line(&json!({
                "type": "control_request", "request_id": request,
                "request": { "subtype": "can_use_tool", "tool_name": "Bash",
                    "input": { "command": "ls" }, "tool_use_id": "toolu_open" },
            }));
            std::thread::sleep(std::time::Duration::from_millis(300));
            std::process::exit(5);
        } else if text == "echo-decision" {
            // A permission request without `input`; the answer is shown back as text.
            let request = self.id("perm");
            write_line(&json!({
                "type": "control_request", "request_id": request,
                "request": { "subtype": "can_use_tool", "tool_name": "Bash",
                    "tool_use_id": "toolu_e" },
            }));
            let mut shown = String::new();
            while let Ok(message) = self.rx.recv() {
                if message["type"] == "control_response" {
                    shown = message["response"]["response"].to_string();
                    break;
                }
            }
            self.say(&message_id, 0, &[&shown]);
            self.result("success", false, "done");
        } else if text == "subagent-tool" {
            write_line(&json!({
                "type": "assistant", "session_id": self.session, "parent_tool_use_id": "toolu_p",
                "message": { "id": "sub-2", "role": "assistant", "content": [
                    { "type": "tool_use", "id": "toolu_sub", "name": "Bash",
                      "input": { "command": "inner" } }] },
            }));
            let request = self.id("perm");
            write_line(&json!({
                "type": "control_request", "request_id": request,
                "request": { "subtype": "can_use_tool", "tool_name": "Bash",
                    "input": { "command": "inner" }, "tool_use_id": "toolu_sub" },
            }));
            while let Ok(message) = self.rx.recv() {
                if message["type"] == "control_response" {
                    break;
                }
            }
            write_line(&json!({
                "type": "user", "session_id": self.session, "parent_tool_use_id": "toolu_p",
                "message": { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "toolu_sub", "content": "inner done" }
                ] },
            }));
            self.say(&message_id, 0, &["shown"]);
            self.result("success", false, "done");
        } else if text == "parallel-subagents" {
            write_line(&json!({
                "type": "assistant", "session_id": self.session, "parent_tool_use_id": null,
                "message": { "id": "root-agents", "role": "assistant", "content": [
                    { "type": "tool_use", "id": "toolu_alpha", "name": "Agent",
                      "input": { "description": "Alpha", "prompt": "inspect alpha" } },
                    { "type": "tool_use", "id": "toolu_beta", "name": "Agent",
                      "input": { "description": "Beta", "prompt": "inspect beta" } }
                ] },
            }));
            write_line(&json!({
                "type": "assistant", "session_id": self.session,
                "parent_tool_use_id": "toolu_alpha",
                "message": { "id": "alpha-1", "role": "assistant", "content": [
                    { "type": "text", "text": "alpha note" },
                    { "type": "tool_use", "id": "toolu_alpha_read", "name": "Read",
                      "input": { "file_path": "alpha.txt" } }
                ] },
            }));
            write_line(&json!({
                "type": "assistant", "session_id": self.session,
                "parent_tool_use_id": "toolu_beta",
                "message": { "id": "beta-1", "role": "assistant", "content": [
                    { "type": "text", "text": "beta note" },
                    { "type": "tool_use", "id": "toolu_beta_grep", "name": "Grep",
                      "input": { "pattern": "beta" } }
                ] },
            }));
            write_line(&json!({
                "type": "user", "session_id": self.session,
                "parent_tool_use_id": "toolu_beta",
                "message": { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "toolu_beta_grep",
                      "content": "beta match" }
                ] },
            }));
            write_line(&json!({
                "type": "assistant", "session_id": self.session,
                "parent_tool_use_id": "toolu_beta",
                "message": { "id": "beta-2", "role": "assistant", "content": [
                    { "type": "text", "text": "beta final" }
                ] },
            }));
            write_line(&json!({
                "type": "user", "session_id": self.session,
                "parent_tool_use_id": "toolu_alpha",
                "message": { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "toolu_alpha_read",
                      "content": "alpha contents" }
                ] },
            }));
            write_line(&json!({
                "type": "assistant", "session_id": self.session,
                "parent_tool_use_id": "toolu_alpha",
                "message": { "id": "alpha-2", "role": "assistant", "content": [
                    { "type": "text", "text": "alpha final" }
                ] },
            }));
            write_line(&json!({
                "type": "user", "session_id": self.session, "parent_tool_use_id": null,
                "message": { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "toolu_beta",
                      "content": "beta final" },
                    { "type": "tool_result", "tool_use_id": "toolu_alpha",
                      "content": "alpha final" }
                ] },
            }));
            self.say(&message_id, 0, &["combined"]);
            self.result("success", false, "done");
        } else if text == "unknown-request" {
            let request = self.id("odd");
            write_line(&json!({
                "type": "control_request", "request_id": request,
                "request": { "subtype": "mystery" },
            }));
            let mut shown = String::new();
            while let Ok(message) = self.rx.recv() {
                if message["type"] == "control_response" {
                    shown = message["response"].to_string();
                    break;
                }
            }
            self.say(&message_id, 0, &[&shown]);
            self.result("success", false, "done");
        } else if text == "withdraw-question" {
            let request = self.id("perm");
            write_line(&json!({
                "type": "control_request", "request_id": request,
                "request": { "subtype": "can_use_tool", "tool_name": "AskUserQuestion",
                    "input": { "questions": [{ "question": "Which?", "options": [] }] },
                    "tool_use_id": "toolu_wq" },
            }));
            write_line(&json!({ "type": "control_cancel_request", "request_id": request }));
            self.result("success", false, "done");
        } else if text == "leave-open" {
            let request = self.id("perm");
            write_line(&json!({
                "type": "control_request", "request_id": request,
                "request": { "subtype": "can_use_tool", "tool_name": "Bash",
                    "input": { "command": "ls" }, "tool_use_id": "toolu_lo" },
            }));
            self.result("success", false, "done");
        } else if text == "subagent-stream" {
            write_line(&json!({
                "type": "stream_event", "session_id": self.session, "parent_tool_use_id": "toolu_p",
                "event": { "type": "content_block_delta", "index": 0,
                    "delta": { "type": "text_delta", "text": "hidden-delta" } },
            }));
            self.say(&message_id, 0, &["shown"]);
            self.result("success", false, "done");
        } else if text == "truncate" {
            print!("{{\"type\": \"assist");
            let _ = std::io::stdout().flush();
            std::process::exit(0);
        } else if text == "question-body" {
            let input = json!({ "questions": [{
                "question": "Which?", "header": "Pick",
                "options": [{ "label": "A" }, { "label": "B" }], "multiSelect": false,
            }] });
            let answer = self.ask("AskUserQuestion", "toolu_qb", &input);
            let shown = answer.map(|a| a.to_string()).unwrap_or_default();
            self.say(&message_id, 0, &[&shown]);
            self.result("success", false, "done");
        } else if text == "question-odd" || text == "question-bare" {
            let input = if text == "question-odd" {
                json!({ "questions": [
                    { "question": "  ", "options": [{ "label": "A" }] },
                    { "question": " Q ", "options": [{ "label": "B" }, { "label": " " }] },
                ] })
            } else {
                json!({})
            };
            let answer = self.ask("AskUserQuestion", "toolu_qo", &input);
            let shown = answer.map(|a| a.to_string()).unwrap_or_default();
            self.say(&message_id, 0, &[&shown]);
            self.result("success", false, "done");
        } else if text == "two-open" {
            for tool in ["toolu_a", "toolu_b"] {
                self.assistant(
                    &message_id,
                    json!({ "type": "tool_use", "id": tool, "name": "Bash", "input": { "command": "ls" } }),
                );
                let request = self.id("perm");
                write_line(&json!({
                    "type": "control_request", "request_id": request,
                    "request": { "subtype": "can_use_tool", "tool_name": "Bash",
                        "input": { "command": "ls" }, "tool_use_id": tool },
                }));
            }
            while self.rx.recv().is_ok() {}
        } else if text == "silent-interrupt" {
            self.assistant(
                &message_id,
                json!({ "type": "tool_use", "id": "toolu_si", "name": "Bash", "input": { "command": "sleep 100" } }),
            );
            // Takes the interrupt, answers it, and never ends the turn.
            while let Ok(message) = self.rx.recv() {
                if message["request"]["subtype"] == "interrupt" {
                    let id = message["request_id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                    self.ok(&id, json!({}));
                }
            }
        } else if let Some(path) = text.strip_prefix("notebook: ") {
            self.tool_turn(
                &message_id,
                "NotebookEdit",
                &json!({ "notebook_path": path, "new_source": "x" }),
                path,
            );
        } else if text == "login-expired" || text == "forbidden" {
            write_line(&json!({
                "type": "assistant", "session_id": self.session, "parent_tool_use_id": null,
                "error": "authentication_failed",
                "message": { "id": "err-1", "role": "assistant",
                    "content": [{ "type": "text", "text": "Invalid API key" }] },
            }));
            let (status, result) = if text == "forbidden" {
                (
                    403,
                    "API Error: 403 Your organization has no access to this model.",
                )
            } else {
                (401, "API Error: 401 Credentials expired.")
            };
            write_line(&json!({
                "type": "result", "subtype": "success", "is_error": true, "result": result,
                "api_error_status": status, "session_id": self.session,
            }));
        } else if let Some(path) = text.strip_prefix("protected: ") {
            // What bypass still refuses: the CLI asks (a protected path).
            self.forced_turn(
                &message_id,
                "Write",
                &json!({ "file_path": path, "content": "x" }),
                "Path is protected: writes there need approval",
            );
        } else if text == "dangerous-rm" {
            self.forced_turn(
                &message_id,
                "Bash",
                &json!({ "command": "rm -rf /data" }),
                "\u{1b}[1mDangerous rm operation detected\u{1b}[22m",
            );
        } else if let Some(path) = text.strip_prefix("read: ") {
            self.tool_turn(&message_id, "Read", &json!({ "file_path": path }), path);
        } else if text == "close-stdin" {
            let request = self.id("perm");
            write_line(&json!({
                "type": "control_request", "request_id": request,
                "request": { "subtype": "can_use_tool", "tool_name": "Bash",
                    "input": { "command": "ls" }, "tool_use_id": "toolu_cs" },
            }));
            // Becomes a shell that closed its stdin and keeps running: the answer of the human
            // has nowhere to go.
            use std::os::unix::process::CommandExt;
            let error = std::process::Command::new("sh")
                .args(["-c", "exec 0<&-; exec sleep 30"])
                .exec();
            panic!("exec failed: {error}");
        } else if text == "garbage" {
            println!("<<< not a json line >>>");
        } else if text == "stderr" {
            eprintln!("stub warning: something odd");
            self.say(&message_id, 0, &["fine"]);
            self.result("success", false, "fine");
        } else if text == "secret-crash" {
            eprintln!("fatal ANTHROPIC_API_KEY=sk-ant-api03-Zx9Qw8Er7Ty6Ui5Op4As3Df2Gh1Jk0Lm");
            std::process::exit(1);
        } else if text == "crash" {
            self.say(&message_id, 0, &["partial"]);
            eprintln!("stub crashed");
            std::process::exit(3);
        } else if text == "failure" {
            self.result("error_during_execution", true, "boom");
        }
    }

    /// A tool call the CLI asks about whatever the mode (it ignores `no_callback`).
    fn forced_turn(&mut self, message_id: &str, tool: &str, input: &Value, reason: &str) {
        self.assistant(
            message_id,
            json!({ "type": "tool_use", "id": "toolu_1", "name": tool, "input": input }),
        );
        let request = self.id("perm");
        write_line(&json!({
            "type": "control_request", "request_id": request,
            "request": { "subtype": "can_use_tool", "tool_name": tool, "input": input,
                "tool_use_id": "toolu_1", "title": "Claude needs approval",
                "decision_reason": reason },
        }));
        let mut decision = None;
        while let Ok(message) = self.rx.recv() {
            if message["type"] == "control_response" {
                decision = Some(message["response"]["response"].clone());
                break;
            }
        }
        let allowed = decision.as_ref().is_some_and(|d| d["behavior"] == "allow");
        let said = decision
            .and_then(|d| d["message"].as_str().map(str::to_owned))
            .unwrap_or_default();
        if allowed {
            self.tool_result("toolu_1", "ran", false);
        } else {
            self.tool_result("toolu_1", &said, true);
        }
        self.result("success", false, "done");
    }

    fn tool_turn(&mut self, message_id: &str, tool: &str, input: &Value, what: &str) {
        self.assistant(
            message_id,
            json!({ "type": "tool_use", "id": "toolu_1", "name": tool, "input": input }),
        );
        let decision = self.ask(tool, "toolu_1", input);
        if decision.as_ref().is_some_and(|d| d["behavior"] == "allow") {
            self.tool_result("toolu_1", &format!("ran: {what}"), false);
            self.say(message_id, 1, &["done"]);
        } else {
            let interrupt = decision.as_ref().is_some_and(|d| d["interrupt"] == true);
            let mut message = decision
                .and_then(|d| d["message"].as_str().map(str::to_owned))
                .unwrap_or_default();
            if interrupt {
                message.push_str(" [interrupt]");
            }
            self.tool_result("toolu_1", &message, true);
            self.say(message_id, 1, &["denied"]);
        }
        self.result("success", false, "done");
    }
}
