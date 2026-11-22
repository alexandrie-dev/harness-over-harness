//! The external-harness slot in the real window engine: the built `hoh` binary with the window
//! profile answers `hoh/executor/*` for Claude Code and runs Claude sessions, driven over stdio
//! JSON-RPC as the Tauri core drives it. The `claude` here is a fake (a Python script): it answers
//! `--version`, the stream-json `initialize` and a turn; it asks `can_use_tool` and waits for the
//! answer as the CLI does; it records the command line, the folder and the names of the
//! environment variables of every start, and every answer it got. The real CLI is never run and
//! nothing goes to the network.
//!
//! The access mode of Claude is the human's choice in the window (`hoh/executor/access/set`, a
//! privileged command) and nothing else: the tests below check the command line Claude is
//! actually started with against that choice.
//!
//! Contract sources: `hoh-protocol` (`hoh/executor/*`, `PRIVILEGED_METHODS`),
//! `hoh-external-harness/src/{window.rs,agent.rs}` (`launch_policy`),
//! `hoh-harness-claude/src/harness.rs` (`launch_for`, `session_args`), `docs/claude-live-run.md`.

use hoh_protocol::{HostApprovalRequest, approval};
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::PathBuf, process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};

const FRAME_TIMEOUT: Duration = Duration::from_secs(60);
const FAKE_VERSION: &str = "2.0.31";
const SESSION: &str = "window-claude";
/// The tools Claude keeps in a read-only session (`hoh-harness-claude`, `READ_ONLY_TOOLS`).
const READ_ONLY_TOOLS: [&str; 6] = ["Read", "Glob", "Grep", "LS", "NotebookRead", "TodoWrite"];
/// A read-only session of the slot: what a reviewer is configured with.
const READ_ONLY_SLOT: &str =
    "- id: external-harness\n  config:\n    policy:\n      sandbox: read-only\n";

fn approval_key() -> [u8; 32] {
    std::array::from_fn(|index| u8::try_from(index).expect("index fits a byte") + 7)
}

/// The window profile of a first run, with `harness-claude` pointed at the fake `claude`.
struct Workspace {
    root: tempfile::TempDir,
    folder: PathBuf,
    claude: PathBuf,
}

/// One start of the fake `claude`.
#[derive(Debug, serde::Deserialize)]
struct Start {
    argv: Vec<String>,
    /// Names only: the values may be secrets.
    env: Vec<String>,
    /// The folder the process runs in.
    cwd: PathBuf,
    /// The process id of the fake.
    pid: u32,
}

/// What the fake `claude` was answered to one `can_use_tool`.
#[derive(Debug, serde::Deserialize)]
struct Answer {
    tool: String,
    behavior: String,
    message: Option<String>,
    /// What the CLI is told to run with instead: the host's answers to a question.
    #[serde(rename = "updatedInput")]
    updated_input: Option<Value>,
}

impl Start {
    /// A native session, not `--version` or the discovery probe.
    fn is_session(&self) -> bool {
        self.argv
            .iter()
            .any(|arg| arg == "--session-id" || arg == "--resume")
    }

    fn value_of(&self, switch: &str) -> Option<&str> {
        let at = self.argv.iter().position(|arg| arg == switch)?;
        self.argv.get(at + 1).map(String::as_str)
    }

    fn has(&self, word: &str) -> bool {
        self.argv.iter().any(|arg| arg == word)
    }
}

impl Workspace {
    fn new() -> Self {
        Self::with("", "")
    }

    /// `claude_config`: more lines of the `harness-claude` row's config, indented under it;
    /// `rows`: more rows of the profile patch.
    fn with(claude_config: &str, rows: &str) -> Self {
        Self::setup(claude_config, rows, FAKE_VERSION)
    }

    /// [`with`](Self::with), with the version the fake `claude` answers `--version` with.
    fn setup(claude_config: &str, rows: &str, version: &str) -> Self {
        let root = tempfile::tempdir().expect("temporary root");
        let folder = root.path().join("work");
        std::fs::create_dir_all(&folder).expect("workspace folder");
        let folder = folder.canonicalize().expect("canonical workspace");
        std::fs::write(folder.join("notes.txt"), "notes\n").expect("notes");
        std::fs::create_dir_all(folder.join("build")).expect("build folder");
        std::fs::create_dir_all(root.path().join("tmp")).expect("engine temporary folder");
        let starts = root.path().join("claude-starts");
        std::fs::create_dir_all(&starts).expect("starts folder");
        let claude = root.path().join("bin").join("claude");
        std::fs::create_dir_all(claude.parent().expect("bin folder")).expect("bin folder");
        let fake = FAKE_CLAUDE
            .replace("@STARTS@", &starts.display().to_string())
            .replace("@VERSION@", version);
        std::fs::write(&claude, fake).expect("fake claude");
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755))
            .expect("fake claude is executable");
        let home = root.path().join("home");
        let profile = home.join("profiles").join("window");
        std::fs::create_dir_all(&profile).expect("profile folder");
        // The credentials store refuses a harness home that other users can enter.
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))
            .expect("private harness home");
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
            format!(
                "- id: model-host\n  config:\n    baseUrl: http://127.0.0.1:9/v1\n    model: window-mock\n\
                 - id: harness-claude\n  config:\n    executable: {}\n{claude_config}{rows}",
                claude.display()
            ),
        )
        .expect("profile patch");
        Self {
            root,
            folder,
            claude,
        }
    }

    fn profile(&self) -> PathBuf {
        self.root
            .path()
            .join("home")
            .join("profiles")
            .join("window")
    }

    /// Every start of the fake, in order.
    fn starts(&self) -> Vec<Start> {
        let mut files = std::fs::read_dir(self.root.path().join("claude-starts"))
            .expect("starts folder")
            .map(|entry| entry.expect("start record").path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect::<Vec<_>>();
        files.sort();
        files
            .iter()
            .map(|file| {
                serde_json::from_slice(&std::fs::read(file).expect("start record"))
                    .expect("start record is JSON")
            })
            .collect()
    }

    /// Every answer the fake got to a `can_use_tool`, in order.
    fn answers(&self) -> Vec<Answer> {
        std::fs::read_to_string(self.root.path().join("claude-answers.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).expect("answer record"))
            .collect()
    }

    /// Every file of the test's tree with its text: the engine's data (its session logs in
    /// `home/sessions`), its temporary files and its stderr, and what the fake wrote.
    fn files(&self) -> Vec<(PathBuf, String)> {
        let mut found = Vec::new();
        let mut folders = vec![self.root.path().to_path_buf()];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).expect("test folder") {
                let path = entry.expect("test folder entry").path();
                if path.is_dir() {
                    folders.push(path);
                } else {
                    let bytes = std::fs::read(&path).expect("test file");
                    found.push((path, String::from_utf8_lossy(&bytes).into_owned()));
                }
            }
        }
        found
    }

    /// The native Claude sessions started so far, in order.
    fn sessions(&self) -> Vec<Start> {
        self.starts()
            .into_iter()
            .filter(Start::is_session)
            .collect()
    }
}

/// A stand-in for the `claude` CLI (`@STARTS@` and `@VERSION@` are filled in). Every start
/// writes its arguments, folder and the names of its environment variables to its own file in
/// `claude-starts`. It answers `--version`, `initialize` (a signed-in account, two models) and
/// `interrupt`. A turn begins with `system/init` naming the model `claude-x-1`; the prompt picks
/// the step:
/// - `edit` / `run`: an `Edit` of `notes.txt` / a `Bash` `touch ran.txt`, asked with `can_use_tool`
///   (under `bypassPermissions` run without asking, as the CLI does);
/// - `protected`: an `Edit` of `.claude/settings.json`, which the CLI asks even under bypass;
/// - `remove`: a `Bash` `rm -rf build` with the CLI's removal warning (`Dangerous rm …`), asked
///   in every mode;
/// - `ask`: an `AskUserQuestion` (which colour?), asked with `can_use_tool` in confirm; in full
///   the CLI sends no callback for it (PORT_NOTES of `hoh-harness-claude`, `AskUserQuestion` in
///   full), so nothing is asked and nothing is recorded;
/// - `leak`: writes the `ANTHROPIC_API_KEY` it was given to stderr and dies mid-turn;
/// - `long`: runs `sleep 120` in a session of its own, as Claude Code runs its commands, writes its
///   pid to `claude-commands/<pid of the CLI>` and says `working`; like Claude Code 2.1.329 in the
///   pilot of 22.11, its interrupt ends the turn and leaves the command running, and the end of
///   its stdin does not end the turn; `long <name>` runs it as `<name>` (a link to `sleep` in the
///   root);
/// - `short`: the one-word answer `ok1`, streamed (`stream_event`) before its message;
/// - with `wake-on-resume` in the root, a CLI started with `--resume` first answers a lost
///   background command's notification (a result of no turn) when the first prompt comes;
/// - anything else: a one-line answer.
///
/// Every answer to a `can_use_tool` is appended to `claude-answers.jsonl`; a step runs only after
/// `allow`.
const FAKE_CLAUDE: &str = r##"#!/usr/bin/python3 -I
import json, os, shutil, subprocess, sys, time

STARTS = "@STARTS@"
ARGS = sys.argv[1:]
record = os.path.join(STARTS, "start-%d-%d" % (time.time_ns(), os.getpid()))
with open(record + ".part", "w") as file:
    json.dump({"argv": ARGS, "env": sorted(os.environ), "cwd": os.getcwd(), "pid": os.getpid()}, file)
os.rename(record + ".part", record + ".json")
if "--version" in ARGS:
    print("@VERSION@ (Claude Code)")
    sys.exit(0)


def value_of(flag):
    return ARGS[ARGS.index(flag) + 1] if flag in ARGS[:-1] else None


SESSION = value_of("--session-id") or value_of("--resume") or "fake-session"
# While this file exists the CLI names its session with a word that is not a session id.
if os.path.exists(os.path.join(os.path.dirname(STARTS), "odd-session-id")):
    SESSION = "--dangerously-skip-permissions"
BYPASS = value_of("--permission-mode") == "bypassPermissions"
CWD = os.getcwd()
ANSWERS = os.path.join(os.path.dirname(STARTS), "claude-answers.jsonl")
# While this file exists the CLI is slow to come up: it does not answer `initialize`.
SLOW_START = os.path.join(os.path.dirname(STARTS), "slow-initialize")
# What the conversation remembers across processes, as the CLI keeps it on disk by session id.
MEMORY = os.path.join(os.path.dirname(STARTS), "claude-memory-" + SESSION)
counter = [0]


def write(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()


def read():
    line = sys.stdin.readline()
    if not line:
        sys.exit(0)
    try:
        return json.loads(line)
    except ValueError:
        return {}


def next_id(prefix):
    counter[0] += 1
    return "%s-%d" % (prefix, counter[0])


def control(message):
    request = message.get("request") or {}
    response = {}
    if request.get("subtype") == "initialize" and os.path.exists(SLOW_START):
        return
    if request.get("subtype") == "initialize":
        response = {"models": [{"value": "default", "displayName": "Default"},
                               {"value": "sonnet", "displayName": "Sonnet"}],
                    "account": {"email": "user@example.com", "tokenSource": "claude.ai"}}
    write({"type": "control_response",
           "response": {"subtype": "success", "request_id": message.get("request_id"), "response": response}})


def assistant(block):
    write({"type": "assistant", "session_id": SESSION, "parent_tool_use_id": None,
           "message": {"id": next_id("msg"), "role": "assistant", "model": "claude-x-1", "content": [block]}})


def step(tool, tool_input, reason, run, asked_in_bypass):
    tool_id = next_id("toolu")
    assistant({"type": "tool_use", "id": tool_id, "name": tool, "input": tool_input})
    if BYPASS and not asked_in_bypass:
        allowed = True
    else:
        request = next_id("perm")
        body = {"subtype": "can_use_tool", "tool_name": tool, "input": tool_input, "tool_use_id": tool_id}
        if reason:
            body["decision_reason"] = reason
        write({"type": "control_request", "request_id": request, "request": body})
        while True:
            message = read()
            response = message.get("response") or {}
            if message.get("type") == "control_response" and response.get("request_id") == request:
                answer = response.get("response") or {}
                break
            if message.get("type") == "control_request":
                control(message)
        allowed = answer.get("behavior") == "allow"
        with open(ANSWERS, "a") as file:
            file.write(json.dumps({"tool": tool, "behavior": answer.get("behavior"),
                                   "message": answer.get("message"),
                                   "updatedInput": answer.get("updatedInput")}) + "\n")
    if allowed:
        run()
    write({"type": "user", "session_id": SESSION, "parent_tool_use_id": None,
           "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": tool_id,
                                                     "content": "done" if allowed else "denied",
                                                     "is_error": not allowed}]}})


