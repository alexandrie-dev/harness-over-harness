//! The native host against the real `hoh` engine (window profile) and the scenario model mock.
//! Needs HOH_REAL_ENGINE (built `hoh`) and HOH_MOCK_BIN (built `hoh-llm-openai-mock`).
use hoh_desktop::{
    approval::{Begun, Broker, Resolution, now_ms},
    engine::{Engine, HostEvent, ProcessConfig},
    questions::QuestionBroker,
};
use hoh_protocol::ApprovalOutcome;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader},
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

const KEY: [u8; 32] = [0x5a; 32];

struct Mock {
    child: Child,
    base_url: String,
}
impl Drop for Mock {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn start_mock(scenario: &str, journal: &std::path::Path) -> Mock {
    let bin = std::env::var_os("HOH_MOCK_BIN").expect("set HOH_MOCK_BIN to hoh-llm-openai-mock");
    let mut child = Command::new(bin)
        .args(["--scenario", scenario, "--journal"])
        .arg(journal)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let ready: Value = serde_json::from_str(&line).unwrap();
    Mock {
        child,
        base_url: ready["baseURL"].as_str().unwrap().to_owned(),
    }
}

#[derive(Default)]
struct Seen {
    notifications: Vec<Value>,
    /// Windows opened for the engine's own requests (texts). The settings window is not counted.
    approvals: Vec<String>,
    /// Engine requests signed by the core without a window after the person confirmed the setting.
    signed_without_window: usize,
    /// Every frame the engine sent to the core as a request (what would reach the window's approval path).
    engine_requests: Vec<Value>,
    /// The person presses "Отклонить" instead of "Разрешить один раз".
    reject: bool,
    questions: Vec<Value>,
}

/// Plays the native core: every engine `approval/request` goes through the real Broker and is
/// allowed once, exactly as the approval window would after the one-second delay.
fn pump(
    engine: Arc<Engine>,
    broker: Arc<Broker>,
    questions: Arc<QuestionBroker>,
    events: std::sync::mpsc::Receiver<HostEvent>,
    seen: Arc<Mutex<Seen>>,
) {
    while let Ok(event) = events.recv() {
        match event {
            HostEvent::Notification(frame) => seen.lock().unwrap().notifications.push(frame),
            HostEvent::Status(_) => {}
            HostEvent::Request {
                generation,
                id,
                method,
                params,
            } if method == "user-question/ask" => {
                seen.lock().unwrap().engine_requests.push(params.clone());
                // The person picks the first option of every question.
                let view = questions.begin(generation, id, params).unwrap();
                let answers: Vec<Value> = view
                    .questions
                    .iter()
                    .map(|q| json!({"id":q.id,"selected":[q.options.as_ref().unwrap()[0].label]}))
                    .collect();
                seen.lock()
                    .unwrap()
                    .questions
                    .push(serde_json::to_value(&view).unwrap());
                let (generation, frame) = questions
                    .answer(&view.request_id, &json!({"answers":answers}))
                    .unwrap();
                engine.respond(generation, frame).unwrap();
            }
            HostEvent::Request {
                generation,
                id,
                method,
                params,
            } => {
                seen.lock().unwrap().engine_requests.push(params.clone());
                let open = match broker
                    .begin_engine_or_grant(generation, id, &method, params, now_ms())
                    .unwrap()
                {
                    Begun::Window(open) => open,
                    Begun::Signed(Resolution::Engine {
                        generation,
                        id,
                        decision: Ok(decision),
                    }) => {
                        seen.lock().unwrap().signed_without_window += 1;
                        engine
                            .respond(
                                generation,
                                hoh_protocol::framing::success(
                                    id,
                                    serde_json::to_value(decision).unwrap(),
                                ),
                            )
                            .unwrap();
                        continue;
                    }
                    Begun::Signed(_) => panic!("unexpected resolution"),
                };
                seen.lock()
                    .unwrap()
                    .approvals
                    .push(open.view.human_text.clone());
                let approval_id = open.view.approval_id.clone();
                broker
                    .current(
                        &open.label,
                        now_ms(),
                        Instant::now().checked_sub(Duration::from_secs(2)).unwrap(),
                    )
                    .unwrap();
                let outcome = if seen.lock().unwrap().reject {
                    ApprovalOutcome::Rejected
                } else {
                    ApprovalOutcome::AllowedOnce
                };
                let decided = broker
                    .decide(&open.label, &approval_id, outcome, now_ms(), Instant::now())
                    .unwrap();
                if let Resolution::Engine {
                    generation,
                    id,
                    decision: Ok(decision),
                } = decided.resolution
                {
                    engine
                        .respond(
                            generation,
                            hoh_protocol::framing::success(
                                id,
                                serde_json::to_value(decision).unwrap(),
                            ),
                        )
                        .unwrap();
                }
            }
        }
    }
}

fn wait_for(seen: &Arc<Mutex<Seen>>, what: &str, check: impl Fn(&Seen) -> bool) {
    let until = Instant::now() + Duration::from_secs(60);
    while Instant::now() < until {
        if check(&seen.lock().unwrap()) {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!(
        "timeout waiting for {what}: {:#?}",
        seen.lock().unwrap().notifications
    );
}

struct Harness {
    engine: Arc<Engine>,
    broker: Arc<Broker>,
    mock: Mock,
    seen: Arc<Mutex<Seen>>,
    work: PathBuf,
    root: PathBuf,
    journal: PathBuf,
    home: PathBuf,
    /// Every answer the engine gave the core.
    responses: Mutex<Vec<Value>>,
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.engine.stop();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
impl Harness {
    fn request(&self, method: &str, params: Value) -> Value {
        self.request_as_window_action(method, params, false)
    }
    /// `window_action`: the request is the one a confirmed window action became, so the core binds
    /// the person's confirmation to its id while it is in flight (as `engine_request` does).
    fn request_as_window_action(&self, method: &str, params: Value, window_action: bool) -> Value {
        // The window cannot ask before the handshake has finished.
        let until = Instant::now() + Duration::from_secs(60);
        loop {
            let sent = Arc::new(Mutex::new(None::<(u64, String)>));
            let arm: Option<hoh_desktop::engine::OnSend> = window_action.then(|| {
                let (broker, sent, method, params) = (
                    self.broker.clone(),
                    sent.clone(),
                    method.to_owned(),
                    params.clone(),
                );
                Box::new(move |generation: u64, id: &str| {
                    broker.arm_in_flight(&method, &params, generation, id, now_ms());
                    *sent.lock().unwrap() = Some((generation, id.to_owned()));
                }) as hoh_desktop::engine::OnSend
            });
            let outcome = self.engine.request_with(method, params.clone(), arm);
            if let Some((generation, id)) = sent.lock().unwrap().take() {
                self.broker.disarm(generation, &id);
            }
            match outcome {
                Ok(value) => {
                    self.responses.lock().unwrap().push(value.clone());
                    return value;
                }
                Err(error) if error.contains("подключается") && Instant::now() < until =>
                {
                    thread::sleep(Duration::from_millis(100));
                }
                Err(error) => panic!("{method}: {error}"),
            }
        }
    }
    /// A settings action the way the core runs it: the person confirms it once in the settings
    /// window, then the engine's own approval request is signed without a second window.
    fn confirmed_by_person(&self, method: &str, params: Value) -> Value {
        let (open, _receiver) = self
            .broker
            .begin_preflight(method, &params, now_ms())
            .unwrap();
        self.broker
            .current(
                &open.label,
                now_ms(),
                Instant::now().checked_sub(Duration::from_secs(2)).unwrap(),
            )
            .unwrap();
        self.broker
            .decide(
                &open.label,
                &open.view.approval_id,
                ApprovalOutcome::AllowedOnce,
                now_ms(),
                Instant::now(),
            )
            .unwrap();
        self.request_as_window_action(method, params, true)
    }
    /// First run through the real privileged path: key, then address and model.
    fn first_run(&self) {
        self.confirmed_by_person(
            "hoh/model/key/set",
            json!({"apiKey":"sk-real-engine-test","baseUrl":self.mock.base_url}),
        );
        self.confirmed_by_person(
            "hoh/settings/set",
            json!({"model":{"baseUrl":self.mock.base_url,"model":"window-mock"}}),
        );
    }
    /// The model key goes core -> engine over stdin and into the credential store, and nowhere else:
    /// not into frames engine -> core (notifications, requests, answers), the approval journal, the
    /// engine's stderr, or any file outside the credential store.
    fn assert_key_only_in_credential_store(&self, key: &str) {
        let seen = self.seen.lock().unwrap();
        let frames = [
            (
                "notifications",
                serde_json::to_string(&seen.notifications).unwrap(),
            ),
            (
                "engine requests",
                serde_json::to_string(&seen.engine_requests).unwrap(),
            ),
            (
                "engine answers",
                serde_json::to_string(&*self.responses.lock().unwrap()).unwrap(),
            ),
            (
                "audit log in memory",
                serde_json::to_string(&self.broker.audit_log()).unwrap(),
            ),
        ];
        for (name, text) in frames {
            assert!(!text.contains(key), "the key reached {name}");
        }
        let mut holders = Vec::new();
        let mut folders = vec![self.root.clone()];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(folder).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    folders.push(path);
                } else if std::fs::read(&path).is_ok_and(|bytes| {
                    bytes
                        .windows(key.len())
                        .any(|window| window == key.as_bytes())
                }) {
                    holders.push(path);
                }
            }
        }
        // Everything under the test root is scanned: approvals.log, engine-stderr.log, the work
        // folder, the model journal, the engine's data folder. Only the credential store may hold it.
        assert!(
            holders.iter().all(|path| path.starts_with(&self.home)
                && path.to_string_lossy().contains("credential")),
            "files holding the key: {holders:?}"
        );
        assert_eq!(
            holders.len(),
            1,
            "the key must be in the credential store: {holders:?}"
        );
    }
    /// Last notifications, for failure messages.
    fn tail(&self) -> String {
        let seen = self.seen.lock().unwrap();
        let from = seen.notifications.len().saturating_sub(40);
        serde_json::to_string_pretty(&seen.notifications[from..]).unwrap()
    }
    fn idle_count(&self) -> usize {
        self.seen
            .lock()
            .unwrap()
            .notifications
            .iter()
            .filter(|n| n["method"] == "session.status" && n["params"]["status"] == "idle")
            .count()
    }
    fn prompt_and_wait(&self, text: &str) {
        let before = self.idle_count();
        let prompt = self.request(
            "session/prompt",
            json!({"sessionId":"window-main","contentBlocks":[{"type":"text","text":text}]}),
        );
        assert!(prompt["messageId"].is_string());
        wait_for(&self.seen, "turn to finish", |s| {
            s.notifications
                .iter()
                .filter(|n| n["method"] == "session.status" && n["params"]["status"] == "idle")
                .count()
                > before
        });
    }
}

fn launch(scenario: &str, tag: &str) -> Harness {
    let engine_bin = PathBuf::from(
        std::env::var_os("HOH_REAL_ENGINE").expect("set HOH_REAL_ENGINE to the built hoh"),
    );
    let root = std::env::temp_dir().join(format!("hoh-desktop-real-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("engine");
    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join("notes.txt"), "заметка\n").unwrap();
    // The launcher creates the engine's data folder private (credentials refuse anything else).
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&home)
        .unwrap();
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
    let journal = root.join("model-journal.jsonl");
    let mock = start_mock(scenario, &journal);

    let (events, event_rx) = std::sync::mpsc::channel();
    let engine = Engine::start(
        Arc::new(ProcessConfig {
            path: engine_bin,
            args: vec!["--profile".into(), "window".into()],
            env: vec![("DSH_HOME".into(), home.to_str().unwrap().into())],
            stderr_file: Some(root.join("engine-stderr.log")),
            cwd: work.clone(),
            provider: "openai-compatible".into(),
            model: "local".into(),
        }),
        json!({
            "cwd":work, "provider":"openai-compatible", "model":"local",
            "capabilities":{"approvals":"host","userQuestions":true},
            "hoh":{"protocolVersion":1,"approvalKey":hoh_protocol::approval::hex(&KEY)}
        }),
        Arc::new(move |event| {
            let _ = events.send(event);
        }),
    );
    let broker = Arc::new(
        Broker::new(KEY)
            .unwrap()
            .with_audit_file(root.join("approvals.log")),
    );
    let seen = Arc::new(Mutex::new(Seen::default()));
    {
        let (engine, seen, broker) = (engine.clone(), seen.clone(), broker.clone());
        thread::spawn(move || {
            pump(
                engine,
                broker,
                Arc::new(QuestionBroker::default()),
                event_rx,
                seen,
            )
        });
    }
    Harness {
        engine,
        broker,
        mock,
        seen,
        work,
        root,
        journal,
        home,
        responses: Mutex::new(Vec::new()),
    }
}

const WINDOW_SCENARIO: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../crates/hoh-llm-openai-mock/scenarios/window.json"
);
const DESTROY_SCENARIO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/scenarios/destroy.json");
const QUESTION_SCENARIO: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/scenarios/question.json");

