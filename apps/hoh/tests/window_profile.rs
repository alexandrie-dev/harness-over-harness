use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};

#[test]
fn window_profile_composes_every_required_engine_service() {
    use hoh_loader::{BundleSource, Layer, LayerSource};

    let bundle = hoh::profile_boot::ShippedBundles
        .bundle("@hoh/window")
        .unwrap();
    let patches = bundle
        .files
        .into_iter()
        .flat_map(|(file, text)| {
            hoh_loader::parse_patches(&text, &Path::new("/window").join(file)).unwrap()
        })
        .collect();
    let mut warnings = Vec::new();
    let composition = hoh_loader::compose(
        &[],
        &[Layer {
            source: LayerSource::Bundle("@hoh/window".into()),
            file: None,
            patches,
        }],
        &mut |warning| warnings.push(warning),
    );
    assert!(warnings.is_empty(), "{warnings:#?}");
    let ids = composition
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    for required in [
        "llm",
        "session",
        "session-projection",
        "session-title",
        "system-prompt",
        "tools",
        "tool-executor-native",
        "agent",
        "filesystem",
        "working-directory",
        "tool-working-directory",
        "tool-fs",
        "subprocess",
        "sandbox",
        "sandbox-policy",
        "bash-sandbox",
        "credentials",
        "model-host",
        "window-model-settings",
        "privileged-approver",
        "session-persistence-jsonl",
        "shell-env",
        "tool-bash",
        "user-questions",
        "user-approval",
        "tool-ask-user",
        "tool-todo",
        "agent-loop",
        "sdk-app-startup",
        "sdk-jsonrpc-server",
    ] {
        assert!(ids.contains(&required), "missing {required}: {ids:#?}");
    }
    let titles = composition
        .entries
        .iter()
        .find(|entry| entry.id == "session-title")
        .unwrap();
    assert_eq!(
        titles.config,
        json!({"fallbackMaxWords":3,"fallbackMaxBytes":80,"maxTitleBytes":80})
    );
    let policy = composition
        .entries
        .iter()
        .find(|entry| entry.id == "sandbox-policy")
        .unwrap();
    assert_eq!(
        policy.config["protectedRoots"]
            .as_array()
            .expect("window sandbox policy protectedRoots")
            .len(),
        1,
        "the window must protect the whole DSH_HOME as one root"
    );
}

async fn send(stdin: &mut tokio::process::ChildStdin, frame: Value) {
    stdin
        .write_all(format!("{frame}\n").as_bytes())
        .await
        .unwrap();
    stdin.flush().await.unwrap();
}

async fn receive_until(
    lines: &mut Lines<BufReader<tokio::process::ChildStdout>>,
    seen: &mut Vec<Value>,
    predicate: impl Fn(&Value) -> bool,
) -> Value {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let line = lines.next_line().await.unwrap().expect("engine stdout EOF");
            let frame: Value = serde_json::from_str(&line).unwrap();
            seen.push(frame.clone());
            if predicate(&frame) {
                return frame;
            }
        }
    })
    .await
    .expect("engine response timeout")
}

/// A user profile that points the model host at the scenario mock; the real key flow is the
/// window end-to-end test's business, the mock needs no key.
fn write_profile(root: &Path, base_url: &str) {
    write_profile_at(&root.join("home"), &root.join("work"), base_url);
}

