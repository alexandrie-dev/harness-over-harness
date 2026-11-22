//! End to end through the host, the real engine and a stand-in `claude`: a folder change of a chat
//! with Claude reaches the window as the code `folder-fixed` with the words for the person. The
//! engine refuses with the prefix `EXECUTOR_FOLDER_FIXED: `; the host turns that into exactly the
//! object the window's own tests (ui/tests/chat/fake-engine.mjs) feed the chat, where the text and
//! the button «Новый чат в другой папке» are checked.
//! Needs HOH_REAL_ENGINE (built `hoh`).
use hoh_desktop::{
    approval::{Begun, Broker, Resolution, now_ms},
    bridge,
    engine::{Engine, HostEvent, OnSend, ProcessConfig},
};
use hoh_protocol::ApprovalOutcome;
use serde_json::{Value, json};
use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

const KEY: [u8; 32] = [0x3c; 32];
const SESSION: &str = "chat-folder";
const FAKE_CLAUDE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/tests/claude-live-extra/fake-claude"
);
/// What the window's tests make the engine answer to a folder change of a Claude chat.
const UI_FAKE_ENGINE: &str = include_str!("../../../ui/tests/chat/fake-engine.mjs");
const WORDS: &str =
    "Чат с Claude работает в одной папке. Чтобы работать в другой, начните новый чат";

/// Plays the native core for the engine's requests: every `approval/request` goes through the real
/// Broker and is allowed once (signed without a window when the person already confirmed it).
fn pump(
    engine: Arc<Engine>,
    broker: Arc<Broker>,
    events: mpsc::Receiver<HostEvent>,
    notifications: Arc<Mutex<Vec<Value>>>,
) {
    while let Ok(event) = events.recv() {
        let HostEvent::Request {
            generation,
            id,
            method,
            params,
        } = event
        else {
            if let HostEvent::Notification(frame) = event {
                notifications.lock().unwrap().push(frame);
            }
            continue;
        };
        let resolution = match broker
            .begin_engine_or_grant(generation, id, &method, params, now_ms())
            .unwrap()
        {
            Begun::Signed(resolution) => resolution,
            Begun::Window(open) => {
                let shown = Instant::now().checked_sub(Duration::from_secs(2)).unwrap();
                broker.current(&open.label, now_ms(), shown).unwrap();
                broker
                    .decide(
                        &open.label,
                        &open.view.approval_id,
                        ApprovalOutcome::AllowedOnce,
                        now_ms(),
                        Instant::now(),
                    )
                    .unwrap()
                    .resolution
            }
        };
        if let Resolution::Engine {
            generation,
            id,
            decision: Ok(decision),
        } = resolution
        {
            let frame = hoh_protocol::framing::success(id, serde_json::to_value(decision).unwrap());
            engine.respond(generation, frame).unwrap();
        }
    }
}

/// What `engine_request` (native.rs) gives the main window: the bridge, the person's confirmation
/// («Разрешить один раз») bound to the outgoing request, and a failure as `HostError`.
fn window_request(
    engine: &Arc<Engine>,
    broker: &Arc<Broker>,
    method: &str,
    params: Value,
) -> Result<Value, Value> {
    let sent = Arc::new(Mutex::new(None::<(u64, String)>));
    let arm: OnSend = {
        let (broker, sent, method, params) = (
            broker.clone(),
            sent.clone(),
            method.to_owned(),
            params.clone(),
        );
        Box::new(move |generation: u64, id: &str| {
            broker.arm_in_flight(&method, &params, generation, id, now_ms());
            *sent.lock().unwrap() = Some((generation, id.to_owned()));
        })
    };
    let confirm = |params: &Value| {
        let (open, _decided) = broker.begin_preflight(method, params, now_ms())?;
        let shown = Instant::now().checked_sub(Duration::from_secs(2)).unwrap();
        broker.current(&open.label, now_ms(), shown)?;
        broker.decide(
            &open.label,
            &open.view.approval_id,
            ApprovalOutcome::AllowedOnce,
            now_ms(),
            Instant::now(),
        )?;
        Ok(())
    };
    let answer =
        bridge::request_with_approval_armed(engine, "main", method, params, confirm, Some(arm));
    if let Some((generation, id)) = sent.lock().unwrap().take() {
        broker.disarm(generation, &id);
    }
    answer.map_err(|error| serde_json::to_value(hoh_desktop::errors::host_error(error)).unwrap())
}