def append_notes():
    with open(os.path.join(CWD, "notes.txt"), "a") as file:
        file.write("edited\n")


def touch_ran():
    open(os.path.join(CWD, "ran.txt"), "a").close()


def write_settings():
    os.makedirs(os.path.join(CWD, ".claude"), exist_ok=True)
    with open(os.path.join(CWD, ".claude", "settings.json"), "w") as file:
        file.write("{}\n")


def remove_build():
    shutil.rmtree(os.path.join(CWD, "build"), ignore_errors=True)


def hang(listens):
    # A turn that does not end by itself. `listens`: it ends the turn when told to interrupt, as the
    # CLI does; otherwise it ignores the interrupt, and only a kill ends it.
    while True:
        message = read()
        request = message.get("request") or {}
        if message.get("type") == "control_request" and request.get("subtype") == "interrupt":
            if not listens:
                continue
            control(message)
            write({"type": "result", "subtype": "error_during_execution", "is_error": True,
                   "result": "interrupted", "session_id": SESSION, "duration_ms": 1, "num_turns": 1})
            return
        if message.get("type") == "control_request":
            control(message)


def long_command(name):
    assistant({"type": "tool_use", "id": next_id("toolu"), "name": "Bash",
               "input": {"command": "sleep 120", "description": "Wait"}})
    program = "sleep"
    if name:
        program = os.path.join(os.path.dirname(STARTS), name)
        if not os.path.lexists(program):
            os.symlink(shutil.which("sleep"), program)
    command = subprocess.Popen([program, "120"], start_new_session=True)
    commands = os.path.join(os.path.dirname(STARTS), "claude-commands")
    os.makedirs(commands, exist_ok=True)
    with open(os.path.join(commands, "%d.part" % os.getpid()), "w") as file:
        file.write("%d\n" % command.pid)
    os.rename(os.path.join(commands, "%d.part" % os.getpid()), os.path.join(commands, str(os.getpid())))
    assistant({"type": "text", "text": "working"})
    while True:
        line = sys.stdin.readline()
        if not line:
            command.wait()
            sys.exit(0)
        try:
            message = json.loads(line)
        except ValueError:
            continue
        request = message.get("request") or {}
        if message.get("type") == "control_request":
            control(message)
            if request.get("subtype") == "interrupt":
                write({"type": "result", "subtype": "error_during_execution", "is_error": True,
                       "result": "interrupted", "session_id": SESSION, "duration_ms": 1, "num_turns": 1})
                return


def short_answer():
    message_id = next_id("msg")
    stream = {"type": "stream_event", "session_id": SESSION, "parent_tool_use_id": None}
    write(dict(stream, event={"type": "message_start", "message": {"id": message_id, "role": "assistant"}}))
    write(dict(stream, event={"type": "content_block_delta", "index": 0,
                              "delta": {"type": "text_delta", "text": "ok1"}}))
    write({"type": "assistant", "session_id": SESSION, "parent_tool_use_id": None,
           "message": {"id": message_id, "role": "assistant", "model": "claude-x-1",
                       "content": [{"type": "text", "text": "ok1"}]}})
    write({"type": "result", "subtype": "success", "is_error": False, "result": "ok1",
           "session_id": SESSION, "duration_ms": 1, "num_turns": 1})


def turn(text):
    write({"type": "system", "subtype": "init", "session_id": SESSION, "model": "claude-x-1",
           "cwd": CWD, "tools": ["Read", "Edit", "Bash"], "permissionMode": value_of("--permission-mode")})
    if text in ("hang", "hang-deaf"):
        assistant({"type": "text", "text": "working"})
        hang(text == "hang")
        return
    if text == "long" or text.startswith("long "):
        long_command(text[len("long "):])
        return
    if text == "short":
        short_answer()
        return
    if text.startswith("remember "):
        with open(MEMORY, "a") as file:
            file.write(text[len("remember "):] + "\n")
    elif text == "recall":
        said = open(MEMORY).read().split() if os.path.exists(MEMORY) else []
        assistant({"type": "text", "text": "you said: " + " ".join(said) if said else "nothing yet"})
        write({"type": "result", "subtype": "success", "is_error": False, "result": "done",
               "session_id": SESSION, "duration_ms": 1, "num_turns": 1})
        return
    if text == "edit":
        step("Edit", {"file_path": os.path.join(CWD, "notes.txt"), "old_string": "notes\n",
                      "new_string": "notes\nedited\n"}, None, append_notes, False)
    elif text == "run":
        step("Bash", {"command": "touch ran.txt", "description": "Create a marker file"}, None, touch_ran, False)
    elif text.startswith("run: "):
        step("Bash", {"command": text[len("run: "):], "description": "Run a command"}, None, lambda: None, False)
    elif text.startswith("edit: "):
        step("Edit", {"file_path": os.path.join(CWD, text[len("edit: "):]), "old_string": "",
                      "new_string": "x\n"}, None, lambda: None, False)
    elif text == "protected":
        path = os.path.join(CWD, ".claude", "settings.json")
        step("Edit", {"file_path": path, "old_string": "", "new_string": "{}\n"},
             "Claude requested permissions to edit %s which is a sensitive file." % path, write_settings, True)
    elif text == "remove":
        step("Bash", {"command": "rm -rf build", "description": "Remove the build folder"},
             "Dangerous rm -rf command detected", remove_build, True)
    elif text == "ask":
        step("AskUserQuestion", {"questions": [{"question": "Which color?", "header": "Color", "multiSelect": False,
             "options": [{"label": "Red", "description": "warm"}, {"label": "Blue", "description": "cold"}]}]},
             None, lambda: None, False)
    elif text == "leak":
        key = os.environ.get("ANTHROPIC_API_KEY", "")
        sys.stderr.write("API Error: 401 invalid x-api-key %s\nAuthorization: Bearer %s\n" % (key, key))
        sys.stderr.flush()
        sys.exit(3)
    assistant({"type": "text", "text": "done"})
    write({"type": "result", "subtype": "success", "is_error": False, "result": "done",
           "session_id": SESSION, "duration_ms": 1, "num_turns": 1})


# While this file exists a resumed CLI first answers the notification of a background command
# lost with the old process: a result of no turn (origin task-notification), before the prompt's.
WAKE_ON_RESUME = os.path.join(os.path.dirname(STARTS), "wake-on-resume")
woke = [False]

while True:
    message = read()
    if message.get("type") == "control_request":
        control(message)
    elif message.get("type") == "user":
        if "--resume" in ARGS and os.path.exists(WAKE_ON_RESUME) and not woke[0]:
            woke[0] = True
            write({"type": "result", "subtype": "success", "is_error": False, "result": "", "num_turns": 0,
                   "origin": {"kind": "task-notification"}, "session_id": SESSION})
        content = (message.get("message") or {}).get("content") or [{}]
        turn(content[0].get("text", "") if isinstance(content[0], dict) else "")
"##;

struct Host {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: Lines<BufReader<tokio::process::ChildStdout>>,
    stderr: PathBuf,
    key: [u8; 32],
    next_id: u64,
    frames: Vec<Value>,
    /// The label the host picks for every question of Claude (none: no question is expected).
    pick: Option<String>,
    /// The `user-question/ask` requests the host was sent.
    questions: Vec<Value>,
}

impl Host {
    fn start(workspace: &Workspace) -> Self {
        Self::start_with_env(workspace, &[])
    }

    /// [`start`](Self::start) with more variables in the engine's environment.
    fn start_with_env(workspace: &Workspace, env: &[(&str, &str)]) -> Self {
        let stderr = workspace.root.path().join("engine.stderr");
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_hoh"))
            .args(["--profile", "window"])
            .env_clear()
            .env("HOME", workspace.root.path())
            .env("DSH_HOME", workspace.root.path().join("home"))
            .env("TMPDIR", workspace.root.path().join("tmp"))
            .env("PATH", "/usr/bin:/bin")
            .envs(env.iter().copied())
            .current_dir(&workspace.folder)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(std::fs::File::create(&stderr).expect("engine stderr file"))
            .kill_on_drop(true)
            .spawn()
            .expect("spawn hoh");
        let stdin = child.stdin.take().expect("engine stdin");
        let stdout = BufReader::new(child.stdout.take().expect("engine stdout")).lines();
        Self {
            child,
            stdin,
            stdout,
            stderr,
            key: approval_key(),
            next_id: 0,
            frames: Vec::new(),
            pick: None,
            questions: Vec::new(),
        }
    }

    fn context(&self) -> String {
        let tail = self.frames.iter().rev().take(8).rev();
        let tail = tail.map(Value::to_string).collect::<Vec<_>>().join("\n");
        let stderr = std::fs::read_to_string(&self.stderr).unwrap_or_default();
        format!("last frames:\n{tail}\nstderr: {stderr}")
    }

    async fn send(&mut self, method: &str, params: Value) -> String {
        self.next_id += 1;
        let id = format!("host-{}", self.next_id);
        let frame = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        self.write(&frame).await;
        id
    }

    async fn write(&mut self, frame: &Value) {
        self.stdin
            .write_all(format!("{frame}\n").as_bytes())
            .await
            .expect("write engine stdin");
        self.stdin.flush().await.expect("flush engine stdin");
    }

    async fn next(&mut self) -> Value {
        let line = tokio::time::timeout(FRAME_TIMEOUT, self.stdout.next_line())
            .await
            .unwrap_or_else(|_| panic!("engine silent for {FRAME_TIMEOUT:?}\n{}", self.context()))
            .expect("read engine stdout")
            .unwrap_or_else(|| panic!("engine stdout closed\n{}", self.context()));
        let frame: Value = serde_json::from_str(&line)
            .unwrap_or_else(|error| panic!("engine wrote a non-JSON line {line:?}: {error}"));
        self.frames.push(frame.clone());
        frame
    }

    /// The response to `id`; an `approval/request` on the way is answered with `outcome`, and
    /// counted.
    async fn response(&mut self, id: &str, outcome: Option<&str>) -> (Value, usize) {
        let mut asked = 0;
        loop {
            let frame = self.next().await;
            if frame["id"] == id && frame.get("method").is_none() {
                return (frame, asked);
            }
            if frame["method"] == "approval/request" {
                self.answer_approval(&frame, outcome).await;
                asked += 1;
            }
        }
    }

    /// The host's answer to an `approval/request`: `outcome` with a valid MAC.
    async fn answer_approval(
        &mut self,
        frame: &Value,
        outcome: Option<&str>,
    ) -> HostApprovalRequest {
        let outcome = outcome
            .unwrap_or_else(|| panic!("unplanned approval request: {frame}\n{}", self.context()));
        let request: HostApprovalRequest =
            serde_json::from_value(frame["params"].clone()).expect("approval request");
        let mac = approval::mac(&self.key, &request, outcome);
        let answer = json!({
            "jsonrpc":"2.0","id":frame["id"],
            "result":{"approvalId":request.approval_id,"outcome":outcome,"mac":mac}
        });
        self.write(&answer).await;
        request
    }

    /// The host's answer to a `user-question/ask`: the picked label for every question.
    async fn answer_question(&mut self, frame: &Value) {
        let pick = self
            .pick
            .clone()
            .unwrap_or_else(|| panic!("unplanned question: {frame}\n{}", self.context()));
        let answers: Vec<Value> = frame["params"]["questions"]
            .as_array()
            .unwrap_or_else(|| panic!("questions: {frame}"))
            .iter()
            .map(|question| json!({"id": question["id"], "selected": [pick]}))
            .collect();
        self.questions.push(frame["params"].clone());
        let answer = json!({"jsonrpc":"2.0","id":frame["id"],"result":{"answers":answers}});
        self.write(&answer).await;
    }