#[test]
fn first_run_and_a_streamed_turn_work_against_the_real_engine() {
    let h = launch(WINDOW_SCENARIO, "turn");
    h.first_run();
    let settings = h.request("hoh/settings/get", json!({}));
    assert_eq!(settings["model"]["model"], "window-mock");
    // One window per setting (the settings window), none from the engine for the same actions.
    assert_eq!(h.seen.lock().unwrap().approvals.len(), 0);
    assert_eq!(h.seen.lock().unwrap().signed_without_window, 2);
    let probe = h.confirmed_by_person(
        "hoh/model/probe",
        json!({"baseUrl":h.mock.base_url,"model":"window-mock"}),
    );
    assert_eq!(probe["ok"], true, "{probe}");
    assert_eq!(h.seen.lock().unwrap().signed_without_window, 3);
    let journal = h.broker.audit_log();
    assert_eq!(
        journal
            .iter()
            .filter(|e| e.note == "подтверждено человеком в окне настройки")
            .count(),
        3
    );
    assert!(
        !serde_json::to_string(&journal)
            .unwrap()
            .contains("sk-real-engine-test")
    );

    h.prompt_and_wait("привет");
    let text = serde_json::to_string(&h.seen.lock().unwrap().notifications).unwrap();
    assert!(text.contains("Это поток от mock."), "{text}");
    assert!(!text.contains("sk-real-engine-test"));

    // The scenario's second turn runs bash, read and write (and an escalated command).
    h.prompt_and_wait("очисти");
    assert!(
        h.work.join("result.txt").exists(),
        "write tool did not run: {}",
        h.tail()
    );

    h.assert_key_only_in_credential_store("sk-real-engine-test");

    let events = h.request(
        "session/events",
        json!({"sessionId":"window-main","limit":1000}),
    );
    assert!(!events["events"].as_array().unwrap().is_empty());
    let list = h.request("session/list", json!({}));
    assert!(!list["sessions"].as_array().unwrap().is_empty(), "{list}");
}