fn write_profile_at(home: &Path, workspace: &Path, base_url: &str) {
    std::fs::create_dir_all(workspace).unwrap();
    let dir = home.join("profiles").join("window");
    std::fs::create_dir_all(&dir).unwrap();
    // The credentials store refuses a home that others can read: the launcher creates it private.
    std::fs::set_permissions(home, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    std::fs::write(
        dir.join("package.json"),
        json!({"name":"dsh-profile-window","private":true,"dependencies":{},"dsh":{"profile":{"bundles":["@hoh/window"]}}}).to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("cordis.patch.yml"),
        format!("- id: model-host\n  config:\n    baseUrl: {base_url}\n    model: window-mock\n"),
    )
    .unwrap();
}

fn window_workspace(root: &Path) -> std::path::PathBuf {
    let workspace = root.join("work");
    std::fs::create_dir_all(&workspace).unwrap();
    workspace
}

fn command(root: &Path) -> tokio::process::Command {
    command_with_home(root, &root.join("home"))
}

fn command_with_home(root: &Path, dsh_home: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_hoh"));
    command
        .args(["--profile", "window"])
        .env_clear()
        .env("HOME", root)
        .env("DSH_HOME", dsh_home)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(std::fs::File::create(root.join("engine.stderr")).unwrap())
        .kill_on_drop(true);
    command
}

/// What the engine wrote to stderr (a file, so a chatty engine can never block on a full pipe).
fn engine_stderr(root: &Path) -> String {
    std::fs::read_to_string(root.join("engine.stderr")).unwrap()
}

#[tokio::test]
async fn window_profile_initializes_streams_a_mock_prompt_and_shuts_down() {
    let root = tempfile::tempdir().unwrap();
    let mut mock = hoh_llm_openai_mock::MockServer::start(hoh_llm_openai_mock::MockOptions::new(
        hoh_llm_openai_mock::Scenario {
            model: "window-mock".to_owned(),
            responses: vec![
                hoh_llm_openai_mock::ScenarioResponse {
                    text: vec!["window ".to_owned(), "mock answer".to_owned()],
                    tool_calls: vec![],
                    usage: hoh_llm_openai_mock::ScenarioUsage {
                        prompt_tokens: 3,
                        completion_tokens: 2,
                    },
                },
                hoh_llm_openai_mock::ScenarioResponse {
                    text: vec![r#"{"title":"Названия чатов"}"#.to_owned()],
                    tool_calls: vec![],
                    usage: hoh_llm_openai_mock::ScenarioUsage {
                        prompt_tokens: 8,
                        completion_tokens: 4,
                    },
                },
            ],
        },
    ))
    .await
    .unwrap();
    write_profile(root.path(), &mock.base_url);
    let mut child = command(root.path()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut seen = Vec::new();

    let initialize = json!({
        "jsonrpc":"2.0",
        "id":"initialize",
        "method":"initialize",
        "params":{
            "cwd":window_workspace(root.path()),
            "provider":"openai-compatible",
            // The window announces a placeholder: the engine uses the model it was configured with.
            "model":"local",
            "hoh":{"protocolVersion":1,"approvalKey":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
            "capabilities":{"approvals":"host"}
        }
    });
    send(&mut stdin, initialize.clone()).await;
    let initialized =
        receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "initialize").await;
    assert!(initialized.get("result").is_some(), "{initialized}");

    send(
        &mut stdin,
        json!({
            "jsonrpc":"2.0",
            "id":"prompt",
            "method":"session/prompt",
            "params":{"sessionId":"window-main","contentBlocks":[{"type":"text","text":"Сократи названия чатов до трёх слов"}]}
        }),
    )
    .await;
    let prompted = receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "prompt").await;
    assert!(prompted["result"]["messageId"].is_string(), "{prompted}");
    receive_until(&mut stdout, &mut seen, |frame| {
        frame["method"] == "session.status" && frame["params"]["status"] == "idle"
    })
    .await;
    let is_title = |frame: &Value| {
        frame["method"] == "session.event" && frame["params"]["event"]["type"] == "session/title"
    };
    let title = match seen.iter().find(|frame| is_title(frame)) {
        Some(frame) => frame.clone(),
        None => receive_until(&mut stdout, &mut seen, is_title).await,
    };
    assert_eq!(
        title["params"]["event"]["data"]["title"],
        "Сократи названия чатов"
    );

    // The session log reaches the window as ordered events; the model's text is one message.
    let events: Vec<&Value> = seen
        .iter()
        .filter(|frame| frame["method"] == "session.event")
        .map(|frame| &frame["params"]["event"])
        .collect();
    let types: Vec<&str> = events.iter().filter_map(|e| e["type"].as_str()).collect();
    let position = |name: &str| types.iter().position(|t| *t == name);
    let (start, context, answer, end) = (
        position("turn/start"),
        position("request/context"),
        position("assistant/message"),
        position("turn/end"),
    );
    assert!(
        matches!((start, context, answer, end), (Some(a), Some(b), Some(c), Some(d)) if a < b && b < c && c < d),
        "{types:?}"
    );
    let seqs: Vec<u64> = events.iter().filter_map(|e| e["seq"].as_u64()).collect();
    assert!(seqs.windows(2).all(|pair| pair[0] < pair[1]), "{seqs:?}");
    let message = events
        .iter()
        .find(|e| e["type"] == "assistant/message")
        .unwrap();
    assert_eq!(
        message["data"]["message"]["content"][0]["text"], "window mock answer",
        "{message}"
    );
    let requests = mock.requests().await;
    let completion = requests
        .iter()
        .find(|r| r.method == "POST" && r.path.ends_with("/chat/completions"))
        .unwrap_or_else(|| panic!("{requests:#?}"));
    let body = completion.body.as_ref().expect("request body");
    assert_eq!(body["model"], "window-mock", "{body}");
    assert_eq!(body["stream"], true, "{body}");

    for (id, executor) in [("wrong-harness", "codex"), ("generate-title", "builtin")] {
        send(
            &mut stdin,
            json!({
                "jsonrpc":"2.0",
                "id":id,
                "method":"hoh/executor/title/generate",
                "params":{"executor":executor,"sessionId":"window-main","prompt":"Return a short title as JSON"}
            }),
        )
        .await;
        let result = receive_until(&mut stdout, &mut seen, |frame| frame["id"] == id).await;
        if executor == "builtin" {
            assert_eq!(result["result"]["title"], "Названия чатов", "{result}");
        } else {
            assert!(result.get("error").is_some(), "{result}");
        }
    }
    let requests = mock.requests().await;
    let completions = requests
        .iter()
        .filter(|request| request.method == "POST" && request.path.ends_with("/chat/completions"))
        .collect::<Vec<_>>();
    assert_eq!(completions.len(), 2, "{requests:#?}");
    let naming = completions[1].body.as_ref().expect("title request body");
    assert_eq!(naming["model"], "window-mock", "{naming}");
    assert!(naming["tools"].is_null(), "{naming}");

    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":"shutdown","method":"shutdown","params":{}}),
    )
    .await;
    let shutdown = receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "shutdown").await;
    assert!(shutdown.get("result").is_some(), "{shutdown}");
    // The client keeps its end of stdin open (the Tauri core does): `shutdown` alone ends the engine.
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("the engine did not exit after shutdown while stdin was still open")
        .unwrap();
    drop(stdin);
    let stderr = engine_stderr(root.path());
    assert_eq!(status.code(), Some(0), "{stderr}");
    assert!(stderr.is_empty(), "{stderr}");

    let mut reopened = command(root.path()).spawn().unwrap();
    let mut stdin = reopened.stdin.take().unwrap();
    let mut stdout = BufReader::new(reopened.stdout.take().unwrap()).lines();
    let mut seen = Vec::new();
    send(&mut stdin, initialize).await;
    receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "initialize").await;
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":"list","method":"session/list","params":{}}),
    )
    .await;
    let list = receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "list").await;
    assert_eq!(list["result"]["sessions"][0]["id"], "window-main", "{list}");
    assert_eq!(
        list["result"]["sessions"][0]["title"], "Сократи названия чатов",
        "{list}"
    );
    drop(stdin);
    let status = tokio::time::timeout(Duration::from_secs(10), reopened.wait())
        .await
        .expect("the reopened engine did not exit after stdin closed")
        .unwrap();
    assert_eq!(status.code(), Some(0), "{}", engine_stderr(root.path()));
    mock.close().await;
}

#[tokio::test]
async fn window_profile_exits_with_code_zero_when_the_client_closes_stdin() {
    let root = tempfile::tempdir().unwrap();
    let mut child = command(root.path()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut seen = Vec::new();
    send(
        &mut stdin,
        json!({
            "jsonrpc":"2.0",
            "id":"initialize",
            "method":"initialize",
            "params":{"cwd":window_workspace(root.path()),"provider":"openai-compatible","model":"window-mock"}
        }),
    )
    .await;
    receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "initialize").await;
    drop(stdin);
    let status = tokio::time::timeout(Duration::from_secs(30), child.wait())
        .await
        .expect("engine did not leave after its client disconnected")
        .unwrap();
    assert_eq!(status.code(), Some(0));
}

const APPROVAL_KEY: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn host_initialize(root: &Path) -> Value {
    json!({
        "jsonrpc":"2.0",
        "id":"initialize",
        "method":"initialize",
        "params":{
            "cwd":root,
            "provider":"openai-compatible",
            "model":"window-mock",
            "hoh":{"protocolVersion":1,"approvalKey":APPROVAL_KEY},
            "capabilities":{"approvals":"host"}
        }
    })
}

/// Calls a privileged method the way the Tauri core does: the engine asks, the core answers with a
/// mac over its own copy of the request, then the call completes.
async fn call_privileged(
    stdin: &mut tokio::process::ChildStdin,
    stdout: &mut Lines<BufReader<tokio::process::ChildStdout>>,
    seen: &mut Vec<Value>,
    id: &str,
    method: &str,
    params: Value,
    outcome: &str,
) -> Value {
    send(
        stdin,
        json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
    )
    .await;
    let asked = receive_until(stdout, seen, |frame| {
        frame["method"] == "approval/request" || frame["id"] == id
    })
    .await;
    assert_eq!(asked["method"], "approval/request", "{asked}");
    let request: hoh_protocol::HostApprovalRequest =
        serde_json::from_value(asked["params"].clone()).unwrap();
    let key = hoh_protocol::approval::key_from_hex(APPROVAL_KEY).unwrap();
    send(
        stdin,
        json!({
            "jsonrpc":"2.0",
            "id":asked["id"],
            "result":{
                "approvalId":request.approval_id,
                "outcome":outcome,
                "mac":hoh_protocol::approval::mac(&key, &request, outcome)
            }
        }),
    )
    .await;
    receive_until(stdout, seen, |frame| frame["id"] == id).await
}