    /// A call that must succeed; its result is checked against the protocol schema.
    async fn ok(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params).await;
        let (frame, asked) = self.response(&id, None).await;
        assert_eq!(asked, 0, "{method} asked the host");
        let result = frame
            .get("result")
            .unwrap_or_else(|| panic!("{method} failed: {frame}\n{}", self.context()))
            .clone();
        hoh_protocol::validate_result(method, &result)
            .unwrap_or_else(|error| panic!("{method} result is invalid: {error}\n{result}"));
        result
    }

    /// A privileged call the host answers with `outcome`; it asks the host exactly once.
    async fn privileged(&mut self, method: &str, params: Value, outcome: &str) -> Value {
        let id = self.send(method, params).await;
        let (frame, asked) = self.response(&id, Some(outcome)).await;
        assert_eq!(asked, 1, "{method} must ask the host once: {frame}");
        frame
    }

    async fn initialize(&mut self, workspace: &Workspace) {
        let params = json!({
            "cwd": workspace.folder,
            "provider": "openai-compatible",
            "model": "window-mock",
            "hoh": {"protocolVersion": 1, "approvalKey": approval::hex(&self.key)},
            "capabilities": {"approvals": "host", "userQuestions": true}
        });
        let result = self.ok("initialize", params).await;
        assert_eq!(result["hoh"]["approvals"], "host", "{result}");
    }

    /// The human sends new chats to Claude.
    async fn select_claude(&mut self) {
        self.select_model("default").await;
    }

    /// The human sends new chats to Claude with `model`.
    async fn select_model(&mut self, model: &str) {
        let params = json!({"executor": "claude", "model": model});
        let frame = self
            .privileged("hoh/executor/select", params, "allowed-once")
            .await;
        assert_eq!(
            frame["result"]["executor"],
            "claude",
            "{frame}\n{}",
            self.context()
        );
    }

    /// The human chooses the access mode of Claude; the host answers `outcome`.
    async fn set_access(&mut self, mode: &str, outcome: &str) -> Value {
        let params = json!({"executor": "claude", "mode": mode});
        self.privileged("hoh/executor/access/set", params, outcome)
            .await
    }

    async fn access(&mut self) -> Value {
        self.ok("hoh/executor/access/get", json!({"executor": "claude"}))
            .await
    }

    /// `access/get` for one open conversation.
    async fn access_of(&mut self, session: &str) -> Value {
        self.ok(
            "hoh/executor/access/get",
            json!({"executor": "claude", "sessionId": session}),
        )
        .await
    }

    /// The human sends new chats back to the engine's own model.
    async fn select_engine_model(&mut self) {
        let params = json!({"executor": null, "model": "default"});
        let frame = self
            .privileged("hoh/executor/select", params, "allowed-once")
            .await;
        assert_eq!(frame["result"]["executor"], Value::Null, "{frame}");
    }

    /// A turn of the Claude chat; returns when the session is idle again.
    async fn prompt(&mut self, text: &str) {
        let asked = self.turn(SESSION, text, None).await;
        assert!(asked.is_empty(), "{asked:?}");
    }

    /// A turn of the chat `session`; every `approval/request` on the way is answered with
    /// `outcome` (none is expected without one). Returns what the host was asked, when the
    /// session is idle again.
    async fn turn(
        &mut self,
        session: &str,
        text: &str,
        outcome: Option<&str>,
    ) -> Vec<HostApprovalRequest> {
        let from = self.frames.len();
        let params =
            json!({"sessionId": session, "contentBlocks": [{"type": "text", "text": text}]});
        let id = self.send("session/prompt", params).await;
        let mut asked = Vec::new();
        let (mut answered, mut running, mut index) = (false, false, from);
        loop {
            while index < self.frames.len() {
                let frame = self.frames[index].clone();
                index += 1;
                if frame["id"] == id.as_str() && frame.get("method").is_none() {
                    assert!(frame["result"]["messageId"].is_string(), "{frame}");
                    answered = true;
                }
                if frame["method"] == "approval/request" {
                    asked.push(self.answer_approval(&frame, outcome).await);
                }
                if frame["method"] == "user-question/ask" {
                    self.answer_question(&frame).await;
                }
                if frame["method"] == "session.status" && frame["params"]["sessionId"] == session {
                    match frame["params"]["status"].as_str() {
                        Some("running") => running = true,
                        Some("idle") if running && answered => return asked,
                        _ => {}
                    }
                }
            }
            self.next().await;
        }
    }

    /// The reason of the last `turn/end` of `session` the window was sent.
    fn turn_end(&self, session: &str) -> Value {
        self.frames
            .iter()
            .rev()
            .find(|frame| {
                frame["method"] == "session.event"
                    && frame["params"]["sessionId"] == session
                    && frame["params"]["event"]["type"] == "turn/end"
            })
            .map(|frame| frame["params"]["event"]["data"]["reason"].clone())
            .unwrap_or_else(|| panic!("no turn/end of {session}\n{}", self.context()))
    }

    async fn shutdown(mut self) {
        self.ok("shutdown", json!({})).await;
        let Self {
            mut child, stdin, ..
        } = self;
        drop(stdin);
        let status = tokio::time::timeout(FRAME_TIMEOUT, child.wait())
            .await
            .expect("engine exits after shutdown")
            .expect("wait for engine");
        assert_eq!(status.code(), Some(0));
    }
}

// ---------------------------------------------------------------- the command line of Claude

/// The switches that decide what Claude may do without asking, each with its value: the
/// permission mode and channel, the tool limits, and whose settings it loads.
fn access_switches(words: &[String]) -> Vec<Vec<String>> {
    const WITH_VALUE: [&str; 6] = [
        "--permission-mode",
        "--permission-prompt-tool",
        "--tools",
        "--allowedTools",
        "--disallowedTools",
        "--setting-sources",
    ];
    const ALONE: [&str; 3] = [
        "--allow-dangerously-skip-permissions",
        "--dangerously-skip-permissions",
        "--strict-mcp-config",
    ];
    let mut picked = Vec::new();
    let mut words = words.iter();
    while let Some(word) = words.next() {
        if WITH_VALUE.contains(&word.as_str()) {
            picked.push(
                [Some(word), words.next()]
                    .into_iter()
                    .flatten()
                    .cloned()
                    .collect(),
            );
        } else if ALONE.contains(&word.as_str()) {
            picked.push(vec![word.clone()]);
        }
    }
    picked.sort();
    picked
}

/// The window's name for the mode a command line runs in: no questions is full access.
fn mode_of(start: &Start) -> &'static str {
    match start.value_of("--permission-mode") {
        Some("default") => "confirm",
        Some("bypassPermissions" | "dontAsk") => "full",
        other => panic!("unexpected permission mode {other:?}: {start:?}"),
    }
}

/// Nothing secret-looking is handed to Claude by name (the engine's own environment has none).
fn assert_clean_environment(start: &Start) {
    assert!(start.env.iter().any(|name| name == "PATH"), "{start:?}");
    let secret = ["KEY", "TOKEN", "SECRET", "PASSWORD"];
    assert!(
        !start
            .env
            .iter()
            .any(|name| secret.iter().any(|word| name.to_uppercase().contains(word))),
        "a secret-looking variable reaches Claude: {:?}",
        start.env
    );
}

/// «Спрашивать разрешение»: every action the CLI does not settle itself is a question to HoH.
fn assert_confirm(start: &Start) {
    assert_eq!(
        start.value_of("--permission-mode"),
        Some("default"),
        "{start:?}"
    );
    assert_eq!(
        start.value_of("--permission-prompt-tool"),
        Some("stdio"),
        "{start:?}"
    );
    // The human's own settings only: not the folder's (`project`, `local`).
    assert_eq!(
        start.value_of("--setting-sources"),
        Some("user"),
        "{start:?}"
    );
    assert!(!start.has("--strict-mcp-config"), "{start:?}");
    for wide in [
        "bypassPermissions",
        "--allow-dangerously-skip-permissions",
        "--dangerously-skip-permissions",
    ] {
        assert!(!start.has(wide), "{wide} under confirm: {start:?}");
    }
    assert_clean_environment(start);
}

/// «Полный доступ»: no questions, as the human chose.
fn assert_full(start: &Start) {
    assert_eq!(
        start.value_of("--permission-mode"),
        Some("bypassPermissions"),
        "{start:?}"
    );
    assert!(
        start.has("--allow-dangerously-skip-permissions"),
        "{start:?}"
    );
    // Full access limits no tool.
    assert!(
        !start.has("--disallowedTools") && !start.has("--tools"),
        "{start:?}"
    );
    assert_clean_environment(start);
}

/// What `hoh/executor/access/get` shows is what Claude runs with: the mode and the switches of
/// the recorded command line.
async fn assert_effective_is_the_launch(host: &mut Host, start: &Start) {
    let access = host.access().await;
    let flags: Vec<String> = serde_json::from_value(access["effective"]["flags"].clone())
        .unwrap_or_else(|error| panic!("effective.flags: {error}\n{access}"));
    assert_eq!(
        access_switches(&flags),
        access_switches(&start.argv),
        "effective.flags {flags:?} are not the switches Claude runs with: {:?}",
        start.argv
    );
    for flag in &flags {
        assert!(
            start.has(flag),
            "{flag:?} of effective.flags is not in {:?}",
            start.argv
        );
    }
    assert_eq!(
        access["effective"]["mode"],
        mode_of(start),
        "{access}\n{start:?}"
    );
}

/// What `access/get{sessionId}` says about one conversation: its own mode, the mode of its live
/// process (null before the first turn), and whether a change waits for its next message.
fn assert_session(access: &Value, mode: &str, running: &str, context: &str) {
    let session = &access["session"];
    assert_eq!(session["executor"], "claude", "{context}: {access}");
    assert_eq!(session["privacy"], "remote", "{context}: {access}");
    assert_eq!(session["mode"], mode, "{context}: {access}");
    assert_eq!(session["running"], running, "{context}: {access}");
    assert_eq!(session["pending"], mode != running, "{context}: {access}");
}

/// The chat's idle process ended with the choice of `mode`: nothing runs, nothing waits.
fn assert_no_process(access: &Value, mode: &str, context: &str) {
    let session = &access["session"];
    assert_eq!(session["mode"], mode, "{context}: {access}");
    assert_eq!(session["running"], Value::Null, "{context}: {access}");
    assert_eq!(session["pending"], false, "{context}: {access}");
    assert_eq!(access["effective"]["mode"], mode, "{context}: {access}");
}

/// The switches and mode `effective` shows are those of `start`, the process of this conversation.
fn assert_effective_of(access: &Value, start: &Start, context: &str) {
    let flags: Vec<String> = serde_json::from_value(access["effective"]["flags"].clone())
        .unwrap_or_else(|error| panic!("{context}: effective.flags: {error}\n{access}"));
    assert_eq!(
        access_switches(&flags),
        access_switches(&start.argv),
        "{context}: {access}\n{start:?}"
    );
    assert_eq!(
        access["effective"]["mode"],
        mode_of(start),
        "{context}: {access}"
    );
}

fn only_session(workspace: &Workspace, host: &Host) -> Start {
    let mut sessions = workspace.sessions();
    assert_eq!(
        sessions.len(),
        1,
        "one Claude session start expected: {sessions:?}\n{}",
        host.context()
    );
    sessions.remove(0)
}

// ---------------------------------------------------------------- tests

#[tokio::test]
async fn the_window_engine_discovers_claude_and_keeps_its_access_without_the_host() {
    let workspace = Workspace::new();
    let mut host = Host::start(&workspace);
    host.initialize(&workspace).await;
    let claude = json!({"executor": "claude"});

    // Discovery runs the configured program, never a `claude` from PATH.
    let found = host.ok("hoh/executor/discover", claude.clone()).await;
    assert_eq!(found["executor"], "claude", "{found}");
    assert_eq!(found["installed"], true, "{found}");
    assert_eq!(found["version"], FAKE_VERSION, "{found}");
    assert_eq!(found["versionStatus"], "supported", "{found}");
    assert_eq!(found["auth"], "authenticated", "{found}");
    let models = found["models"].as_array().expect("models").clone();
    assert!(
        models.iter().any(|model| model["id"] == "sonnet"),
        "the probe's models are reported: {found}"
    );
    let starts = workspace.starts();
    assert!(
        starts.iter().any(|start| start.argv == ["--version"]),
        "{} was not asked for its version: {starts:?}",
        workspace.claude.display()
    );

    // A first run: the default mode, and the switches the next start would use.
    let access = host.access().await;
    assert_eq!(access["executor"], "claude", "{access}");
    assert_eq!(access["mode"], "confirm", "{access}");
    assert_eq!(access["effective"]["mode"], "confirm", "{access}");
    let flags = access["effective"]["flags"]
        .as_array()
        .unwrap_or_else(|| panic!("effective.flags: {access}"));
    assert!(flags.iter().all(Value::is_string), "{access}");
    assert!(
        access["effective"]["text"]
            .as_str()
            .is_some_and(|text| text.starts_with("С подтверждением")),
        "{access}"
    );
    assert_eq!(access["selected"]["executor"], Value::Null, "{access}");

    // Full access is the user's call: the host refuses, nothing changes and nothing is saved.
    let choices = workspace.profile().join("external-harness.json");
    let refused = host.set_access("full", "rejected").await;
    assert!(refused.get("error").is_some(), "{refused}");
    let after = host.access().await;
    assert_eq!(
        after["mode"], "confirm",
        "a refused change took effect: {after}"
    );
    assert_eq!(after["effective"]["mode"], "confirm", "{after}");
    assert!(!choices.exists(), "a refused change was saved");

    // The same call with the host's consent does change it: the refusal above was the host's.
    let allowed = host.set_access("full", "allowed-once").await;
    assert_eq!(
        allowed["result"],
        json!({"executor": "claude", "mode": "full"}),
        "{allowed}\n{}",
        host.context()
    );
    let full = host.access().await;
    assert_eq!(full["mode"], "full", "{full}");
    let saved: Value =
        serde_json::from_slice(&std::fs::read(&choices).expect("the choice is saved"))
            .expect("saved choices are JSON");
    assert_eq!(saved["access"]["claude"], "full", "{saved}");

    host.shutdown().await;
}