/// QA A-05: after the app is started again on the same data the earlier conversation must come
/// back. The engine keeps the session on disk but does not open it by itself, so the window
/// resumes it first (ui/src/screens/chat/controller.mjs) and only then asks for its folder and events.
#[test]
fn a_restarted_engine_on_the_same_data_gives_back_the_conversation_after_a_resume() {
    let h = launch(WINDOW_SCENARIO, "restart");
    h.first_run();
    h.prompt_and_wait("привет");
    // the person closes the app and opens it again: a new engine process on the same data folder
    h.engine.restart_in(&h.work).unwrap();
    let resumed = h.request("session/resume", json!({"sessionId":"window-main"}));
    assert_eq!(resumed, json!({}));
    let folder = h.request(
        "session/working-directory/get",
        json!({"sessionId":"window-main"}),
    );
    assert_eq!(folder["cwd"], json!(h.work));
    let events = h.request(
        "session/events",
        json!({"sessionId":"window-main","limit":1000}),
    );
    let text = serde_json::to_string(&events["events"]).unwrap();
    assert!(
        text.contains("привет"),
        "the person's message is gone: {text}"
    );
    assert!(
        text.contains("Это поток от mock."),
        "the answer is gone: {text}"
    );
}

/// Condition of the milestone (CEO 19.11): deleting files INSIDE the work folder also asks the
/// person. Rejecting leaves the files; allowing once removes them. The engine does not do this
/// yet, so this test is expected to stay red until the engine change lands.
#[test]
fn first_run_destructive_action_inside_the_folder_waits_for_the_person() {
    let h = launch(DESTROY_SCENARIO, "destroy");
    h.first_run();
    let target = h.work.join("generated");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("a.txt"), "a").unwrap();
    let asked = h.seen.lock().unwrap().approvals.len();

    h.seen.lock().unwrap().reject = true;
    h.prompt_and_wait("удали generated");
    assert!(
        h.seen.lock().unwrap().approvals.len() > asked,
        "deleting inside the folder was not sent for approval"
    );
    assert!(
        target.join("a.txt").exists(),
        "files were deleted although the person rejected"
    );

    h.seen.lock().unwrap().reject = false;
    h.prompt_and_wait("удали generated ещё раз");
    assert!(
        !target.exists(),
        "files stayed although the person allowed once"
    );
}