#[tokio::test]
async fn window_profile_asks_the_host_before_privileged_calls_and_keeps_the_model_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let mock = hoh_llm_openai_mock::MockServer::start(hoh_llm_openai_mock::MockOptions::new(
        hoh_llm_openai_mock::Scenario {
            model: "window-mock".to_owned(),
            responses: vec![],
        },
    ))
    .await;
    // An empty scenario may be refused; the engine never calls the model in this test.
    let base_url = mock.as_ref().map_or_else(
        |_| "http://127.0.0.1:9/v1".to_owned(),
        |m| m.base_url.clone(),
    );
    write_profile(root.path(), &base_url);
    let mut seen = Vec::new();

    let mut child = command(root.path()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    send(&mut stdin, host_initialize(&window_workspace(root.path()))).await;
    let initialized =
        receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "initialize").await;
    assert_eq!(
        initialized["result"]["hoh"]["approvals"], "host",
        "{initialized}"
    );

    // A refused request ends the call with an error and stores nothing.
    let refused = call_privileged(
        &mut stdin,
        &mut stdout,
        &mut seen,
        "refused",
        "hoh/settings/set",
        json!({"model":{"baseUrl":"http://127.0.0.1:9/v1","model":"refused-model"}}),
        "rejected",
    )
    .await;
    assert!(refused.get("error").is_some(), "{refused}");
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":"after-refusal","method":"hoh/settings/get","params":{}}),
    )
    .await;
    let unchanged = receive_until(&mut stdout, &mut seen, |f| f["id"] == "after-refusal").await;
    assert_eq!(
        unchanged["result"]["model"]["model"], "window-mock",
        "a refused change took effect: {unchanged}"
    );
    let saved = root.path().join("home/profiles/window/model-settings.json");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!saved.exists(), "a refused change was saved");
    let stored = call_privileged(
        &mut stdin,
        &mut stdout,
        &mut seen,
        "key",
        "hoh/model/key/set",
        json!({"apiKey":"sk-window-test-key","baseUrl":base_url}),
        "allowed-once",
    )
    .await;
    assert!(stored.get("result").is_some(), "{stored}");
    let changed = call_privileged(
        &mut stdin,
        &mut stdout,
        &mut seen,
        "set",
        "hoh/settings/set",
        json!({"model":{"baseUrl":"http://127.0.0.1:9/v1","model":"restarted-model"}}),
        "allowed-once",
    )
    .await;
    assert!(changed.get("result").is_some(), "{changed}");
    assert!(
        !seen
            .iter()
            .any(|f| f.to_string().contains("sk-window-test-key")),
        "the key crossed the protocol: {seen:#?}"
    );

    // The save is asynchronous: wait for the file before closing the engine.
    tokio::time::timeout(Duration::from_secs(10), async {
        while !saved.exists() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("model settings were not saved");
    let mode =
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&saved).unwrap().permissions());
    assert_eq!(mode & 0o777, 0o600, "the settings file must be private");
    let on_disk: Value = serde_json::from_slice(&std::fs::read(&saved).unwrap()).unwrap();
    assert_eq!(
        on_disk,
        json!({"baseUrl":"http://127.0.0.1:9/v1","model":"restarted-model"})
    );
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":"shutdown","method":"shutdown","params":{}}),
    )
    .await;
    receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "shutdown").await;
    drop(stdin);
    assert_eq!(child.wait().await.unwrap().code(), Some(0));

    let mut child = command(root.path()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    send(&mut stdin, host_initialize(&window_workspace(root.path()))).await;
    receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "initialize").await;
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":"get","method":"hoh/settings/get","params":{}}),
    )
    .await;
    let settings = receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "get").await;
    assert_eq!(
        settings["result"]["model"]["model"], "restarted-model",
        "{settings}"
    );
    drop(stdin);
    assert_eq!(child.wait().await.unwrap().code(), Some(0));
}

#[tokio::test]
async fn window_profile_refuses_privileged_calls_without_host_approvals() {
    let root = tempfile::tempdir().unwrap();
    write_profile(root.path(), "http://127.0.0.1:9/v1");
    let mut child = command(root.path()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut seen = Vec::new();
    send(
        &mut stdin,
        json!({
            "jsonrpc":"2.0","id":"initialize","method":"initialize",
            "params":{"cwd":window_workspace(root.path()),"provider":"openai-compatible","model":"window-mock"}
        }),
    )
    .await;
    receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "initialize").await;
    send(
        &mut stdin,
        json!({
            "jsonrpc":"2.0","id":"key","method":"hoh/model/key/set",
            "params":{"apiKey":"sk-window-test-key","baseUrl":"http://127.0.0.1:9/v1"}
        }),
    )
    .await;
    let refused = receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "key").await;
    assert!(refused.get("error").is_some(), "{refused}");
    assert!(
        !seen
            .iter()
            .any(|f| f.to_string().contains("approval/request")),
        "{seen:#?}"
    );
    drop(stdin);
    assert_eq!(child.wait().await.unwrap().code(), Some(0));
}

fn scenario(
    responses: Vec<hoh_llm_openai_mock::ScenarioResponse>,
) -> hoh_llm_openai_mock::Scenario {
    hoh_llm_openai_mock::Scenario {
        model: "window-mock".to_owned(),
        responses,
    }
}

fn say(text: &str) -> hoh_llm_openai_mock::ScenarioResponse {
    hoh_llm_openai_mock::ScenarioResponse {
        text: vec![text.to_owned()],
        tool_calls: vec![],
        usage: hoh_llm_openai_mock::ScenarioUsage {
            prompt_tokens: 3,
            completion_tokens: 2,
        },
    }
}

fn call_bash(arguments: Value) -> hoh_llm_openai_mock::ScenarioResponse {
    call_tool("bash", arguments)
}

fn call_tool(name: &str, arguments: Value) -> hoh_llm_openai_mock::ScenarioResponse {
    call_tool_with_id(&format!("call-{name}"), name, arguments)
}

fn call_tool_with_id(
    id: &str,
    name: &str,
    arguments: Value,
) -> hoh_llm_openai_mock::ScenarioResponse {
    hoh_llm_openai_mock::ScenarioResponse {
        text: vec![],
        tool_calls: vec![hoh_llm_openai_mock::ScenarioToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments,
        }],
        usage: hoh_llm_openai_mock::ScenarioUsage {
            prompt_tokens: 3,
            completion_tokens: 2,
        },
    }
}

fn tool_result_text(body: &Value, call_id: &str) -> String {
    let message = body["messages"]
        .as_array()
        .and_then(|messages| {
            messages
                .iter()
                .rev()
                .find(|message| message["role"] == "tool" && message["tool_call_id"] == call_id)
        })
        .unwrap_or_else(|| panic!("missing tool result {call_id}: {body}"));
    message["content"]
        .as_str()
        .map_or_else(|| message["content"].to_string(), str::to_owned)
}

/// A scratch folder outside `/tmp`: `workspace-write` always leaves `/tmp` writable, so a folder
/// there would make "outside the workspace" meaningless.
/// A fresh directory the sandbox can see. The cargo target directory is the default home, but it may
/// sit on a mount that bubblewrap's private `/dev` hides (`/dev/shm`); then the process temporary
/// directory serves, and a root nobody can use fails the test at once instead of making sandboxed
/// commands silently miss their files.
fn sandboxed_root() -> tempfile::TempDir {
    let hidden = |path: &Path| {
        let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        real.starts_with("/dev") || real.starts_with("/tmp") || real.starts_with("/proc")
    };
    let target = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let temporary = std::env::temp_dir();
    let base = if !hidden(target) {
        target.to_path_buf()
    } else {
        temporary
    };
    assert!(
        !hidden(&base),
        "no test directory outside /dev, /tmp and /proc: set TMPDIR to a regular directory"
    );
    tempfile::tempdir_in(base).unwrap()
}