/// (а) The composition says `access: full`, the human left «Спрашивать разрешение»: Claude asks.
#[tokio::test]
async fn a_full_access_in_the_composition_does_not_widen_the_humans_confirm() {
    let workspace = Workspace::with("    access: full\n", "");
    let mut host = Host::start(&workspace);
    host.initialize(&workspace).await;
    host.select_claude().await;
    assert_eq!(host.access().await["mode"], "confirm");

    host.prompt("hello").await;
    assert_confirm(&only_session(&workspace, &host));

    host.shutdown().await;
}

/// (б) The human chose «Полный доступ» and the host approved it: Claude runs without questions.
#[tokio::test]
async fn b_full_access_chosen_by_the_human_starts_claude_without_questions() {
    let workspace = Workspace::new();
    let mut host = Host::start(&workspace);
    host.initialize(&workspace).await;
    let chosen = host.set_access("full", "allowed-once").await;
    assert_eq!(chosen["result"]["mode"], "full", "{chosen}");
    host.select_claude().await;

    host.prompt("hello").await;
    assert_full(&only_session(&workspace, &host));

    host.shutdown().await;
}

/// (в) Without the host's approval `access/set` changes neither the mode nor the running Claude.
#[tokio::test]
async fn c_access_set_refused_by_the_host_changes_neither_the_mode_nor_the_launch() {
    let workspace = Workspace::new();
    let mut host = Host::start(&workspace);
    host.initialize(&workspace).await;
    host.select_claude().await;
    host.prompt("hello").await;
    let first = only_session(&workspace, &host);
    assert_confirm(&first);
    let before = host.access().await;

    let refused = host.set_access("full", "rejected").await;
    assert!(refused.get("error").is_some(), "{refused}");
    let after = host.access().await;
    assert_eq!(
        after["mode"], "confirm",
        "a refused change took effect: {after}"
    );
    assert_eq!(after["effective"], before["effective"], "{after}");
    // The file holds the selection of Claude; the refused mode is not in it.
    let saved: Value = serde_json::from_slice(
        &std::fs::read(workspace.profile().join("external-harness.json")).expect("choices"),
    )
    .expect("saved choices are JSON");
    assert_ne!(
        saved["access"]["claude"], "full",
        "a refused change was saved: {saved}"
    );

    // The next turn goes to the same Claude, started as before.
    host.prompt("hello again").await;
    let again = only_session(&workspace, &host);
    assert_eq!(again.argv, first.argv);

    host.shutdown().await;
}

/// (г) Full access does not widen a read-only session: Claude keeps the read tools only.
#[tokio::test]
async fn d_a_read_only_session_stays_read_only_under_full_access() {
    let workspace = Workspace::with("", READ_ONLY_SLOT);
    let mut host = Host::start(&workspace);
    host.initialize(&workspace).await;
    let chosen = host.set_access("full", "allowed-once").await;
    assert_eq!(chosen["result"]["mode"], "full", "{chosen}");
    host.select_claude().await;

    host.prompt("hello").await;
    let start = only_session(&workspace, &host);
    for wide in [
        "bypassPermissions",
        "--allow-dangerously-skip-permissions",
        "--dangerously-skip-permissions",
    ] {
        assert!(!start.has(wide), "{wide} in a read-only session: {start:?}");
    }
    let tools = start
        .value_of("--tools")
        .unwrap_or_else(|| panic!("a read-only session limits the tools: {start:?}"));
    assert!(
        tools.split(',').all(|tool| READ_ONLY_TOOLS.contains(&tool)),
        "a read-only session gets {tools}"
    );
    assert_clean_environment(&start);

    host.shutdown().await;
}

/// (д) `access/get` shows the mode and the switches Claude was actually started with, in each
/// case: the human's confirm, then the human's full access, and full access over a read-only
/// session.
#[tokio::test]
async fn e_access_get_shows_the_mode_and_switches_claude_runs_with() {
    let workspace = Workspace::new();
    let mut host = Host::start(&workspace);
    host.initialize(&workspace).await;
    host.select_claude().await;
    host.prompt("hello").await;
    let confirm = only_session(&workspace, &host);
    assert_effective_is_the_launch(&mut host, &confirm).await;

    // The new choice takes effect from the next turn: Claude is started again under it.
    host.set_access("full", "allowed-once").await;
    host.prompt("hello again").await;
    let sessions = workspace.sessions();
    let full = sessions.get(1).unwrap_or_else(|| {
        panic!(
            "Claude was not started again: {sessions:?}\n{}",
            host.context()
        )
    });
    assert_effective_is_the_launch(&mut host, full).await;
    host.shutdown().await;

    let workspace = Workspace::with("", READ_ONLY_SLOT);
    let mut host = Host::start(&workspace);
    host.initialize(&workspace).await;
    host.set_access("full", "allowed-once").await;
    host.select_claude().await;
    host.prompt("hello").await;
    let read_only = only_session(&workspace, &host);
    assert_effective_is_the_launch(&mut host, &read_only).await;
    host.shutdown().await;
}

// ---------------------------------------------------------------- promises between the slot and Claude
// (review of cand/i4, items 3–11: each promise has its own test on the real engine)

/// A started window engine with Claude selected for new chats, under `mode` chosen by the human.
async fn engine_with_claude(workspace: &Workspace, mode: &str) -> Host {
    let mut host = Host::start(workspace);
    host.initialize(workspace).await;
    if mode == "full" {
        let chosen = host.set_access("full", "allowed-once").await;
        assert_eq!(chosen["result"]["mode"], "full", "{chosen}");
    }
    host.select_claude().await;
    host
}

fn notes(workspace: &Workspace) -> String {
    std::fs::read_to_string(workspace.folder.join("notes.txt")).expect("notes")
}

/// 3 (allow). In confirm the CLI's request reaches the host as `approval/request`; the host's
/// "allow" goes back to the CLI and the step runs.
#[tokio::test]
async fn p3_confirm_a_request_allowed_by_the_host_reaches_claude_and_runs() {
    let workspace = Workspace::new();
    let mut host = engine_with_claude(&workspace, "confirm").await;

    let asked = host.turn(SESSION, "edit", Some("allowed-once")).await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(asked[0].tool_name, "edit_file", "{asked:?}");
    let asked = host.turn(SESSION, "run", Some("allowed-once")).await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(asked[0].tool_name, "bash", "{asked:?}");

    let answers = workspace.answers();
    assert_eq!(
        answers
            .iter()
            .map(|answer| (answer.tool.as_str(), answer.behavior.as_str()))
            .collect::<Vec<_>>(),
        [("Edit", "allow"), ("Bash", "allow")],
        "{answers:?}"
    );
    assert_eq!(notes(&workspace), "notes\nedited\n");
    assert!(
        workspace.folder.join("ran.txt").exists(),
        "the allowed command did not run"
    );
    assert_eq!(host.turn_end(SESSION)["kind"], "completed");
    host.shutdown().await;
}

/// 3 (deny). The host's "no" goes back to the CLI as `deny` and the step does not run.
#[tokio::test]
async fn p3_confirm_a_request_refused_by_the_host_reaches_claude_and_does_not_run() {
    let workspace = Workspace::new();
    let mut host = engine_with_claude(&workspace, "confirm").await;

    let asked = host.turn(SESSION, "edit", Some("rejected")).await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    let asked = host.turn(SESSION, "run", Some("rejected")).await;
    assert_eq!(asked.len(), 1, "{asked:?}");

    let answers = workspace.answers();
    assert_eq!(
        answers
            .iter()
            .map(|answer| (answer.tool.as_str(), answer.behavior.as_str()))
            .collect::<Vec<_>>(),
        [("Edit", "deny"), ("Bash", "deny")],
        "{answers:?}"
    );
    assert_eq!(notes(&workspace), "notes\n", "a refused edit was made");
    assert!(
        !workspace.folder.join("ran.txt").exists(),
        "a refused command ran"
    );
    host.shutdown().await;
}

/// 4. In full access what the CLI refuses even under bypass (a protected path) is put to the
/// human as a question, and the human's answer reaches the CLI.
#[tokio::test]
async fn p4_full_what_even_bypass_refuses_reaches_the_human() {
    let workspace = Workspace::new();
    let mut host = engine_with_claude(&workspace, "full").await;

    // An ordinary edit runs without a question: that is full access.
    let asked = host.turn(SESSION, "edit", None).await;
    assert!(asked.is_empty(), "{asked:?}");
    assert_eq!(notes(&workspace), "notes\nedited\n");

    let asked = host.turn(SESSION, "protected", Some("allowed-once")).await;
    assert_eq!(
        asked.len(),
        1,
        "the protected path was not put to the human: {asked:?}"
    );
    assert_eq!(asked[0].tool_name, "edit_file", "{asked:?}");
    let answers = workspace.answers();
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(
        (answers[0].tool.as_str(), answers[0].behavior.as_str()),
        ("Edit", "allow")
    );
    assert!(
        workspace
            .folder
            .join(".claude")
            .join("settings.json")
            .exists()
    );
    assert_full(&only_session(&workspace, &host));
    host.shutdown().await;
}

/// 5. In confirm the network is closed: the CLI gets `--disallowedTools WebFetch,WebSearch`.
#[tokio::test]
async fn p5_confirm_closes_the_network_of_claude() {
    let workspace = Workspace::new();
    let mut host = engine_with_claude(&workspace, "confirm").await;
    host.prompt("hello").await;
    let start = only_session(&workspace, &host);
    assert_eq!(
        start.value_of("--disallowedTools"),
        Some("WebFetch,WebSearch"),
        "{start:?}"
    );
    let flags = host.access().await["effective"]["flags"].clone();
    let flags: Vec<String> = serde_json::from_value(flags).expect("flags");
    let at = flags.iter().position(|flag| flag == "--disallowedTools");
    assert_eq!(
        at.and_then(|at| flags.get(at + 1)).map(String::as_str),
        Some("WebFetch,WebSearch"),
        "{flags:?}"
    );
    host.shutdown().await;
}

/// 6. A read-only session in confirm, through the engine: the CLI asks (`default`, the stdio
/// channel) and has the read tools only.
#[tokio::test]
async fn p6_confirm_a_read_only_session_asks_and_keeps_the_read_tools_only() {
    let workspace = Workspace::with("", READ_ONLY_SLOT);
    let mut host = engine_with_claude(&workspace, "confirm").await;
    host.prompt("hello").await;
    let start = only_session(&workspace, &host);
    assert_confirm(&start);
    assert!(!start.has("dontAsk"), "{start:?}");
    let tools = start
        .value_of("--tools")
        .unwrap_or_else(|| panic!("a read-only session limits the tools: {start:?}"));
    assert!(!tools.is_empty(), "{start:?}");
    assert!(
        tools.split(',').all(|tool| READ_ONLY_TOOLS.contains(&tool)),
        "a read-only session gets {tools}"
    );
    host.shutdown().await;
}

/// 7 (confirm). The CLI's removal check is a question to the human in confirm.
#[tokio::test]
async fn p7_confirm_the_removal_check_is_put_to_the_human() {
    let workspace = Workspace::new();
    let mut host = engine_with_claude(&workspace, "confirm").await;
    let asked = host.turn(SESSION, "remove", Some("rejected")).await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(asked[0].tool_name, "bash", "{asked:?}");
    let answers = workspace.answers();
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(answers[0].behavior, "deny", "{answers:?}");
    assert!(
        workspace.folder.join("build").exists(),
        "the refused removal ran"
    );
    host.shutdown().await;
}