#[test]
fn a_folder_change_of_a_claude_chat_reaches_the_window_as_folder_fixed_with_the_words() {
    let engine_bin = PathBuf::from(
        std::env::var_os("HOH_REAL_ENGINE").expect("set HOH_REAL_ENGINE to the built hoh"),
    );
    let root = std::env::temp_dir().join(format!("hoh-desktop-folder-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (data, work, other) = (
        root.join("engine"),
        root.join("work"),
        root.join("elsewhere"),
    );
    let (home, state) = (root.join("home"), root.join("claude-state"));
    for folder in [&work, &other, &home.join(".claude"), &state] {
        std::fs::create_dir_all(folder).unwrap();
    }
    let (work, other) = (work.canonicalize().unwrap(), other.canonicalize().unwrap());
    // The credentials store refuses an engine data folder that others can enter.
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&data)
        .unwrap();
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
    let profile = data.join("profiles").join("window");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(
        profile.join("package.json"),
        json!({"name":"dsh-profile-window","private":true,"dependencies":{},
               "dsh":{"profile":{"bundles":["@hoh/window"]}}})
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        profile.join("cordis.patch.yml"),
        format!(
            "- id: model-host\n  config:\n    baseUrl: http://127.0.0.1:9/v1\n    model: window-mock\n\
             - id: harness-claude\n  config:\n    executable: {FAKE_CLAUDE}\n"
        ),
    )
    .unwrap();

    let (events, event_rx) = mpsc::channel();
    let engine = Engine::start(
        Arc::new(ProcessConfig {
            path: engine_bin,
            args: vec!["--profile".into(), "window".into()],
            env: vec![
                ("DSH_HOME".into(), data.to_str().unwrap().into()),
                ("DSH_TELEMETRY_DISABLED".into(), "1".into()),
                ("HOME".into(), home.to_str().unwrap().into()),
                ("FAKE_STATE".into(), state.to_str().unwrap().into()),
            ],
            stderr_file: Some(root.join("engine-stderr.log")),
            cwd: work.clone(),
            provider: "openai-compatible".into(),
            model: "local".into(),
        }),
        json!({
            "cwd":work, "provider":"openai-compatible", "model":"window-mock",
            "capabilities":{"approvals":"host","userQuestions":true},
            "hoh":{"protocolVersion":1,"approvalKey":hoh_protocol::approval::hex(&KEY)}
        }),
        Arc::new(move |event| {
            let _ = events.send(event);
        }),
    );
    let stderr = || std::fs::read_to_string(root.join("engine-stderr.log")).unwrap_or_default();
    loop {
        let event = event_rx
            .recv_timeout(Duration::from_secs(60))
            .unwrap_or_else(|_| panic!("the engine did not start: {}", stderr()));
        if matches!(event, HostEvent::Status(v) if v["state"] == "ready") {
            break;
        }
    }
    let broker = Arc::new(Broker::new(KEY).unwrap());
    let notifications = Arc::new(Mutex::new(Vec::<Value>::new()));
    {
        let (engine, broker, notifications) =
            (engine.clone(), broker.clone(), notifications.clone());
        thread::spawn(move || pump(engine, broker, event_rx, notifications));
    }

    let selected = window_request(
        &engine,
        &broker,
        "hoh/executor/select",
        json!({"executor":"claude","model":"default"}),
    )
    .unwrap_or_else(|error| panic!("executor/select: {error}\n{}", stderr()));
    assert_eq!(selected["executor"], "claude", "{selected}");

    // One turn, so the chat is Claude's conversation (the stand-in keeps the word under its id).
    let idle = |notifications: &Mutex<Vec<Value>>| {
        notifications
            .lock()
            .unwrap()
            .iter()
            .filter(|n| {
                n["method"] == "session.status"
                    && n["params"]["sessionId"] == SESSION
                    && n["params"]["status"] == "idle"
            })
            .count()
    };
    let before = idle(&notifications);
    let prompt = window_request(
        &engine,
        &broker,
        "session/prompt",
        json!({"sessionId":SESSION,"contentBlocks":[{"type":"text","text":"Remember the word HOHFI7. Reply with ok."}]}),
    )
    .unwrap_or_else(|error| panic!("session/prompt: {error}\n{}", stderr()));
    assert!(prompt["messageId"].is_string(), "{prompt}");
    let until = Instant::now() + Duration::from_secs(60);
    while idle(&notifications) <= before {
        assert!(Instant::now() < until, "the turn did not end: {}", stderr());
        thread::sleep(Duration::from_millis(50));
    }
    let kept = std::fs::read_dir(&state).unwrap().count();
    assert_eq!(kept, 1, "the stand-in claude did not run the turn");
    let folder = |engine: &Arc<Engine>| {
        window_request(
            engine,
            &broker,
            "session/working-directory/get",
            json!({"sessionId":SESSION}),
        )
        .unwrap()
    };
    let was = folder(&engine);

    let refused = window_request(
        &engine,
        &broker,
        "session/working-directory/set",
        json!({"sessionId":SESSION,"path":other}),
    )
    .expect_err("a Claude chat took another folder");
    assert_eq!(
        refused,
        json!({"code":"folder-fixed","message":WORDS}),
        "{}",
        stderr()
    );
    assert_eq!(folder(&engine), was, "the folder changed");
    // The window's tests start from this very object.
    let thrown = format!("throw {{ code: 'folder-fixed', message: '{WORDS}' }}");
    assert!(
        UI_FAKE_ENGINE.contains(&thrown),
        "ui/tests/chat/fake-engine.mjs no longer throws {thrown}"
    );

    engine.stop();
    let _ = std::fs::remove_dir_all(&root);
}