struct Engine {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: Lines<BufReader<tokio::process::ChildStdout>>,
    seen: Vec<Value>,
    mock: hoh_llm_openai_mock::MockServer,
    work: std::path::PathBuf,
    outside: std::path::PathBuf,
}

/// An engine in host-approval mode over a mock that plays `responses`; `work` is the workspace
/// the session is opened in, `outside` a sibling directory the sandbox must keep closed.
async fn start_engine(
    root: &Path,
    responses: Vec<hoh_llm_openai_mock::ScenarioResponse>,
) -> Engine {
    start_engine_with_home(root, &root.join("home"), responses).await
}

async fn start_engine_with_home(
    root: &Path,
    dsh_home: &Path,
    responses: Vec<hoh_llm_openai_mock::ScenarioResponse>,
) -> Engine {
    let mock = hoh_llm_openai_mock::MockServer::start(hoh_llm_openai_mock::MockOptions::new(
        scenario(responses),
    ))
    .await
    .unwrap();
    let work = root.join("work");
    write_profile_at(dsh_home, &work, &mock.base_url);
    let outside = root.join("outside");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let mut child = command_with_home(root, dsh_home).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut seen = Vec::new();
    let mut init = host_initialize(&work);
    init["params"]["cwd"] = json!(work);
    send(&mut stdin, init).await;
    let ready = receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "initialize").await;
    assert!(ready.get("result").is_some(), "{ready}");
    Engine {
        child,
        stdin,
        stdout,
        seen,
        mock,
        work,
        outside,
    }
}

impl Engine {
    async fn prompt(&mut self, text: &str) {
        send(
            &mut self.stdin,
            json!({"jsonrpc":"2.0","id":"prompt","method":"session/prompt",
                "params":{"sessionId":"window-main","contentBlocks":[{"type":"text","text":text}]}}),
        )
        .await;
        receive_until(&mut self.stdout, &mut self.seen, |f| f["id"] == "prompt").await;
    }

    /// The next frame is either the engine asking the host or the turn ending.
    async fn next_approval(&mut self) -> Option<hoh_protocol::HostApprovalRequest> {
        let frame = receive_until(&mut self.stdout, &mut self.seen, |f| {
            f["method"] == "approval/request"
                || (f["method"] == "session.status" && f["params"]["status"] == "idle")
        })
        .await;
        (frame["method"] == "approval/request").then(|| {
            self.seen.pop();
            self.seen.push(frame.clone());
            serde_json::from_value(frame["params"].clone()).unwrap()
        })
    }

    async fn answer(&mut self, outcome: &str) {
        let asked = self
            .seen
            .iter()
            .rev()
            .find(|f| f["method"] == "approval/request")
            .cloned()
            .expect("no approval/request to answer");
        let request: hoh_protocol::HostApprovalRequest =
            serde_json::from_value(asked["params"].clone()).unwrap();
        let key = hoh_protocol::approval::key_from_hex(APPROVAL_KEY).unwrap();
        send(
            &mut self.stdin,
            json!({"jsonrpc":"2.0","id":asked["id"],"result":{
                "approvalId":request.approval_id,
                "outcome":outcome,
                "mac":hoh_protocol::approval::mac(&key, &request, outcome)
            }}),
        )
        .await;
    }

    async fn finish(mut self) {
        drop(self.stdin);
        let status = tokio::time::timeout(Duration::from_secs(30), self.child.wait())
            .await
            .expect("engine exit timeout")
            .unwrap();
        assert_eq!(status.code(), Some(0));
        self.mock.close().await;
    }
}

fn escalated(command: &str) -> Value {
    json!({
        "command": command,
        "description": "remove a file outside the workspace",
        "sandbox_permissions": "danger-full-access",
        "justification": "The user asked to delete this file."
    })
}

#[tokio::test]
async fn a_dangerous_bash_call_waits_for_the_host_and_a_rejection_keeps_the_file() {
    let root = sandboxed_root();
    let victim_dir = root.path().join("outside");
    std::fs::create_dir_all(&victim_dir).unwrap();
    let victim = victim_dir.join("precious.txt");
    std::fs::write(&victim, "keep me").unwrap();
    let mut engine = start_engine(
        root.path(),
        vec![
            call_bash(escalated(&format!("rm {}", victim.display()))),
            say("not done"),
        ],
    )
    .await;
    engine.prompt("delete it").await;
    let asked = engine
        .next_approval()
        .await
        .expect("the dangerous command ran without asking the host");
    assert_eq!(asked.tool_name, "bash");
    assert!(
        victim.exists(),
        "the file was removed before the user answered"
    );
    engine.answer("rejected").await;
    assert!(engine.next_approval().await.is_none());
    assert!(victim.exists(), "a rejected command must not run");
    engine.finish().await;
}

#[tokio::test]
async fn a_dangerous_bash_call_runs_after_the_host_allows_it() {
    let root = sandboxed_root();
    let victim_dir = root.path().join("outside");
    std::fs::create_dir_all(&victim_dir).unwrap();
    let victim = victim_dir.join("precious.txt");
    std::fs::write(&victim, "delete me").unwrap();
    let mut engine = start_engine(
        root.path(),
        vec![
            call_bash(escalated(&format!("rm {}", victim.display()))),
            say("deleted"),
        ],
    )
    .await;
    engine.prompt("delete it").await;
    engine
        .next_approval()
        .await
        .expect("the dangerous command ran without asking the host");
    assert!(victim.exists());
    engine.answer("allowed-once").await;
    assert!(engine.next_approval().await.is_none());
    assert!(!victim.exists(), "the approved command did not run");
    engine.finish().await;
}

#[tokio::test]
async fn bash_stays_in_the_workspace_without_asking_and_cannot_write_outside_it() {
    let root = sandboxed_root();
    let mut engine = start_engine(
        root.path(),
        vec![
            call_bash(json!({
                "command": "echo inside > inside.txt; echo outside > ../outside/leak.txt",
                "description": "write one file in and one file out of the workspace"
            })),
            say("done"),
        ],
    )
    .await;
    engine.prompt("write files").await;
    assert!(
        engine.next_approval().await.is_none(),
        "an unescalated command must not ask: the sandbox keeps it inside"
    );
    assert!(engine.work.join("inside.txt").exists());
    assert!(
        !engine.outside.join("leak.txt").exists(),
        "the sandbox let a write out of the workspace"
    );
    engine.finish().await;
}