/// 7 (full). Only under bypass the removal check is refused by itself: no question, the CLI is
/// told the check refused it, nothing is removed.
#[tokio::test]
async fn p7_full_the_removal_check_is_refused_without_a_question() {
    let workspace = Workspace::new();
    let mut host = engine_with_claude(&workspace, "full").await;
    let asked = host.turn(SESSION, "remove", None).await;
    assert!(asked.is_empty(), "{asked:?}");
    let answers = workspace.answers();
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(answers[0].behavior, "deny", "{answers:?}");
    assert!(
        answers[0]
            .message
            .as_deref()
            .is_some_and(|message| message.contains("removal safety check")),
        "{answers:?}"
    );
    assert!(
        workspace.folder.join("build").exists(),
        "the refused removal ran"
    );
    host.shutdown().await;
}

/// 8. The model chosen with `hoh/executor/select` reaches the CLI as `--model`; `default` adds none.
#[tokio::test]
async fn p8_the_selected_model_reaches_claude() {
    let workspace = Workspace::new();
    let mut host = Host::start(&workspace);
    host.initialize(&workspace).await;
    host.select_model("sonnet").await;
    host.turn("chat-sonnet", "hello", None).await;
    host.select_model("default").await;
    host.turn("chat-default", "hello", None).await;
    let sessions = workspace.sessions();
    assert_eq!(sessions.len(), 2, "{sessions:?}");
    assert_eq!(
        sessions[0].value_of("--model"),
        Some("sonnet"),
        "{:?}",
        sessions[0]
    );
    assert!(!sessions[1].has("--model"), "{:?}", sessions[1]);
    host.shutdown().await;
}

/// 9. Claude runs in the session's working folder.
#[tokio::test]
async fn p9_claude_runs_in_the_folder_of_the_session() {
    let workspace = Workspace::new();
    let mut host = engine_with_claude(&workspace, "confirm").await;
    host.prompt("hello").await;
    let start = only_session(&workspace, &host);
    assert_eq!(
        start.cwd.canonicalize().expect("canonical cwd"),
        workspace.folder,
        "{start:?}"
    );
    host.shutdown().await;
}

/// 10. A version outside the range: the chat refuses to start with a readable error, and no
/// session process is started.
#[tokio::test]
async fn p10_an_unsupported_version_refuses_the_chat() {
    let workspace = Workspace::setup("", "", "1.0.0");
    let mut host = engine_with_claude(&workspace, "confirm").await;
    let asked = host.turn(SESSION, "hello", None).await;
    assert!(asked.is_empty(), "{asked:?}");
    let reason = host.turn_end(SESSION);
    assert_eq!(reason["kind"], "error", "{reason}");
    assert_eq!(reason["error"]["code"], "unsupported-version", "{reason}");
    let message = reason["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("1.0.0") && message.contains("не поддерживается"),
        "{reason}"
    );
    assert!(
        workspace.sessions().is_empty(),
        "a session process started: {:?}",
        workspace.sessions()
    );
    assert!(
        workspace
            .starts()
            .iter()
            .all(|start| start.argv == ["--version"]),
        "{:?}",
        workspace.starts()
    );
    host.shutdown().await;
}

