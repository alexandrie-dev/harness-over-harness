#![cfg(unix)]

mod support;

use futures_util::StreamExt;
use hoh_error::BoxError;
use hoh_llm::{GenerateOptions, LlmAdapter, LlmStream};
use hoh_session::{SessionEvent, SessionStore, SurfaceIntent, TOOL_OUTCOME_UNKNOWN};
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, ToolExecutionInput, ToolRegistrationOptions, define_tool,
};
use serde_json::json;
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use support::{StubAgent, execute_tool, live_signal, setup};

const MODE_ENV: &str = "HOH_CHECKPOINT_CRASH_MODE";
const ROOT_ENV: &str = "HOH_CHECKPOINT_CRASH_ROOT";

fn unique_root(mode: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    std::env::temp_dir().join(format!(
        "hoh-checkpoint-{}-{mode}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn durable_path(root: &Path) -> PathBuf {
    root.join("durable.jsonl")
}

fn marker_path(root: &Path) -> PathBuf {
    root.join("failpoint")
}

fn write_marker(path: &Path, text: &str) {
    let mut file = File::create(path).unwrap();
    file.write_all(text.as_bytes()).unwrap();
    file.sync_all().unwrap();
}

fn persist_session(path: &Path, session: &hoh_session::Session) -> Result<(), BoxError> {
    let mut file = File::create(path)?;
    for event in session.snapshot_events(None, None).iter() {
        serde_json::to_writer(&mut file, event.as_value())?;
        file.write_all(b"\n")?;
    }
    file.sync_all()?;
    Ok(())
}

fn load_events(root: &Path) -> Vec<Arc<SessionEvent>> {
    BufReader::new(File::open(durable_path(root)).unwrap())
        .lines()
        .map(|line| {
            Arc::new(SessionEvent::from_value(
                serde_json::from_str(&line.unwrap()).unwrap(),
            ))
        })
        .collect()
}

struct CrashAdapter(PathBuf);
impl LlmAdapter for CrashAdapter {
    fn stream(&self, _: GenerateOptions) -> Result<LlmStream, BoxError> {
        write_marker(&self.0, "request-dispatched");
        Ok(LlmStream::new(futures_util::stream::pending()))
    }
}

async fn run_child(mode: &str, root: &Path) {
    let harness = setup().await;
    let session = harness.session("semantic-checkpoint-crash");
    let output = durable_path(root);
    SessionStore::on_flush(&harness.ctx, move |session| {
        let output = output.clone();
        async move { persist_session(&output, &session) }
    })
    .unwrap();

    session
        .append("turn/start", json!({"turn":1}), None)
        .unwrap();
    session
        .append("step/start", json!({"turn":1,"step":1}), None)
        .unwrap();

    if mode == "request" {
        session
            .append(
                "request/header",
                json!({"header":{"config":{"provider":"crash","model":"crash"}},"reason":"initial"}),
                None,
            )
            .unwrap();
        session
            .append(
                "request/context",
                json!({"turn":1,"step":1,"messages":[]}),
                None,
            )
            .unwrap();
        harness
            .llm
            .register_adapter(
                &harness.ctx,
                vec!["crash".into()],
                Arc::new(CrashAdapter(marker_path(root))),
            )
            .unwrap();
        let stream = harness.llm.stream(
            &harness.ctx,
            GenerateOptions {
                provider: "crash".into(),
                model: "crash".into(),
                messages: vec![],
                session_id: Some(session.id().clone()),
                ..Default::default()
            },
        );
        futures_util::pin_mut!(stream);
        let _ = stream.next().await;
        return;
    }

    let message = json!({
        "turn":1,
        "step":1,
        "message":{
            "id":"assistant-tools",
            "role":"assistant",
            "content":[{"type":"tool-call","id":"crash-call","name":"crash_tool","arguments":"{}"}],
            "source":{"kind":"model","provider":"crash","model":"crash"}
        },
        "stream":[]
    });
    session
        .append("assistant/message", message, Some(SurfaceIntent::append()))
        .unwrap();
    session
        .append(
            "tool/call",
            json!({"turn":1,"step":1,"callId":"crash-call","name":"crash_tool","arguments":"{}"}),
            None,
        )
        .unwrap();

    let marker = marker_path(root);
    let definition = define_tool(DefineToolOptions::new(
        "crash_tool",
        "records an external effect and never returns",
        json!({}),
        DefineToolOutput {
            schema: json!({"type":"null"}),
            render: Arc::new(|_, _| vec![]),
            presentation_meta: None,
        },
        move |_, _| {
            write_marker(&marker, "tool-side-effect");
            Box::pin(std::future::pending())
        },
    ))
    .unwrap();
    harness
        .tools
        .register(&harness.ctx, definition, ToolRegistrationOptions::default())
        .unwrap();
    let agent = StubAgent::new(&harness.ctx, session);
    let mut input =
        ToolExecutionInput::new("crash-call".into(), "crash_tool", json!({}), live_signal());
    input.agent = Some(agent as Arc<dyn hoh_tools::Agent>);
    let _ = execute_tool(&harness.ctx, input).await;
}

#[tokio::test]
#[ignore = "child process fixture; parent tests execute it explicitly"]
async fn crash_child_fixture() {
    let mode = std::env::var(MODE_ENV).unwrap();
    let root = PathBuf::from(std::env::var_os(ROOT_ENV).unwrap());
    run_child(&mode, &root).await;
}

fn crash_at(mode: &str, expected: &str) -> PathBuf {
    let root = unique_root(mode);
    fs::create_dir_all(&root).unwrap();
    write_marker(&marker_path(&root), "");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "crash_child_fixture", "--nocapture"])
        .env(MODE_ENV, mode)
        .env(ROOT_ENV, &root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let text = fs::read_to_string(marker_path(&root)).unwrap_or_default();
        if text == expected {
            break;
        }
        if let Some(status) = child.try_wait().unwrap() {
            let stderr = child
                .stderr
                .take()
                .map(|mut stderr| {
                    let mut text = String::new();
                    std::io::Read::read_to_string(&mut stderr, &mut text).unwrap();
                    text
                })
                .unwrap_or_default();
            panic!("crash child exited early ({status}): {stderr}");
        }
        assert!(
            Instant::now() < deadline,
            "crash child did not reach {expected}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    child.kill().unwrap();
    let status = child.wait().unwrap();
    assert_eq!(status.signal(), Some(9));
    root
}

#[test]
fn persists_the_complete_request_before_model_dispatch() {
    let root = crash_at("request", "request-dispatched");
    let events = load_events(&root);
    let closers = hoh_session::interrupted_turn_closers(&events).unwrap();
    assert_eq!(
        events
            .iter()
            .chain(&closers)
            .map(|event| event.event_type())
            .collect::<Vec<_>>(),
        [
            "turn/start",
            "step/start",
            "request/header",
            "request/context",
            "step/end",
            "turn/end"
        ]
    );
    assert_eq!(closers.last().unwrap().event_type(), "turn/end");
    assert_eq!(
        closers.last().unwrap().data()["reason"]["kind"],
        "interrupted"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn persists_tool_intent_before_a_side_effect_and_repairs_the_unknown_outcome() {
    let root = crash_at("tool", "tool-side-effect");
    let events = load_events(&root);
    assert!(
        events
            .iter()
            .any(|event| event.event_type() == "assistant/message")
    );
    assert!(events.iter().any(|event| event.event_type() == "tool/call"));
    let closers = hoh_session::interrupted_turn_closers(&events).unwrap();
    let result = closers
        .iter()
        .find(|event| event.event_type() == "tool/result")
        .unwrap();
    assert_eq!(
        result.data()["error"],
        json!({"name":"ToolOutcomeUnknownError","code":TOOL_OUTCOME_UNKNOWN})
    );
    assert!(
        result.data()["message"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Do not retry blindly.")
    );
    fs::remove_dir_all(root).unwrap();
}