#[tokio::test]
async fn the_key_never_reaches_stderr_or_the_session_files() {
    let root = tempfile::tempdir().unwrap();
    let mut engine = start_engine(root.path(), vec![say("ok")]).await;
    let key = "sk-window-secret-key";
    let base_url = engine.mock.base_url.clone();
    send(
        &mut engine.stdin,
        json!({"jsonrpc":"2.0","id":"key","method":"hoh/model/key/set",
            "params":{"apiKey":key,"baseUrl":base_url}}),
    )
    .await;
    let asked = receive_until(&mut engine.stdout, &mut engine.seen, |f| {
        f["method"] == "approval/request"
    })
    .await;
    assert!(!asked.to_string().contains(key), "{asked}");
    engine.seen.push(asked);
    engine.answer("allowed-once").await;
    let stored = receive_until(&mut engine.stdout, &mut engine.seen, |f| f["id"] == "key").await;
    assert!(stored.get("result").is_some(), "{stored}");
    engine.prompt("hello").await;
    assert!(engine.next_approval().await.is_none());
    let root_path = root.path().to_path_buf();
    engine.finish().await;
    assert!(!engine_stderr(&root_path).contains(key));
    // The key lives in the credential store only.
    let mut holders = Vec::new();
    let mut pending = vec![root_path.clone()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if std::fs::read(&path)
                .is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains(key))
            {
                holders.push(path);
            }
        }
    }
    assert!(
        holders
            .iter()
            .all(|path| path.ends_with(".credentials.yaml")),
        "{holders:?}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn model_tools_cannot_read_list_or_copy_engine_credentials_directly_or_via_symlink() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = sandboxed_root();
    let home = root.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
    let credentials = home.join(".credentials.yaml");
    let key = "sk-window-model-must-never-see-this";
    std::fs::write(
        &credentials,
        format!("version: 1\nrefs:\n  model_api_key: {key}\n"),
    )
    .unwrap();
    std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600)).unwrap();
    let copied = root.path().join("work/copied-key");
    let archive = root.path().join("work/engine.tar");
    let ordinary = root.path().join("work/ordinary.txt");
    let alias = root.path().join("work/engine-link");
    let bash = |id: &str, command: String| {
        call_tool_with_id(
            id,
            "bash",
            json!({
                "command": command,
                "description": "verify the model-facing filesystem boundary"
            }),
        )
    };
    let mut engine = start_engine(
        root.path(),
        vec![
            call_tool_with_id("read-direct", "read", json!({"file_path": credentials})),
            call_tool_with_id(
                "read-symlink",
                "read",
                json!({"file_path": alias.join(".credentials.yaml")}),
            ),
            bash("bash-cat", format!("cat {}", credentials.display())),
            bash(
                "bash-grep",
                format!("grep OPENAI_API_KEY {}", credentials.display()),
            ),
            bash("bash-ls", format!("ls {}", home.display())),
            bash(
                "bash-cp",
                format!("cp {} {}", credentials.display(), copied.display()),
            ),
            bash(
                "bash-find",
                format!("find {} -maxdepth 3 -type f", home.display()),
            ),
            bash(
                "bash-tar",
                format!("tar -cf {} -C {} .", archive.display(), home.display()),
            ),
            call_tool_with_id(
                "read-session",
                "read",
                json!({"file_path": home.join("sessions/conversation.jsonl")}),
            ),
            call_tool_with_id(
                "read-model-settings",
                "read",
                json!({"file_path": home.join("profiles/window/model-settings.json")}),
            ),
            bash(
                "workspace-write",
                format!("printf workspace-ok > {}", ordinary.display()),
            ),
            say("done"),
        ],
    )
    .await;
    std::fs::create_dir_all(home.join("sessions")).unwrap();
    std::fs::write(
        home.join("sessions/conversation.jsonl"),
        "private conversation",
    )
    .unwrap();
    std::fs::write(
        home.join("profiles/window/model-settings.json"),
        r#"{"baseUrl":"https://must-not-leak.invalid","model":"private"}"#,
    )
    .unwrap();
    symlink(&home, &alias).unwrap();
    engine.prompt("try every file access").await;
    assert!(
        engine.next_approval().await.is_none(),
        "ordinary denied accesses must not become an escalation prompt"
    );
    assert!(!copied.exists());
    assert_eq!(std::fs::read_to_string(&ordinary).unwrap(), "workspace-ok");
    let requests = engine.mock.requests().await;
    assert_eq!(
        requests.len(),
        12,
        "all eleven tool calls and the final response ran"
    );
    for request in &requests {
        if let Some(body) = &request.body {
            assert!(
                !body.to_string().contains(key),
                "model saw credential: {body}"
            );
        }
    }
    let body = requests
        .last()
        .and_then(|request| request.body.as_ref())
        .expect("final model request body");
    for id in [
        "read-direct",
        "read-symlink",
        "read-session",
        "read-model-settings",
    ] {
        let result = tool_result_text(body, id);
        assert!(
            result.contains("protected engine storage") || result.contains("FS_SANDBOX_DENIED"),
            "{id} was not an explicit protected-storage refusal: {result}"
        );
    }
    for id in [
        "bash-cat",
        "bash-grep",
        "bash-ls",
        "bash-cp",
        "bash-find",
        "bash-tar",
    ] {
        let result = tool_result_text(body, id).to_lowercase();
        assert!(
            [
                "permission denied",
                "operation not permitted",
                "access denied"
            ]
            .iter()
            .any(|needle| result.contains(needle)),
            "{id} was not an explicit sandbox refusal: {result}"
        );
    }
    let workspace_result = tool_result_text(body, "workspace-write").to_lowercase();
    assert!(
        !workspace_result.contains("denied") && !workspace_result.contains("error"),
        "ordinary workspace write failed: {workspace_result}"
    );
    engine.finish().await;
}

#[cfg(unix)]
#[tokio::test]
async fn moving_the_engine_parent_keeps_fs_blocked_and_makes_the_next_bash_fail_closed() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let root = sandboxed_root();
    let data_parent = root.path().join("application-data");
    let home = data_parent.join("engine");
    let moved_parent = root.path().join("application-data-moved");
    let moved_home = moved_parent.join("engine");
    let credentials = home.join(".credentials.yaml");
    let moved_credentials = moved_home.join(".credentials.yaml");
    let marker = root.path().join("work/second-bash-ran");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
    let key = "sk-moved-engine-must-remain-secret";
    std::fs::write(
        &credentials,
        format!("version: 1\nrefs:\n  model_api_key: {key}\n"),
    )
    .unwrap();
    std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600)).unwrap();
    let original_home = std::fs::metadata(&home).unwrap();
    let original_identity = (original_home.dev(), original_home.ino());

    let move_command = format!(
        "mv '{}' '{}'; mkdir -p '{}'",
        data_parent.display(),
        moved_parent.display(),
        home.display()
    );
    let second_bash = format!(
        "printf ran > '{}'; ls '{}'; grep never-leak '{}'",
        marker.display(),
        moved_home.display(),
        moved_credentials.display()
    );
    let mut engine = start_engine_with_home(
        root.path(),
        &home,
        vec![
            call_tool_with_id("move-parent", "bash", escalated(&move_command)),
            call_tool_with_id(
                "read-moved-secret",
                "read",
                json!({"file_path":moved_credentials}),
            ),
            call_tool_with_id("read-moved-root", "read", json!({"file_path":moved_home})),
            call_tool_with_id(
                "bash-after-move",
                "bash",
                json!({
                    "command": second_bash,
                    "description": "list and search the moved engine directory"
                }),
            ),
            say("done"),
        ],
    )
    .await;
    engine.prompt("move the engine data and inspect it").await;
    engine
        .next_approval()
        .await
        .expect("danger-full-access parent rename ran without host approval");
    engine.answer("allowed-once").await;
    assert!(
        engine.next_approval().await.is_none(),
        "protected follow-up accesses must fail without another approval"
    );

    assert!(
        home.exists(),
        "the approved command did not recreate the configured mountpoint"
    );
    assert!(
        !home.join(".credentials.yaml").exists(),
        "the replacement unexpectedly contains the protected secret"
    );
    let replacement_home = std::fs::metadata(&home).unwrap();
    assert_ne!(
        (replacement_home.dev(), replacement_home.ino()),
        original_identity,
        "the configured mountpoint was not replaced with a different object"
    );
    assert!(moved_credentials.exists());
    assert!(
        !marker.exists(),
        "the post-rename bash command unexpectedly ran"
    );
    let requests = engine.mock.requests().await;
    assert_eq!(
        requests.len(),
        5,
        "four tool calls and the final response must all run"
    );
    for request in &requests {
        if let Some(body) = &request.body {
            assert!(
                !body.to_string().contains(key),
                "model saw credential: {body}"
            );
        }
    }
    let body = requests
        .last()
        .and_then(|request| request.body.as_ref())
        .expect("final model request body");
    for id in ["read-moved-secret", "read-moved-root"] {
        let result = tool_result_text(body, id);
        assert!(
            result.contains("protected engine storage") || result.contains("FS_SANDBOX_DENIED"),
            "{id} was not an explicit moved-storage refusal: {result}"
        );
    }
    let bash_result = tool_result_text(body, "bash-after-move");
    assert!(
        bash_result.contains("protected path identity changed")
            && bash_result.contains("refusing to run"),
        "post-rename bash did not fail closed before execution: {bash_result}"
    );
    engine.finish().await;
}