/// 11. A key in the engine's environment, which the CLI also writes to stderr when it dies: it is
/// in no session log, no answer or event of the window, nor the engine's own log. The CLI does
/// get the variable (the code passes the environment through: an API-key login of Claude Code
/// needs it).
#[tokio::test]
async fn p11_a_key_stays_out_of_the_logs_and_the_window() {
    const KEY: &str = "sk-ant-test-0123456789abcdefghijklmnopqrstuvwxyzABCDEF";
    let workspace = Workspace::new();
    let mut host = Host::start_with_env(&workspace, &[("ANTHROPIC_API_KEY", KEY)]);
    host.initialize(&workspace).await;
    host.select_claude().await;
    host.prompt("hello").await;
    let asked = host.turn(SESSION, "leak", None).await;
    assert!(asked.is_empty(), "{asked:?}");
    let reason = host.turn_end(SESSION);
    assert_eq!(reason["kind"], "error", "{reason}");
    // The CLI's own words reach the human, masked.
    let message = reason["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("invalid x-api-key"), "{reason}");
    host.access().await;

    let start = &workspace.sessions()[0];
    assert!(
        start.env.iter().any(|name| name == "ANTHROPIC_API_KEY"),
        "{start:?}"
    );
    for frame in &host.frames {
        assert!(
            !frame.to_string().contains(KEY),
            "the key reached the window: {frame}"
        );
    }
    host.shutdown().await;
    let files = workspace.files();
    let sessions = workspace.root.path().join("home").join("sessions");
    assert!(
        files.iter().any(|(path, _)| path.starts_with(&sessions)),
        "no session log was written: {:?}",
        files.iter().map(|(path, _)| path).collect::<Vec<_>>()
    );
    assert!(
        files
            .iter()
            .any(|(path, _)| path.ends_with("engine.stderr")),
        "{files:?}"
    );
    for (path, text) in &files {
        assert!(!text.contains(KEY), "the key is in {}", path.display());
    }
}

// ---------------------------------------------------------------- i5 review: questions, sources, discover

/// p12. In confirm a question of Claude (`AskUserQuestion`) reaches the human as a
/// `user-question/ask` of the window, and the human's answer goes back to Claude in its answer to
/// the callback (`updatedInput.answers`, by the text of the question).
#[tokio::test]
async fn p12_confirm_a_question_of_claude_reaches_the_human_and_the_answer_returns() {
    let workspace = Workspace::new();
    let mut host = engine_with_claude(&workspace, "confirm").await;
    host.pick = Some("Blue".to_owned());
    let asked = host.turn(SESSION, "ask", None).await;
    assert!(asked.is_empty(), "a question is not an approval: {asked:?}");

    assert_eq!(host.questions.len(), 1, "{:?}", host.questions);
    let sent = &host.questions[0];
    assert_eq!(sent["sessionId"], SESSION, "{sent}");
    let question = &sent["questions"][0];
    assert_eq!(question["question"], "Which color?", "{sent}");
    let labels: Vec<&Value> = question["options"]
        .as_array()
        .expect("options")
        .iter()
        .map(|option| &option["label"])
        .collect();
    assert_eq!(labels, ["Red", "Blue"], "{sent}");

    let answers = workspace.answers();
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(answers[0].tool, "AskUserQuestion", "{answers:?}");
    assert_eq!(answers[0].behavior, "allow", "{answers:?}");
    let updated = answers[0].updated_input.as_ref().expect("updatedInput");
    assert_eq!(
        updated["answers"],
        json!({"Which color?": "Blue"}),
        "{updated}"
    );
    assert!(updated["questions"].is_array(), "{updated}");
    assert_eq!(host.turn_end(SESSION)["kind"], "completed");
    host.shutdown().await;
}

/// p12 (full). In full access no question reaches the human: the CLI sends no callback for
/// `AskUserQuestion` there (PORT_NOTES of `hoh-harness-claude`), and the turn goes on. This holds
/// the engine to what the notes promise; whether the real CLI keeps it is for the live run.
#[tokio::test]
async fn p12_full_a_question_of_claude_is_not_asked() {
    let workspace = Workspace::new();
    let mut host = engine_with_claude(&workspace, "full").await;
    let asked = host.turn(SESSION, "ask", None).await;
    assert!(asked.is_empty(), "{asked:?}");
    assert!(host.questions.is_empty(), "{:?}", host.questions);
    assert!(workspace.answers().is_empty(), "{:?}", workspace.answers());
    assert_eq!(host.turn_end(SESSION)["kind"], "completed");
    host.shutdown().await;
}

/// The profile's `settingSources: isolated` reaches the command line: no settings file and no MCP
/// server of the human or the folder, in every other respect still confirm.
#[tokio::test]
async fn p13_an_isolated_profile_starts_claude_without_any_settings_source() {
    let workspace = Workspace::with("    settingSources: isolated\n", "");
    let mut host = engine_with_claude(&workspace, "confirm").await;
    host.prompt("hello").await;
    let start = only_session(&workspace, &host);
    assert_eq!(start.value_of("--setting-sources"), Some(""), "{start:?}");
    assert!(start.has("--strict-mcp-config"), "{start:?}");
    assert_eq!(mode_of(&start), "confirm", "{start:?}");
    assert_eq!(
        start.value_of("--permission-prompt-tool"),
        Some("stdio"),
        "{start:?}"
    );
    assert_effective_is_the_launch(&mut host, &start).await;
    host.shutdown().await;
}

/// p14. `hoh/executor/discover` names a version outside the range `unsupported`, and says so in
/// words that agree with the refusal of the chat.
#[tokio::test]
async fn p14_discover_calls_a_version_outside_the_range_unsupported() {
    let workspace = Workspace::setup("", "", "1.0.0");
    let mut host = Host::start(&workspace);
    host.initialize(&workspace).await;
    let found = host
        .ok("hoh/executor/discover", json!({"executor": "claude"}))
        .await;
    assert_eq!(found["installed"], true, "{found}");
    assert_eq!(found["version"], "1.0.0", "{found}");
    assert_eq!(found["versionStatus"], "unsupported", "{found}");
    let detail = found["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("1.0.0"), "{found}");
    assert!(
        !detail.contains("работа возможна"),
        "discover promises what the start refuses: {found}"
    );
    host.shutdown().await;
}

/// p14. A `claude` that is not there: not installed, the version status `unknown`.
#[tokio::test]
async fn p14_discover_calls_a_missing_claude_not_installed_and_unknown() {
    let workspace = Workspace::new();
    std::fs::remove_file(&workspace.claude).expect("remove the fake claude");
    let mut host = Host::start(&workspace);
    host.initialize(&workspace).await;
    let found = host
        .ok("hoh/executor/discover", json!({"executor": "claude"}))
        .await;
    assert_eq!(found["installed"], false, "{found}");
    assert_eq!(found["versionStatus"], "unknown", "{found}");
    assert_eq!(found["version"], Value::Null, "{found}");
    host.shutdown().await;
}

// ---------------------------------------------------------------- the mode of one conversation
// (CEO 21.11: the window shows the open chat's own mode; review claude-executor-01, logic-e2e-02/03)

/// A change of mode between two turns: the conversation says what its next turn runs in and what
/// its live process runs in, until the next message starts Claude anew; and back again.
#[tokio::test]
async fn s1_a_change_of_mode_is_pending_until_the_next_message_and_works_both_ways() {
    let workspace = Workspace::new();
    let mut host = Host::start(&workspace);
    host.initialize(&workspace).await;
    host.select_claude().await;

    host.turn("chat-a", "hello", None).await;
    let first = workspace.sessions();
    assert_eq!(first.len(), 1, "{first:?}");
    let access = host.access_of("chat-a").await;
    assert_session(&access, "confirm", "confirm", "after the first turn");
    assert_effective_of(&access, &first[0], "after the first turn");
    assert_eq!(access["session"]["model"], "default", "{access}");

    // To full: the idle process in confirm ends at once; the next message starts one in full.
    host.set_access("full", "allowed-once").await;
    assert!(until_dead(first[0].pid).await, "{:?} still runs", first[0]);
    let access = host.access_of("chat-a").await;
    assert_no_process(&access, "full", "full chosen, not yet sent");
    host.turn("chat-a", "hello again", None).await;
    let starts = workspace.sessions();
    assert_eq!(starts.len(), 2, "{starts:?}");
    assert_full(&starts[1]);
    let access = host.access_of("chat-a").await;
    assert_session(&access, "full", "full", "after the turn in full");
    assert_effective_of(&access, &starts[1], "after the turn in full");

    // And back to confirm.
    host.set_access("confirm", "allowed-once").await;
    assert!(
        until_dead(starts[1].pid).await,
        "{:?} still runs",
        starts[1]
    );
    let access = host.access_of("chat-a").await;
    assert_no_process(&access, "confirm", "confirm chosen, not yet sent");
    host.turn("chat-a", "and again", None).await;
    let starts = workspace.sessions();
    assert_eq!(starts.len(), 3, "{starts:?}");
    assert_confirm(&starts[2]);
    let access = host.access_of("chat-a").await;
    assert_session(&access, "confirm", "confirm", "back in confirm");
    assert_effective_of(&access, &starts[2], "back in confirm");

    // Without a conversation the answer is what it was: no `session`.
    assert!(host.access().await.get("session").is_none());
    host.shutdown().await;
}

/// Two chats do not overwrite each other: each shows the process it has, and a chat keeps Claude,
/// its model and its data line when new chats go back to the engine's own model.
#[tokio::test]
async fn s2_two_chats_keep_their_own_mode_and_a_chat_stays_on_claude_when_new_chats_leave_it() {
    let workspace = Workspace::new();
    let mut host = Host::start(&workspace);
    host.initialize(&workspace).await;
    host.select_model("sonnet").await;

    host.turn("chat-a", "hello", None).await;
    host.set_access("full", "allowed-once").await;
    host.turn("chat-b", "hello", None).await;
    let starts = workspace.sessions();
    assert_eq!(starts.len(), 2, "{starts:?}");
    assert_confirm(&starts[0]);
    assert_full(&starts[1]);

    // The chat started last does not stand in for the other.
    let a = host.access_of("chat-a").await;
    assert!(
        until_dead(starts[0].pid).await,
        "{:?} still runs",
        starts[0]
    );
    assert_no_process(&a, "full", "chat-a");
    let b = host.access_of("chat-b").await;
    assert_session(&b, "full", "full", "chat-b");
    assert_effective_of(&b, &starts[1], "chat-b");

    // New chats return to the engine's own model: the open chats stay on Claude.
    host.select_engine_model().await;
    let a = host.access_of("chat-a").await;
    assert_eq!(a["selected"]["executor"], Value::Null, "{a}");
    assert_no_process(&a, "full", "chat-a after the return");
    assert_eq!(a["session"]["model"], "sonnet", "{a}");
    // A conversation the engine's loop drives (or none) has no executor, mode or process.
    let other = host.access_of("chat-unknown").await;
    assert_eq!(
        other["session"],
        json!({"executor": null, "model": null, "privacy": null,
               "mode": null, "running": null, "pending": false}),
        "{other}"
    );
    // The next message of the chat still goes to Claude, in the mode chosen now.
    host.turn("chat-a", "still here", None).await;
    let starts = workspace.sessions();
    assert_eq!(starts.len(), 3, "{starts:?}");
    assert_full(&starts[2]);
    assert_eq!(
        starts[2].value_of("--model"),
        Some("sonnet"),
        "{:?}",
        starts[2]
    );
    host.shutdown().await;
}

// ---------------------------------------------------------------- i7 additions (verify-4)

/// Whether a process still runs (a zombie that nobody collected does not count: it is dead). Its
/// name, before the state, may be no UTF-8.
fn alive(pid: u32) -> bool {
    std::fs::read(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            let end_of_name = stat.iter().rposition(|&byte| byte == b')')?;
            stat.get(end_of_name + 2).copied()
        })
        .is_some_and(|state| state != b'Z')
}

async fn until_dead(pid: u32) -> bool {
    for _ in 0..100 {
        if !alive(pid) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    false
}

impl Host {
    /// A prompt, then "Стоп" (`session/cancel`) once the turn runs; returns when the chat is idle.
    async fn turn_then_stop(&mut self, session: &str, text: &str) {
        let params =
            json!({"sessionId": session, "contentBlocks": [{"type": "text", "text": text}]});
        let id = self.send("session/prompt", params).await;
        let (mut answered, mut running, mut worked) = (false, false, false);
        while !(answered && running && worked) {
            let frame = self.next().await;
            if frame["id"] == id.as_str() && frame.get("method").is_none() {
                answered = true;
            }
            if frame["method"] == "session.status" && frame["params"]["sessionId"] == session {
                running |= frame["params"]["status"] == "running";
            }
            // The fake has said it is working: the turn is in the middle.
            worked |= frame["method"] == "session.event" && frame.to_string().contains("working");
        }
        let cancel = self
            .send("session/cancel", json!({"sessionId": session}))
            .await;
        let (mut cancelled, mut idle) = (false, false);
        while !(cancelled && idle) {
            let frame = self.next().await;
            if frame["id"] == cancel.as_str() && frame.get("method").is_none() {
                assert!(frame.get("error").is_none(), "{frame}\n{}", self.context());
                cancelled = true;
            }
            if frame["method"] == "session.status"
                && frame["params"]["sessionId"] == session
                && frame["params"]["status"] == "idle"
            {
                idle = true;
            }
        }
    }
}

/// Блокер 1. Choosing a mode ends the process that idles between turns in the old one: no turn it
/// started on its own could run in the old mode. The conversation then shows no live process, and
/// its next message starts Claude in the chosen mode.
#[tokio::test]
async fn t1_a_mode_chosen_between_turns_ends_the_idle_process_at_once() {
    let workspace = Workspace::new();
    let mut host = Host::start(&workspace);
    host.initialize(&workspace).await;
    host.select_claude().await;
    host.turn("chat-a", "hello", None).await;
    host.turn("chat-b", "hello", None).await;
    let starts = workspace.sessions();
    assert_eq!(starts.len(), 2, "{starts:?}");
    assert!(starts.iter().all(|start| alive(start.pid)), "{starts:?}");

    host.set_access("full", "allowed-once").await;
    // Both idle processes were started in confirm: both end, whatever the chat.
    for start in &starts {
        assert!(until_dead(start.pid).await, "{start:?} still runs");
    }
    let a = host.access_of("chat-a").await;
    assert_eq!(a["session"]["mode"], "full", "{a}");
    assert_eq!(a["session"]["running"], Value::Null, "{a}");
    assert_eq!(a["session"]["pending"], false, "{a}");
    assert_eq!(
        workspace.sessions().len(),
        2,
        "no new start before a message"
    );

    host.turn("chat-a", "hello again", None).await;
    let starts = workspace.sessions();
    assert_eq!(starts.len(), 3, "{starts:?}");
    assert_full(&starts[2]);
    assert_session(
        &host.access_of("chat-a").await,
        "full",
        "full",
        "next message",
    );
    // The other chat's process is not started until it is written to.
    assert_eq!(
        host.access_of("chat-b").await["session"]["running"],
        Value::Null
    );
    host.shutdown().await;
}

/// Блокер 2. "Стоп" in the middle of a turn: the turn ends as stopped, the CLI does not hang, and
/// the next turn works. Since the pilot of 22.11 «Стоп» ends the CLI whether it listens to its
/// interrupt or not, and the next turn starts a new one.
#[tokio::test]
async fn t2_stop_in_the_middle_of_a_turn_ends_it_and_the_next_turn_works() {
    let workspace = Workspace::with(
        "",
        "- id: external-harness\n  config:\n    interruptGraceMs: 400\n    terminateGraceMs: 400\n",
    );
    let mut host = Host::start(&workspace);
    host.initialize(&workspace).await;
    host.select_claude().await;

    // A CLI that listens to its interrupt, and one that does not.
    for (chat, text) in [("chat-s", "hang"), ("chat-d", "hang-deaf")] {
        host.turn_then_stop(chat, text).await;
        let reason = host.turn_end(chat);
        assert_eq!(reason["kind"], "aborted", "{reason}\n{}", host.context());
        let all = workspace.sessions();
        let stopped = all.last().expect("the CLI started");
        assert!(until_dead(stopped.pid).await, "{stopped:?} still runs");
        host.turn(chat, "hello", None).await;
        let after = workspace.sessions();
        assert_eq!(after.len(), all.len() + 1, "{after:?}");
        assert_eq!(
            host.turn_end(chat)["kind"],
            "completed",
            "{}",
            host.context()
        );
    }
    host.shutdown().await;
}

/// Блокер 2. After the engine restarts on the same data the chat goes on: the CLI is started with
/// `--resume` and the id it had, and its answer remembers the earlier turn.
#[tokio::test]
async fn t3_a_chat_goes_on_after_the_engine_restarts_and_remembers_the_earlier_turn() {
    let workspace = Workspace::new();
    let mut host = Host::start(&workspace);
    host.initialize(&workspace).await;
    host.select_claude().await;
    host.turn("chat-r", "remember blue", None).await;
    let first = workspace.sessions();
    assert_eq!(first.len(), 1, "{first:?}");
    let native = first[0]
        .value_of("--session-id")
        .expect("a new session")
        .to_owned();
    host.shutdown().await;

    let mut host = Host::start(&workspace);
    host.initialize(&workspace).await;
    host.ok("session/resume", json!({"sessionId": "chat-r"}))
        .await;
    host.turn("chat-r", "recall", None).await;
    let all = workspace.sessions();
    assert_eq!(all.len(), 2, "{all:?}");
    assert_eq!(
        all[1].value_of("--resume"),
        Some(native.as_str()),
        "{:?}",
        all[1]
    );
    assert!(!all[1].has("--session-id"), "{:?}", all[1]);
    let said = host
        .frames
        .iter()
        .any(|frame| frame.to_string().contains("you said: blue"));
    assert!(
        said,
        "the answer does not remember the earlier turn\n{}",
        host.context()
    );
    host.shutdown().await;
}

/// Блокер 3. The window of the protection names Claude as a word and says what it asks for; the
/// explanation is empty when there is nothing to add; the risk is not claimed (unassessed).
#[tokio::test]
async fn t4_the_request_of_claude_is_worded_for_a_person_and_its_risk_is_not_claimed() {
    let workspace = Workspace::new();
    let mut host = engine_with_claude(&workspace, "confirm").await;

    let asked = host.turn(SESSION, "run", Some("allowed-once")).await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(
        asked[0].human_text,
        "Claude просит выполнить команду: touch ran.txt"
    );
    assert_eq!(
        asked[0].risk,
        hoh_protocol::Risk::Unassessed,
        "{:?}",
        asked[0]
    );
    assert!(
        asked[0].reason.as_deref().unwrap_or("").is_empty(),
        "{:?}",
        asked[0]
    );
    assert!(
        asked[0]
            .details
            .as_deref()
            .unwrap_or("")
            .contains("touch ran.txt"),
        "{:?}",
        asked[0]
    );
    let asked = host.turn(SESSION, "edit", Some("allowed-once")).await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    let path = workspace.folder.join("notes.txt");
    assert_eq!(
        asked[0].human_text,
        format!("Claude просит изменить файл {}", path.display())
    );
    for request in host
        .frames
        .iter()
        .filter(|f| f["method"] == "approval/request")
    {
        let text = request["params"]["humanText"].as_str().unwrap_or_default();
        assert!(
            !text.contains("Исполнитель") && !text.contains('«'),
            "{text}"
        );
    }
    // What the classifier judges dangerous stays high.
    let asked = host.turn(SESSION, "remove", Some("rejected")).await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(asked[0].risk, hoh_protocol::Risk::High, "{:?}", asked[0]);
    host.shutdown().await;
}

/// Финальное ревью Fable, B. A character the window may not show (U+FE0F, U+00A0, U+200B) in a
/// command or a file name does not take the words away: the headline still says what Claude asks
/// for, that character as a space, and the risk stays unassessed.
#[tokio::test]
async fn t4b_an_invisible_character_keeps_the_request_worded_for_a_person() {
    let workspace = Workspace::new();
    let mut host = engine_with_claude(&workspace, "confirm").await;
    let cases = [
        (
            "run: echo \"ok \u{2714}\u{fe0f}\"",
            "Claude просит выполнить команду: echo \"ok \u{2714} \"".to_owned(),
        ),
        (
            "run: echo a\u{a0}b",
            "Claude просит выполнить команду: echo a b".to_owned(),
        ),
        (
            "edit: no\u{200b}tes.txt",
            format!(
                "Claude просит изменить файл {}",
                workspace.folder.join("no tes.txt").display()
            ),
        ),
    ];
    for (prompt, headline) in cases {
        let asked = host.turn(SESSION, prompt, Some("rejected")).await;
        assert_eq!(asked.len(), 1, "{prompt}: {asked:?}");
        assert_eq!(asked[0].human_text, headline, "{prompt}: {:?}", asked[0]);
        assert_eq!(
            asked[0].risk,
            hoh_protocol::Risk::Unassessed,
            "{prompt}: {:?}",
            asked[0]
        );
    }
    host.shutdown().await;
}

/// Пункт 6. A chat with Claude does not take a folder change: the refusal has the code in front
/// and the words for the person, and the folder stays.
#[tokio::test]
async fn t5_a_chat_with_claude_does_not_take_a_change_of_folder() {
    let workspace = Workspace::new();
    let mut host = Host::start(&workspace);
    host.initialize(&workspace).await;
    host.select_claude().await;
    host.turn("chat-f", "hello", None).await;
    let before = host
        .ok(
            "session/working-directory/get",
            json!({"sessionId": "chat-f"}),
        )
        .await;
    let other = workspace.root.path().join("elsewhere");
    std::fs::create_dir_all(&other).expect("another folder");

    let frame = host
        .privileged(
            "session/working-directory/set",
            json!({"sessionId": "chat-f", "path": other}),
            "allowed-once",
        )
        .await;
    let message = frame["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("{frame}"));
    assert_eq!(
        message,
        "EXECUTOR_FOLDER_FIXED: Чат с Claude работает в одной папке. Чтобы работать в другой, \
         начните новый чат"
    );
    let after = host
        .ok(
            "session/working-directory/get",
            json!({"sessionId": "chat-f"}),
        )
        .await;
    assert_eq!(before, after);
    host.turn("chat-f", "hello", None).await;
    assert_eq!(
        workspace.sessions().last().expect("started").cwd,
        workspace.folder
    );
    host.shutdown().await;
}

/// Ревью i7, п. 1. «Новый чат в другой папке»: a new chat with Claude takes another folder before
/// its first turn, the turn runs there; after that turn the folder is the conversation's own.
#[tokio::test]
async fn t6_a_new_claude_chat_takes_a_folder_before_its_first_turn_and_keeps_it_after() {
    let workspace = Workspace::new();
    let mut host = Host::start(&workspace);
    host.initialize(&workspace).await;
    host.select_claude().await;
    // What the window does for a new chat: it reads the folder (the chat is created), then sets it.
    host.ok(
        "session/working-directory/get",
        json!({"sessionId": "chat-new"}),
    )
    .await;
    let other = workspace.root.path().join("elsewhere");
    std::fs::create_dir_all(&other).expect("another folder");
    let other = other.canonicalize().expect("canonical folder");
    let frame = host
        .privileged(
            "session/working-directory/set",
            json!({"sessionId": "chat-new", "path": other}),
            "allowed-once",
        )
        .await;
    assert!(
        frame.get("result").is_some(),
        "a new chat refused the folder: {frame}\n{}",
        host.context()
    );
    host.turn("chat-new", "hello", None).await;
    let start = only_session(&workspace, &host);
    assert_eq!(
        start.cwd.canonicalize().expect("canonical cwd"),
        other,
        "{start:?}"
    );

    let frame = host
        .privileged(
            "session/working-directory/set",
            json!({"sessionId": "chat-new", "path": workspace.folder}),
            "allowed-once",
        )
        .await;
    let message = frame["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("the folder changed after the turn: {frame}"));
    assert!(message.starts_with("EXECUTOR_FOLDER_FIXED: "), "{message}");
    host.turn("chat-new", "hello", None).await;
    let last = workspace.sessions().pop().expect("started");
    assert_eq!(
        last.cwd.canonicalize().expect("canonical cwd"),
        other,
        "{last:?}"
    );
    host.shutdown().await;
}

/// Ревью i7, п. 2 и 3. After the engine restarts, a chat with Claude keeps what it runs with:
/// `access/get{sessionId}` gives the same executor, mode and data line (before the window resumes
/// it, from the log; after, from the live conversation, with its own model), and the next turn
/// starts the CLI with the conversation's model, never the engine's route (`window-mock`).
#[tokio::test]
async fn t7_after_the_engine_restarts_a_claude_chat_keeps_its_model_and_its_mode() {
    let workspace = Workspace::new();
    let mut host = Host::start(&workspace);
    host.initialize(&workspace).await;
    host.select_model("sonnet").await;
    host.set_access("full", "allowed-once").await;
    host.turn("chat-m", "hello", None).await;
    let before = host.access_of("chat-m").await;
    assert_eq!(before["session"]["model"], "sonnet", "{before}");
    assert_session(&before, "full", "full", "before the restart");
    host.shutdown().await;

    let mut host = Host::start(&workspace);
    host.initialize(&workspace).await;
    let stored = host.access_of("chat-m").await;
    assert_eq!(stored["session"]["executor"], "claude", "{stored}");
    assert_eq!(stored["session"]["privacy"], "remote", "{stored}");
    assert_no_process(&stored, "full", "after the restart, before the resume");
    host.ok("session/resume", json!({"sessionId": "chat-m"}))
        .await;
    let resumed = host.access_of("chat-m").await;
    assert_eq!(resumed["session"]["model"], "sonnet", "{resumed}");
    assert_eq!(resumed["session"]["executor"], "claude", "{resumed}");
    assert_eq!(resumed["session"]["privacy"], "remote", "{resumed}");
    assert_no_process(&resumed, "full", "after the resume");

    host.turn("chat-m", "hello", None).await;
    let all = workspace.sessions();
    assert_eq!(all.len(), 2, "{all:?}");
    assert!(all[1].has("--resume"), "{:?}", all[1]);
    assert_eq!(all[1].value_of("--model"), Some("sonnet"), "{:?}", all[1]);
    let after = host.access_of("chat-m").await;
    assert_eq!(after["session"]["model"], "sonnet", "{after}");
    assert_session(&after, "full", "full", "after the next turn");
    host.shutdown().await;
}

/// Финальное ревью Fable, C. The choices the window saved (`external-harness.json` next to the
/// profile) are what the next engine runs with: the mode stays `full`, a new chat goes to Claude,
/// and Claude starts with the full-access switches. A damaged file leaves the defaults (confirm).
#[tokio::test]
async fn t8_the_saved_choices_are_what_the_next_engine_runs_with() {
    let workspace = Workspace::new();
    let host = engine_with_claude(&workspace, "full").await;
    host.shutdown().await;
    let saved = workspace.profile().join("external-harness.json");
    assert!(saved.is_file(), "no saved choices at {}", saved.display());

    let mut host = Host::start(&workspace);
    host.initialize(&workspace).await;
    let access = host.access().await;
    assert_eq!(access["mode"], "full", "{access}");
    assert_eq!(access["selected"]["executor"], "claude", "{access}");
    host.prompt("hello").await;
    let start = only_session(&workspace, &host);
    assert_full(&start);
    host.shutdown().await;

    std::fs::write(&saved, "{ not json").expect("damage the saved choices");
    let mut host = Host::start(&workspace);
    host.initialize(&workspace).await;
    let access = host.access().await;
    assert_eq!(access["mode"], "confirm", "{access}");
    assert_eq!(access["selected"]["executor"], Value::Null, "{access}");
    host.select_claude().await;
    host.turn("chat-after-damage", "hello", None).await;
    let last = workspace.sessions().pop().expect("started");
    assert_confirm(&last);
    host.shutdown().await;
}

/// Итоговое ревью Fable cand/i8. «Стоп» while Claude is still starting (no answer to `initialize`
/// yet): the turn ends as stopped and no `claude` process is left behind (in full access it would
/// run with `bypassPermissions`).
#[tokio::test]
async fn t9_stop_while_claude_starts_leaves_no_process() {
    let workspace = Workspace::new();
    let mut host = engine_with_claude(&workspace, "full").await;
    let start = stop_at_launch(&mut host, &workspace).await;
    assert_eq!(
        host.turn_end(SESSION)["kind"],
        "aborted",
        "{}",
        host.context()
    );
    assert!(until_dead(start.pid).await, "{start:?} outlived the stop");
    host.shutdown().await;
}

/// A message to the chat [`SESSION`], and «Стоп» while the Claude it starts does not yet answer
/// `initialize`; returns that start once the chat is idle.
async fn stop_at_launch(host: &mut Host, workspace: &Workspace) -> Start {
    let before = workspace.sessions().len();
    let slow = workspace.root.path().join("slow-initialize");
    std::fs::write(&slow, "").expect("a slow start");
    let params =
        json!({"sessionId": SESSION, "contentBlocks": [{"type": "text", "text": "hello"}]});
    let prompt = host.send("session/prompt", params).await;
    let until = std::time::Instant::now() + FRAME_TIMEOUT;
    let start = loop {
        let mut sessions = workspace.sessions();
        if sessions.len() > before {
            break sessions.remove(before);
        }
        assert!(
            std::time::Instant::now() < until,
            "claude did not start\n{}",
            host.context()
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    let cancel = host
        .send("session/cancel", json!({"sessionId": SESSION}))
        .await;
    let (mut answered, mut cancelled, mut idle) = (false, false, false);
    while !(answered && cancelled && idle) {
        let frame = host.next().await;
        if frame.get("method").is_none() {
            answered |= frame["id"] == prompt.as_str();
            if frame["id"] == cancel.as_str() {
                assert!(frame.get("error").is_none(), "{frame}\n{}", host.context());
                cancelled = true;
            }
        }
        idle |= frame["method"] == "session.status"
            && frame["params"]["sessionId"] == SESSION
            && frame["params"]["status"] == "idle";
    }
    std::fs::remove_file(&slow).expect("the start is quick again");
    start
}

/// The turn of `session` in the middle: `text` ("hang", "long") sent, the fake says it is working.
async fn hang_turn(host: &mut Host, session: &str, text: &str) {
    let params = json!({"sessionId": session, "contentBlocks": [{"type": "text", "text": text}]});
    host.send("session/prompt", params).await;
    while !host.frames.iter().any(|frame| {
        frame["method"] == "session.event"
            && frame["params"]["sessionId"] == session
            && frame.to_string().contains("working")
    }) {
        host.next().await;
    }
}

/// «Стоп» of the running turn of `session`; returns when the chat is idle.
async fn stop_turn(host: &mut Host, session: &str) {
    let cancel = host
        .send("session/cancel", json!({"sessionId": session}))
        .await;
    let (mut cancelled, mut idle) = (false, false);
    while !(cancelled && idle) {
        let frame = host.next().await;
        if frame["id"] == cancel.as_str() && frame.get("method").is_none() {
            assert!(frame.get("error").is_none(), "{frame}\n{}", host.context());
            cancelled = true;
        }
        idle |= frame["method"] == "session.status"
            && frame["params"]["sessionId"] == session
            && frame["params"]["status"] == "idle";
    }
}

/// Ревью Fable cand/i9 (152628). The mode changed and changed back between turns: the idle
/// process ended at the first change, so the next message starts Claude anew in the mode in
/// effect, continues the conversation (`--resume`) and gets its answer. Choosing the mode already
/// in effect ends nothing. A change during a turn leaves that turn's process alone; the next
/// message starts one in the new mode.
#[tokio::test]
async fn t10_a_mode_changed_and_changed_back_between_turns_starts_claude_anew() {
    let workspace = Workspace::new();
    let mut host = Host::start(&workspace);
    host.initialize(&workspace).await;
    host.select_claude().await;
    host.turn("chat-a", "hello", None).await;
    let first = only_session(&workspace, &host);
    let native = first
        .value_of("--session-id")
        .expect("a new session")
        .to_owned();

    host.set_access("full", "allowed-once").await;
    host.set_access("confirm", "allowed-once").await;
    assert!(until_dead(first.pid).await, "{first:?} still runs");
    host.turn("chat-a", "hello", None).await;
    let end = host.turn_end("chat-a");
    assert_eq!(end["kind"], "completed", "{end}\n{}", host.context());
    let starts = workspace.sessions();
    assert_eq!(starts.len(), 2, "{starts:?}");
    assert_confirm(&starts[1]);
    assert_eq!(
        starts[1].value_of("--resume"),
        Some(native.as_str()),
        "{:?}",
        starts[1]
    );

    host.set_access("confirm", "allowed-once").await;
    host.turn("chat-a", "hello", None).await;
    assert_eq!(host.turn_end("chat-a")["kind"], "completed");
    assert_eq!(workspace.sessions().len(), 2, "the same mode ended nothing");
    assert!(alive(starts[1].pid), "{:?}", starts[1]);

    hang_turn(&mut host, "chat-a", "hang").await;
    host.set_access("full", "allowed-once").await;
    assert!(alive(starts[1].pid), "the turn's process ended under it");
    stop_turn(&mut host, "chat-a").await;
    host.turn("chat-a", "hello", None).await;
    assert_eq!(host.turn_end("chat-a")["kind"], "completed");
    let starts = workspace.sessions();
    assert_eq!(starts.len(), 3, "{starts:?}");
    assert_full(&starts[2]);
    assert!(
        until_dead(starts[1].pid).await,
        "{:?} still runs",
        starts[1]
    );
    host.shutdown().await;
}

/// Ревью Fable cand/i9 (152628). A session id of the CLI that is not of the UUID form never goes
/// after `--resume` (it would read as a switch): the conversation counts as lost and the next
/// start opens a new native session.
#[tokio::test]
async fn t11_a_session_id_that_is_not_a_uuid_is_never_resumed() {
    let workspace = Workspace::new();
    let mut host = engine_with_claude(&workspace, "confirm").await;
    let odd = workspace.root.path().join("odd-session-id");
    std::fs::write(&odd, "").expect("an odd session id");
    host.prompt("hello").await;
    std::fs::remove_file(&odd).expect("ordinary ids again");

    host.set_access("full", "allowed-once").await;
    host.prompt("hello").await;
    assert_eq!(host.turn_end(SESSION)["kind"], "completed");
    let starts = workspace.sessions();
    assert_eq!(starts.len(), 2, "{starts:?}");
    let next = &starts[1];
    assert!(next.has("--session-id"), "{next:?}");
    assert!(!next.has("--resume"), "{next:?}");
    assert!(!next.has("--dangerously-skip-permissions"), "{next:?}");
    host.shutdown().await;
}

/// Решение CEO 22.11 16:06, обратный путь к `t10_…`: full → confirm → full between turns. The idle
/// process in full ended at the first change; the next message starts Claude anew in full,
/// continues the conversation (`--resume`) and its answer remembers the earlier turn.
#[tokio::test]
async fn t10b_a_mode_changed_back_to_full_between_turns_starts_claude_anew() {
    let workspace = Workspace::new();
    let mut host = engine_with_claude(&workspace, "full").await;
    host.turn("chat-f", "remember green", None).await;
    let first = only_session(&workspace, &host);
    assert_full(&first);
    let native = first
        .value_of("--session-id")
        .expect("a new session")
        .to_owned();

    host.set_access("confirm", "allowed-once").await;
    host.set_access("full", "allowed-once").await;
    assert!(until_dead(first.pid).await, "{first:?} still runs");
    host.turn("chat-f", "recall", None).await;
    let end = host.turn_end("chat-f");
    assert_eq!(end["kind"], "completed", "{end}\n{}", host.context());
    let starts = workspace.sessions();
    assert_eq!(starts.len(), 2, "{starts:?}");
    assert_full(&starts[1]);
    assert_eq!(
        starts[1].value_of("--resume"),
        Some(native.as_str()),
        "{:?}",
        starts[1]
    );
    let said = host
        .frames
        .iter()
        .any(|frame| frame.to_string().contains("you said: green"));
    assert!(said, "no answer to the message\n{}", host.context());
    host.shutdown().await;
}

/// The pid of the long command the CLI of `start` runs (`long`).
fn command_of(workspace: &Workspace, start: &Start) -> u32 {
    let path = workspace
        .root
        .path()
        .join("claude-commands")
        .join(start.pid.to_string());
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
        .trim()
        .parse()
        .expect("a pid")
}

/// Пилот 22.11, «Стоп» (блокер) in the middle of a turn whose command runs in a session of its
/// own: the CLI and the command end; the next message starts the CLI anew on the same
/// conversation (`--resume`) and nothing of the stopped turn reaches it.
async fn stop_ends_claude_and_its_command(mode: &str, long: &str) {
    let workspace = Workspace::new();
    let mut host = engine_with_claude(&workspace, mode).await;
    hang_turn(&mut host, SESSION, long).await;
    let first = only_session(&workspace, &host);
    let native = first
        .value_of("--session-id")
        .expect("a new session")
        .to_owned();
    let command = command_of(&workspace, &first);
    assert!(alive(first.pid) && alive(command), "{first:?}, {command}");
    stop_turn(&mut host, SESSION).await;
    let end = host.turn_end(SESSION);
    assert_eq!(end["kind"], "aborted", "{end}\n{}", host.context());
    assert!(until_dead(first.pid).await, "{first:?} outlived «Стоп»");
    assert!(
        until_dead(command).await,
        "its command {command} outlived «Стоп»"
    );
    let access = host.access_of(SESSION).await;
    assert_eq!(access["session"]["running"], Value::Null, "{access}");

    let from = host.frames.len();
    host.prompt("hello").await;
    let end = host.turn_end(SESSION);
    assert_eq!(end["kind"], "completed", "{end}\n{}", host.context());
    let starts = workspace.sessions();
    assert_eq!(starts.len(), 2, "{starts:?}");
    assert_eq!(mode_of(&starts[1]), mode, "{:?}", starts[1]);
    assert_eq!(
        starts[1].value_of("--resume"),
        Some(native.as_str()),
        "{:?}",
        starts[1]
    );
    let next = host.frames[from..]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        next.contains("\"done\""),
        "no answer to the message\n{next}"
    );
    assert!(
        !next.contains("working") && !next.contains("sleep 120"),
        "the stopped turn reached the next one\n{next}"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn t12_confirm_stop_ends_claude_and_its_command() {
    stop_ends_claude_and_its_command("confirm", "long").await;
}

#[tokio::test]
async fn t12_full_stop_ends_claude_and_its_command() {
    stop_ends_claude_and_its_command("full", "long").await;
}

/// Ревью Fable 174941: a command named `проверка.sh` — the kernel keeps 15 bytes of a name and cuts
/// its `а` in half, so its `/proc/<pid>/stat` is no UTF-8 — still ends on «Стоп».
#[tokio::test]
async fn t17_stop_ends_a_command_whose_name_is_no_utf8() {
    stop_ends_claude_and_its_command("confirm", "long проверка.sh").await;
}

/// Пилот 22.11, «Стоп» (блокер) at the launch of Claude: the mode changed between turns, so the
/// next message starts Claude anew, and «Стоп» comes before it answers `initialize`. No process is
/// left; the next message starts it again on the same conversation.
async fn stop_at_launch_then_resume(mode: &str) {
    let other = if mode == "full" { "confirm" } else { "full" };
    let workspace = Workspace::new();
    let mut host = engine_with_claude(&workspace, other).await;
    host.prompt("hello").await;
    let first = only_session(&workspace, &host);
    let native = first
        .value_of("--session-id")
        .expect("a new session")
        .to_owned();
    host.set_access(mode, "allowed-once").await;
    assert!(until_dead(first.pid).await, "{first:?} still runs");

    let launched = stop_at_launch(&mut host, &workspace).await;
    let end = host.turn_end(SESSION);
    assert_eq!(end["kind"], "aborted", "{end}\n{}", host.context());
    assert!(
        until_dead(launched.pid).await,
        "{launched:?} outlived «Стоп»"
    );

    host.prompt("hello").await;
    let end = host.turn_end(SESSION);
    assert_eq!(end["kind"], "completed", "{end}\n{}", host.context());
    let starts = workspace.sessions();
    assert_eq!(starts.len(), 3, "{starts:?}");
    for start in &starts[1..] {
        assert_eq!(mode_of(start), mode, "{start:?}");
        assert_eq!(
            start.value_of("--resume"),
            Some(native.as_str()),
            "{start:?}"
        );
    }
    host.shutdown().await;
}

#[tokio::test]
async fn t13_confirm_stop_at_the_launch_leaves_no_process_and_the_chat_goes_on() {
    stop_at_launch_then_resume("confirm").await;
}

#[tokio::test]
async fn t13_full_stop_at_the_launch_leaves_no_process_and_the_chat_goes_on() {
    stop_at_launch_then_resume("full").await;
}

/// Пилот 22.11, `live.v5.kill-full` (блокер). The engine killed with SIGKILL in the middle of a
/// long command: neither Claude nor its command (in a session of its own) outlives it.
#[tokio::test]
async fn t14_killing_the_engine_ends_claude_and_its_command() {
    let workspace = Workspace::new();
    let mut host = engine_with_claude(&workspace, "full").await;
    hang_turn(&mut host, SESSION, "long").await;
    let start = only_session(&workspace, &host);
    let command = command_of(&workspace, &start);
    assert!(alive(start.pid) && alive(command), "{start:?}, {command}");
    host.child.start_kill().expect("SIGKILL to the engine");
    host.child.wait().await.expect("the engine is gone");
    assert!(until_dead(start.pid).await, "{start:?} outlived the engine");
    assert!(
        until_dead(command).await,
        "its command {command} outlived the engine (claude {})",
        start.pid
    );
}

/// Пилот 22.11, the answer shown twice: a one-word answer is held back whole while it streams (it
/// may be the start of a key), and must still reach the window as live text before the recorded
/// answer that takes its place — a piece after the record is shown as a second answer.
#[tokio::test]
async fn t15_a_one_word_answer_reaches_the_window_before_its_record() {
    let workspace = Workspace::new();
    let mut host = engine_with_claude(&workspace, "confirm").await;
    let from = host.frames.len();
    host.prompt("short").await;
    let frames = &host.frames[from..];
    let records: Vec<usize> = (0..frames.len())
        .filter(|&at| {
            frames[at]["method"] == "session.event"
                && frames[at]["params"]["event"]["type"] == "assistant/message"
        })
        .collect();
    let chunks: Vec<usize> = (0..frames.len())
        .filter(|&at| {
            frames[at]["method"] == "session.assistant-stream"
                && frames[at]["params"]["frame"]["type"] == "chunk"
        })
        .collect();
    let shown: String = chunks
        .iter()
        .filter_map(|&at| frames[at]["params"]["frame"]["chunk"]["text"].as_str())
        .collect();
    let all = frames
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(records.len(), 1, "{all}");
    assert_eq!(shown, "ok1", "{all}");
    assert!(
        chunks.iter().all(|&at| at < records[0]),
        "live text after its record\n{all}"
    );
}

/// Предварительная живая проверка 939feabd (s3, 22.11): «Стоп» about a second after a message to
/// Claude resumed after an earlier «Стоп». The resumed CLI first answers the notification of the
/// background command lost with the old process — a result of no turn. Taken for the message's
/// answer, it closed the turn at once: the window went idle and its «Стоп» was gone, the CLI went
/// on with the message between turns, and the next message went to the same process. Now the turn
/// runs until its own answer: «Стоп» ends the CLI and its command, the turn ends as stopped, and
/// the next message resumes the conversation in a new process.
#[tokio::test]
async fn t16_stop_right_after_a_message_to_a_resumed_claude_ends_it() {
    let workspace = Workspace::new();
    let mut host = engine_with_claude(&workspace, "confirm").await;
    hang_turn(&mut host, SESSION, "long").await;
    let first = only_session(&workspace, &host);
    let native = first
        .value_of("--session-id")
        .expect("a new session")
        .to_owned();
    stop_turn(&mut host, SESSION).await;
    assert!(until_dead(first.pid).await, "{first:?} outlived «Стоп»");

    std::fs::write(workspace.root.path().join("wake-on-resume"), "").expect("a lost command");
    let params = json!({"sessionId": SESSION, "contentBlocks": [{"type": "text", "text": "long"}]});
    host.send("session/prompt", params).await;
    // The resumed CLI has answered the notification and runs the message's command.
    let until = std::time::Instant::now() + FRAME_TIMEOUT;
    let commands = workspace.root.path().join("claude-commands");
    let (second, command) = loop {
        let mut sessions = workspace.sessions();
        if sessions.len() > 1 && commands.join(sessions[1].pid.to_string()).exists() {
            let second = sessions.remove(1);
            let command = command_of(&workspace, &second);
            break (second, command);
        }
        assert!(
            std::time::Instant::now() < until,
            "claude did not run the message\n{}",
            host.context()
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    assert_eq!(
        second.value_of("--resume"),
        Some(native.as_str()),
        "{second:?}"
    );

    let cancel = host
        .send("session/cancel", json!({"sessionId": SESSION}))
        .await;
    let stopped = |host: &Host| {
        host.frames.iter().find_map(|frame| {
            let event = &frame["params"]["event"];
            (frame["method"] == "session.event"
                && frame["params"]["sessionId"] == SESSION
                && event["type"] == "turn/end"
                && event["data"]["turn"] == 2)
                .then(|| event["data"]["reason"].clone())
        })
    };
    let mut answered = false;
    while !answered || stopped(&host).is_none() {
        let frame = host.next().await;
        if frame["id"] == cancel.as_str() && frame.get("method").is_none() {
            assert!(frame.get("error").is_none(), "{frame}\n{}", host.context());
            answered = true;
        }
    }
    let reason = stopped(&host).expect("the end of the turn");
    assert_eq!(reason["kind"], "aborted", "{reason}\n{}", host.context());
    assert!(until_dead(second.pid).await, "{second:?} outlived «Стоп»");
    assert!(
        until_dead(command).await,
        "its command {command} outlived «Стоп»"
    );

    host.prompt("hello").await;
    let end = host.turn_end(SESSION);
    assert_eq!(end["kind"], "completed", "{end}\n{}", host.context());
    let starts = workspace.sessions();
    assert_eq!(starts.len(), 3, "{starts:?}");
    assert_eq!(
        starts[2].value_of("--resume"),
        Some(native.as_str()),
        "{:?}",
        starts[2]
    );
    host.shutdown().await;
}