#[test]
fn the_session_is_rooted_in_the_chosen_folder_and_follows_a_new_choice() {
    let h = launch(WINDOW_SCENARIO, "folder");
    let first = h.request(
        "session/working-directory/get",
        json!({"sessionId":"window-main"}),
    );
    assert_eq!(first["cwd"], json!(h.work), "{first}");
    assert_ne!(
        std::path::Path::new(first["cwd"].as_str().unwrap()),
        std::env::home_dir().unwrap()
    );

    // What the core does after the person's rules were allowed: reopen in the chosen folder.
    let chosen = h.root.join("chosen");
    std::fs::create_dir_all(&chosen).unwrap();
    h.engine.restart_in(&chosen).unwrap();
    let until = Instant::now() + Duration::from_secs(60);
    loop {
        match h.engine.request(
            "session/working-directory/get",
            json!({"sessionId":"window-main"}),
        ) {
            Ok(value) if value["cwd"] == json!(chosen) => break,
            _ if Instant::now() < until => thread::sleep(Duration::from_millis(100)),
            other => panic!("engine did not reopen in the chosen folder: {other:?}"),
        }
    }
}

#[test]
fn the_assistant_question_reaches_the_core_and_the_answer_reaches_the_model() {
    let h = launch(QUESTION_SCENARIO, "question");
    h.first_run();
    h.prompt_and_wait("подготовь отчёт");
    {
        let seen = h.seen.lock().unwrap();
        assert_eq!(seen.questions.len(), 1, "{:#?}", seen.notifications);
        assert_eq!(seen.questions[0]["sessionId"], "window-main");
        assert_eq!(
            seen.questions[0]["questions"][0]["question"],
            "Какой формат отчёта?"
        );
    }
    // The next model request carries the chosen option back as the tool result.
    let journal = std::fs::read_to_string(&h.journal).unwrap();
    let requests: Vec<&str> = journal.lines().collect();
    assert!(requests.len() >= 2, "{journal}");
    assert!(requests.last().unwrap().contains("PDF"), "{journal}");
}