#[tokio::test]
async fn session_start_and_working_directory_change_reject_engine_home_overlap() {
    let root = sandboxed_root();
    write_profile(root.path(), "http://127.0.0.1:9/v1");
    let workspace = window_workspace(root.path());
    let home = root.path().join("home");

    let mut child = command(root.path()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut seen = Vec::new();
    send(&mut stdin, host_initialize(&workspace)).await;
    let initialized = receive_until(&mut stdout, &mut seen, |f| f["id"] == "initialize").await;
    assert!(initialized.get("result").is_some(), "{initialized}");
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":"get","method":"session/working-directory/get",
            "params":{"sessionId":"safe-session"}}),
    )
    .await;
    let current = receive_until(&mut stdout, &mut seen, |f| f["id"] == "get").await;
    assert_eq!(
        current["result"]["cwd"],
        workspace.to_string_lossy().as_ref()
    );
    let refused = call_privileged(
        &mut stdin,
        &mut stdout,
        &mut seen,
        "overlap-change",
        "session/working-directory/set",
        json!({"sessionId":"safe-session","path":home}),
        "allowed-once",
    )
    .await;
    assert!(
        refused["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("папкой данных HoH")),
        "{refused}"
    );
    drop(stdin);
    assert_eq!(child.wait().await.unwrap().code(), Some(0));

    let mut child = command(root.path()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut seen = Vec::new();
    send(&mut stdin, host_initialize(&home)).await;
    receive_until(&mut stdout, &mut seen, |f| f["id"] == "initialize").await;
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":"overlap-start","method":"session/working-directory/get",
            "params":{"sessionId":"overlap-session"}}),
    )
    .await;
    let refused = receive_until(&mut stdout, &mut seen, |f| f["id"] == "overlap-start").await;
    assert!(
        refused["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("папкой данных HoH")),
        "{refused}"
    );
    drop(stdin);
    assert_eq!(child.wait().await.unwrap().code(), Some(0));
}

#[tokio::test]
async fn a_model_address_with_credentials_is_refused_before_the_user_is_asked() {
    let root = tempfile::tempdir().unwrap();
    let mut engine = start_engine(root.path(), vec![say("ok")]).await;
    for (index, address) in [
        "https://user:KEY@api.example.test/v1",
        "https://api.example.test/v1?api_key=KEY",
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("set-{index}");
        send(
            &mut engine.stdin,
            json!({"jsonrpc":"2.0","id":id,"method":"hoh/settings/set",
                "params":{"model":{"baseUrl":address,"model":"m"}}}),
        )
        .await;
        let answer = receive_until(&mut engine.stdout, &mut engine.seen, |f| {
            f["id"] == id.as_str() || f["method"] == "approval/request"
        })
        .await;
        assert_eq!(answer["id"], id.as_str(), "the user was asked: {answer}");
        assert!(answer.get("error").is_some(), "{answer}");
    }
    // The same gate covers the other two methods that take an address.
    for (index, address) in [
        "https://user:KEY@api.example.test/v1",
        "https://api.example.test/v1?api_key=KEY",
    ]
    .into_iter()
    .enumerate()
    {
        for (method, params) in [
            (
                "hoh/model/key/set",
                json!({"apiKey":"sk-not-saved","baseUrl":address}),
            ),
            ("hoh/model/probe", json!({"baseUrl":address,"model":"m"})),
        ] {
            let id = format!("{method}-{index}");
            send(
                &mut engine.stdin,
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            )
            .await;
            let answer = receive_until(&mut engine.stdout, &mut engine.seen, |f| {
                f["id"] == id.as_str() || f["method"] == "approval/request"
            })
            .await;
            assert_eq!(
                answer["id"],
                id.as_str(),
                "{method}: the user was asked: {answer}"
            );
            assert!(answer.get("error").is_some(), "{method}: {answer}");
        }
    }
    send(
        &mut engine.stdin,
        json!({"jsonrpc":"2.0","id":"get","method":"hoh/settings/get","params":{}}),
    )
    .await;
    let settings = receive_until(&mut engine.stdout, &mut engine.seen, |f| f["id"] == "get").await;
    assert!(!settings.to_string().contains("KEY"), "{settings}");
    assert!(!settings.to_string().contains("sk-not-saved"), "{settings}");
    engine.finish().await;
}

#[tokio::test]
async fn requests_piped_in_before_end_of_input_are_all_answered() {
    let root = tempfile::tempdir().unwrap();
    let mut child = command(root.path()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut seen = Vec::new();
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":"initialize","method":"initialize",
            "params":{"cwd":window_workspace(root.path()),"provider":"openai-compatible","model":"local"}}),
    )
    .await;
    receive_until(&mut stdout, &mut seen, |f| f["id"] == "initialize").await;
    // Far more than the in-process pipe holds, then the end of input right behind it.
    const COUNT: usize = 3000;
    let batch = (0..COUNT)
        .map(|n| {
            json!({"jsonrpc":"2.0","id":format!("list-{n}"),"method":"session/list","params":{}})
                .to_string()
                + "\n"
        })
        .collect::<String>();
    // A client reads while it writes (the desktop core does): the engine's output queue is bounded
    // (1024 frames), so a peer that reads nothing until the whole batch is out is a slow consumer
    // the engine is allowed to cut off, and under load it did.
    let reader = tokio::spawn(async move {
        let mut output = String::new();
        while let Some(line) = stdout.next_line().await.unwrap() {
            output.push_str(&line);
            output.push('\n');
        }
        output
    });
    stdin.write_all(batch.as_bytes()).await.unwrap();
    drop(stdin);
    let output = tokio::time::timeout(Duration::from_secs(60), reader)
        .await
        .expect("the engine kept its output open")
        .unwrap();
    let status = tokio::time::timeout(Duration::from_secs(30), child.wait())
        .await
        .expect("engine exit timeout")
        .unwrap();
    let answered = (0..COUNT)
        .filter(|n| output.contains(&format!("\"id\":\"list-{n}\"")))
        .count();
    assert_eq!(answered, COUNT, "the engine left with requests unread");
    assert_eq!(status.code(), Some(0));
}

#[tokio::test]
async fn first_run_rules_survive_an_engine_restart() {
    let root = tempfile::tempdir().unwrap();
    write_profile(root.path(), "http://127.0.0.1:9/v1");
    let mut seen = Vec::new();

    let mut child = command(root.path()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    send(&mut stdin, host_initialize(&window_workspace(root.path()))).await;
    receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "initialize").await;
    let stored = call_privileged(
        &mut stdin,
        &mut stdout,
        &mut seen,
        "rules",
        "hoh/settings/rules/set",
        json!({"internet":"public","folder":"/home/user/Work"}),
        "allowed-once",
    )
    .await;
    assert!(stored.get("result").is_some(), "{stored}");
    let saved = root.path().join("home/profiles/window/window-rules.json");
    tokio::time::timeout(Duration::from_secs(10), async {
        while !saved.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the rules were not saved");
    let mode =
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&saved).unwrap().permissions());
    assert_eq!(mode & 0o777, 0o600, "the rules file must be private");
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":"shutdown","method":"shutdown","params":{}}),
    )
    .await;
    receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "shutdown").await;
    assert_eq!(child.wait().await.unwrap().code(), Some(0));
    drop(stdin);

    let mut child = command(root.path()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    send(&mut stdin, host_initialize(&window_workspace(root.path()))).await;
    receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "initialize").await;
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":"get","method":"hoh/settings/get","params":{}}),
    )
    .await;
    let settings = receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "get").await;
    assert_eq!(
        settings["result"]["security"]["internet"], "public",
        "{settings}"
    );
    assert_eq!(
        settings["result"]["security"]["folders"],
        json!(["/home/user/Work"]),
        "{settings}"
    );
    drop(stdin);
    assert_eq!(child.wait().await.unwrap().code(), Some(0));
}

#[tokio::test]
async fn a_failed_save_of_the_model_is_retried_until_it_succeeds() {
    let root = tempfile::tempdir().unwrap();
    write_profile(root.path(), "http://127.0.0.1:9/v1");
    let profile_dir = root.path().join("home/profiles/window");
    let mut seen = Vec::new();
    let mut child = command(root.path()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    send(&mut stdin, host_initialize(&window_workspace(root.path()))).await;
    receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "initialize").await;
    let permissions = |mode| std::os::unix::fs::PermissionsExt::from_mode(mode);
    // The profile folder stops accepting files, then the user changes the model.
    std::fs::set_permissions(&profile_dir, permissions(0o500)).unwrap();
    let changed = call_privileged(
        &mut stdin,
        &mut stdout,
        &mut seen,
        "set",
        "hoh/settings/set",
        json!({"model":{"baseUrl":"http://127.0.0.1:9/v1","model":"retried-model"}}),
        "allowed-once",
    )
    .await;
    assert!(changed.get("result").is_some(), "{changed}");
    let saved = profile_dir.join("model-settings.json");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!saved.exists(), "the save cannot have succeeded yet");
    std::fs::set_permissions(&profile_dir, permissions(0o700)).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !saved.exists() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the failed save was never retried");
    let on_disk: Value = serde_json::from_slice(&std::fs::read(&saved).unwrap()).unwrap();
    assert_eq!(
        on_disk,
        json!({"baseUrl":"http://127.0.0.1:9/v1","model":"retried-model"})
    );
    drop(stdin);
    assert_eq!(child.wait().await.unwrap().code(), Some(0));
}

fn write_outside(target: &Path, escalate: bool) -> Value {
    let mut args = json!({"file_path": target, "content": "written outside"});
    if escalate {
        args["sandbox_permissions"] = json!("danger-full-access");
        args["justification"] = json!("The user asked for a file outside the workspace.");
    }
    args
}

#[tokio::test]
async fn a_write_outside_the_folder_waits_for_the_host_and_a_rejection_writes_nothing() {
    let root = sandboxed_root();
    let target = root.path().join("outside/new.txt");
    let mut engine = start_engine(
        root.path(),
        vec![call_tool("write", write_outside(&target, true)), say("no")],
    )
    .await;
    engine.prompt("write it").await;
    let asked = engine
        .next_approval()
        .await
        .expect("a write outside the folder did not ask the host");
    assert_eq!(asked.tool_name, "write");
    assert!(!target.exists());
    engine.answer("rejected").await;
    assert!(engine.next_approval().await.is_none());
    assert!(!target.exists(), "a rejected write must not land");
    engine.finish().await;
}

#[tokio::test]
async fn a_write_outside_the_folder_lands_after_the_host_allows_it() {
    let root = sandboxed_root();
    let target = root.path().join("outside/new.txt");
    let mut engine = start_engine(
        root.path(),
        vec![
            call_tool("write", write_outside(&target, true)),
            say("done"),
        ],
    )
    .await;
    engine.prompt("write it").await;
    engine
        .next_approval()
        .await
        .expect("the host was not asked");
    engine.answer("allowed-once").await;
    assert!(engine.next_approval().await.is_none());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "written outside");
    engine.finish().await;
}

#[tokio::test]
async fn an_unescalated_write_stays_inside_the_folder_without_asking() {
    let root = sandboxed_root();
    let outside = root.path().join("outside/denied.txt");
    let inside = root.path().join("work/inside.txt");
    let mut engine = start_engine(
        root.path(),
        vec![
            call_tool("write", write_outside(&outside, false)),
            call_tool("write", write_outside(&inside, false)),
            say("done"),
        ],
    )
    .await;
    engine.prompt("write twice").await;
    assert!(
        engine.next_approval().await.is_none(),
        "an unescalated write must not ask"
    );
    assert!(
        !outside.exists(),
        "the sandbox let a write out of the folder"
    );
    assert!(inside.exists(), "a write inside the folder was refused");
    engine.finish().await;
}

fn edit_outside(target: &Path, escalate: bool) -> Value {
    let mut args = json!({"file_path": target, "old_string": "old text", "new_string": "new text"});
    if escalate {
        args["sandbox_permissions"] = json!("danger-full-access");
        args["justification"] = json!("The user asked for an edit outside the workspace.");
    }
    args
}

#[tokio::test]
async fn an_edit_outside_the_folder_waits_for_the_host_and_a_rejection_leaves_the_file() {
    let root = sandboxed_root();
    let target = root.path().join("outside/existing.txt");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "old text").unwrap();
    let mut engine = start_engine(
        root.path(),
        vec![call_tool("edit", edit_outside(&target, true)), say("no")],
    )
    .await;
    engine.prompt("edit it").await;
    let asked = engine
        .next_approval()
        .await
        .expect("an edit outside the folder did not ask the host");
    assert_eq!(asked.tool_name, "edit");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "old text");
    engine.answer("rejected").await;
    assert!(engine.next_approval().await.is_none());
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "old text",
        "a rejected edit must not land"
    );
    engine.finish().await;
}

#[tokio::test]
async fn an_edit_outside_the_folder_lands_after_the_host_allows_it() {
    let root = sandboxed_root();
    let target = root.path().join("outside/existing.txt");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "old text").unwrap();
    let mut engine = start_engine(
        root.path(),
        vec![call_tool("edit", edit_outside(&target, true)), say("done")],
    )
    .await;
    engine.prompt("edit it").await;
    engine
        .next_approval()
        .await
        .expect("the host was not asked");
    engine.answer("allowed-once").await;
    assert!(engine.next_approval().await.is_none());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new text");
    engine.finish().await;
}

#[tokio::test]
async fn an_unescalated_edit_outside_the_folder_does_not_ask_and_changes_nothing() {
    let root = sandboxed_root();
    let target = root.path().join("outside/existing.txt");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "old text").unwrap();
    let mut engine = start_engine(
        root.path(),
        vec![call_tool("edit", edit_outside(&target, false)), say("done")],
    )
    .await;
    engine.prompt("edit it").await;
    assert!(
        engine.next_approval().await.is_none(),
        "an unescalated edit must not ask"
    );
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "old text",
        "the sandbox let an edit out of the folder"
    );
    engine.finish().await;
}

fn patch_outside(target: &Path, escalate: bool) -> Value {
    let mut args = json!({"input": format!(
        "*** Begin Patch\n*** Update File: {}\n@@\n-old text\n+new text\n*** End Patch",
        target.display()
    )});
    if escalate {
        args["sandbox_permissions"] = json!("danger-full-access");
        args["justification"] = json!("The user asked for a patch outside the workspace.");
    }
    args
}

fn existing_outside(root: &Path) -> std::path::PathBuf {
    let target = root.join("outside/patched.txt");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "old text\n").unwrap();
    target
}

#[tokio::test]
async fn an_apply_patch_outside_the_folder_waits_for_the_host_and_a_rejection_leaves_the_file() {
    let root = sandboxed_root();
    let target = existing_outside(root.path());
    let mut engine = start_engine(
        root.path(),
        vec![
            call_tool("apply_patch", patch_outside(&target, true)),
            say("no"),
        ],
    )
    .await;
    engine.prompt("patch it").await;
    let asked = engine
        .next_approval()
        .await
        .expect("a patch outside the folder did not ask the host");
    assert_eq!(asked.tool_name, "apply_patch");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "old text\n");
    engine.answer("rejected").await;
    assert!(engine.next_approval().await.is_none());
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "old text\n",
        "a rejected patch must not land"
    );
    engine.finish().await;
}

#[tokio::test]
async fn an_apply_patch_outside_the_folder_lands_after_the_host_allows_it() {
    let root = sandboxed_root();
    let target = existing_outside(root.path());
    let mut engine = start_engine(
        root.path(),
        vec![
            call_tool("apply_patch", patch_outside(&target, true)),
            say("done"),
        ],
    )
    .await;
    engine.prompt("patch it").await;
    engine
        .next_approval()
        .await
        .expect("the host was not asked");
    engine.answer("allowed-once").await;
    assert!(engine.next_approval().await.is_none());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new text\n");
    engine.finish().await;
}

#[tokio::test]
async fn an_unescalated_apply_patch_outside_the_folder_does_not_ask_and_changes_nothing() {
    let root = sandboxed_root();
    let target = existing_outside(root.path());
    let mut engine = start_engine(
        root.path(),
        vec![
            call_tool("apply_patch", patch_outside(&target, false)),
            say("done"),
        ],
    )
    .await;
    engine.prompt("patch it").await;
    assert!(
        engine.next_approval().await.is_none(),
        "an unescalated patch must not ask"
    );
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "old text\n",
        "the sandbox let a patch out of the folder"
    );
    engine.finish().await;
}

#[tokio::test]
async fn shutdown_with_an_unsaved_change_ends_in_time_and_the_file_is_already_written() {
    let root = tempfile::tempdir().unwrap();
    write_profile(root.path(), "http://127.0.0.1:9/v1");
    let saved = root.path().join("home/profiles/window/model-settings.json");
    let mut seen = Vec::new();
    let mut child = command(root.path()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    send(&mut stdin, host_initialize(&window_workspace(root.path()))).await;
    receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "initialize").await;
    let changed = call_privileged(
        &mut stdin,
        &mut stdout,
        &mut seen,
        "set",
        "hoh/settings/set",
        json!({"model":{"baseUrl":"http://127.0.0.1:9/v1","model":"last-minute-model"}}),
        "allowed-once",
    )
    .await;
    assert!(changed.get("result").is_some(), "{changed}");
    // The Tauri core waits 5 seconds after `shutdown` and then kills the engine; stdin stays open.
    let asked = std::time::Instant::now();
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":"shutdown","method":"shutdown","params":{}}),
    )
    .await;
    receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "shutdown").await;
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("the engine did not exit")
        .unwrap();
    let took = asked.elapsed();
    assert_eq!(status.code(), Some(0), "{}", engine_stderr(root.path()));
    assert!(
        took < Duration::from_secs(5),
        "the engine needed {took:?} after shutdown: the core would have killed it"
    );
    let on_disk: Value =
        serde_json::from_slice(&std::fs::read(&saved).expect("not saved")).unwrap();
    assert_eq!(
        on_disk,
        json!({"baseUrl":"http://127.0.0.1:9/v1","model":"last-minute-model"})
    );
    drop(stdin);
}

#[tokio::test]
async fn a_model_changed_by_settings_set_is_the_name_of_the_next_request() {
    let root = tempfile::tempdir().unwrap();
    let mut engine = start_engine(root.path(), vec![say("first"), say("second")]).await;
    engine.prompt("one").await;
    assert!(engine.next_approval().await.is_none());
    let base_url = engine.mock.base_url.clone();
    send(
        &mut engine.stdin,
        json!({"jsonrpc":"2.0","id":"set","method":"hoh/settings/set",
            "params":{"model":{"baseUrl":base_url,"model":"second-model"}}}),
    )
    .await;
    receive_until(&mut engine.stdout, &mut engine.seen, |f| {
        f["method"] == "approval/request"
    })
    .await;
    engine.answer("allowed-once").await;
    let changed = receive_until(&mut engine.stdout, &mut engine.seen, |f| f["id"] == "set").await;
    assert!(changed.get("result").is_some(), "{changed}");
    engine.prompt("two").await;
    assert!(engine.next_approval().await.is_none());
    let names: Vec<String> = engine
        .mock
        .requests()
        .await
        .iter()
        .filter(|r| r.method == "POST" && r.path.ends_with("/chat/completions"))
        .map(|r| {
            r.body.as_ref().unwrap()["model"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(names, ["window-mock", "second-model"]);
    engine.finish().await;
}

#[tokio::test]
async fn compiled_plugin_is_mounted_and_listed_by_window_engine() {
    let root = tempfile::tempdir().unwrap();
    write_profile(root.path(), "http://127.0.0.1:9");
    let mut child = command(root.path()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut seen = Vec::new();
    send(&mut stdin, json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{"cwd":window_workspace(root.path()),"provider":"openai-compatible","model":"local"}})).await;
    let init = receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "init").await;
    assert!(
        init.get("result").is_some(),
        "{init}: {}",
        engine_stderr(root.path())
    );
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":"plugins","method":"hoh/plugins/list","params":{}}),
    )
    .await;
    let result = receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "plugins").await;
    let plugins = result["result"].as_array().expect("plugin list");
    let plugin = plugins
        .iter()
        .find(|p| p["id"] == "turn-note")
        .expect("example plugin");
    assert_eq!(plugin["status"], "mounted", "{plugin}");
    assert_eq!(plugin["manifest"]["panel"]["title"], "Последний ход");
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","id":"shutdown","method":"shutdown","params":{}}),
    )
    .await;
    let _ = receive_until(&mut stdout, &mut seen, |frame| frame["id"] == "shutdown").await;
    drop(stdin);
    tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .unwrap()
        .unwrap();
}
