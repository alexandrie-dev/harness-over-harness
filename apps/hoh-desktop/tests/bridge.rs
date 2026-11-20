use hoh_desktop::{
    approval::{Broker, Resolution, now_ms},
    bridge,
    engine::{Connector, Engine, HostEvent, Message, ProcessConfig, Transport},
};
use hoh_protocol::{framing, transport::classify};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

type Frames = Arc<Mutex<Vec<Value>>>;
struct MemoryConnector {
    host: bool,
    frames: Frames,
    launches: Arc<AtomicUsize>,
    controls: mpsc::Sender<(u64, mpsc::Sender<Message>)>,
}
struct MemoryTransport {
    host: bool,
    frames: Frames,
    generation: u64,
    inbox: mpsc::Sender<Message>,
    closed: bool,
}

struct FailingInitializeConnector {
    launches: Arc<AtomicUsize>,
}

struct FailingInitializeTransport;

impl Connector for FailingInitializeConnector {
    fn connect(
        &self,
        _generation: u64,
        _inbox: mpsc::Sender<Message>,
    ) -> Result<Box<dyn Transport>, String> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FailingInitializeTransport))
    }
}

impl Transport for FailingInitializeTransport {
    fn send(&mut self, _frame: Value) -> Result<(), String> {
        Err("initialize send failed".into())
    }

    fn exited(&mut self) -> bool {
        false
    }

    fn close(&mut self) {}
}
impl Connector for MemoryConnector {
    fn connect(
        &self,
        generation: u64,
        inbox: mpsc::Sender<Message>,
    ) -> Result<Box<dyn Transport>, String> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        self.controls.send((generation, inbox.clone())).unwrap();
        Ok(Box::new(MemoryTransport {
            host: self.host,
            frames: self.frames.clone(),
            generation,
            inbox,
            closed: false,
        }))
    }
}
impl Transport for MemoryTransport {
    fn send(&mut self, frame: Value) -> Result<(), String> {
        self.frames.lock().unwrap().push(frame.clone());
        if frame["method"] == "hoh/test/missing" {
            self.inbox
                .send(Message::Incoming(
                    self.generation,
                    classify(framing::error(
                        frame["id"].clone(),
                        -32601,
                        "method not found: /etc/secret",
                    ))
                    .unwrap(),
                ))
                .unwrap();
            return Ok(());
        }
        let result = if frame["method"] == "initialize" && self.host {
            json!({"serverInfo":{"name":"deepseek-harness-sdk-runtime","version":"test"},"hoh":{"protocolVersion":1,"approvals":"host"}})
        } else if frame["method"] == "initialize" {
            json!({"serverInfo":{"name":"deepseek-harness-sdk-runtime","version":"legacy"}})
        } else {
            json!({})
        };
        self.inbox
            .send(Message::Incoming(
                self.generation,
                classify(framing::success(frame["id"].clone(), result)).unwrap(),
            ))
            .unwrap();
        Ok(())
    }
    fn exited(&mut self) -> bool {
        self.closed
    }
    fn close(&mut self) {
        self.closed = true;
    }
}

fn wait_memory_ready(events: &mpsc::Receiver<HostEvent>) {
    loop {
        if matches!(events.recv_timeout(Duration::from_secs(3)).unwrap(), HostEvent::Status(value) if value["state"] == "ready")
        {
            return;
        }
    }
}

#[test]
fn privileged_catalog_and_conditional_boundaries_are_fail_closed() {
    for method in hoh_protocol::PRIVILEGED_METHODS {
        assert!(bridge::WINDOW_METHODS.contains(method), "missing {method}");
    }
    let explicit_null = json!({"id":"employee-1","patch":{"permissions":null},"rev":1});
    assert!(bridge::needs_approval(
        "hoh/employee/update",
        &explicit_null
    ));
    assert!(bridge::check_main("main", "hoh/employee/update", &explicit_null).is_err());
    assert!(bridge::needs_approval(
        "hoh/settings/set",
        &json!({"security":{}})
    ));
    assert!(bridge::needs_approval(
        "hoh/settings/set",
        &json!({"model":{}})
    ));
    assert!(!bridge::needs_approval(
        "hoh/settings/set",
        &json!({"view":{"theme":"system","fontSize":16}})
    ));
}

#[test]
fn missing_engine_method_is_marked_without_leaking_error_text() {
    let (controls, _control_rx) = mpsc::channel();
    let (events, event_rx) = mpsc::channel();
    let engine = Engine::start(
        Arc::new(MemoryConnector {
            host: true,
            frames: Arc::default(),
            launches: Arc::default(),
            controls,
        }),
        json!({}),
        Arc::new(move |event| {
            let _ = events.send(event);
        }),
    );
    wait_memory_ready(&event_rx);
    let error = engine.request("hoh/test/missing", json!({})).unwrap_err();
    assert_eq!(error, hoh_desktop::engine::METHOD_UNAVAILABLE);
    engine.stop();
}

#[test]
fn initialize_send_failure_observes_stop_without_restart_spin() {
    let launches = Arc::new(AtomicUsize::new(0));
    let engine = Engine::start(
        Arc::new(FailingInitializeConnector {
            launches: launches.clone(),
        }),
        json!({}),
        Arc::new(|_| {}),
    );
    let (done, stopped) = mpsc::channel();
    thread::spawn(move || {
        engine.stop();
        let _ = done.send(());
    });
    assert!(stopped.recv_timeout(Duration::from_secs(1)).is_ok());
    assert!(launches.load(Ordering::SeqCst) <= 2);
}

#[test]
fn second_parallel_privileged_call_never_reaches_the_engine() {
    let frames = Arc::new(Mutex::new(Vec::new()));
    let launches = Arc::new(AtomicUsize::new(0));
    let (controls, _control_rx) = mpsc::channel();
    let (events, event_rx) = mpsc::channel();
    let engine = Engine::start(
        Arc::new(MemoryConnector {
            host: true,
            frames: frames.clone(),
            launches,
            controls,
        }),
        json!({}),
        Arc::new(move |event| {
            let _ = events.send(event);
        }),
    );
    wait_memory_ready(&event_rx);
    let broker = Arc::new(Broker::new([0x42; 32]).unwrap());
    let (opened, open_rx) = mpsc::channel();
    let first_engine = engine.clone();
    let first_broker = broker.clone();
    let first = thread::spawn(move || {
        bridge::request_with_approval(
            &first_engine,
            "main",
            "hoh/model/probe",
            json!({"baseUrl":"https://first.example.test","model":"first"}),
            |params| {
                let (open, receiver) =
                    first_broker.begin_preflight("hoh/model/probe", params, now_ms())?;
                opened.send(open.label).unwrap();
                match receiver.recv_timeout(Duration::from_secs(3)) {
                    Ok(true) => Ok(()),
                    _ => Err("rejected".into()),
                }
            },
        )
    });
    let first_label = open_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let before = frames.lock().unwrap().len();
    let second = bridge::request_with_approval(
        &engine,
        "main",
        "hoh/model/probe",
        json!({"baseUrl":"https://second.example.test","model":"second"}),
        |params| {
            broker
                .begin_preflight("hoh/model/probe", params, now_ms())
                .map(|_| ())
        },
    );
    assert!(second.is_err());
    assert_eq!(frames.lock().unwrap().len(), before);
    match broker.reject(&first_label, now_ms()).unwrap() {
        Resolution::Preflight { reply, allowed } => {
            assert!(!allowed);
            reply.send(allowed).unwrap();
        }
        Resolution::Engine { .. } => panic!("expected preflight resolution"),
    }
    assert!(first.join().unwrap().is_err());
    assert_eq!(frames.lock().unwrap().len(), before);
    engine.stop();
}
#[test]
fn filtered_calls_notifications_restart_and_shutdown_use_replaceable_transport() {
    let frames = Arc::new(Mutex::new(Vec::new()));
    let launches = Arc::new(AtomicUsize::new(0));
    let (controls, control_rx) = mpsc::channel();
    let (events, event_rx) = mpsc::channel();
    let engine = Engine::start(
        Arc::new(MemoryConnector {
            host: true,
            frames: frames.clone(),
            launches: launches.clone(),
            controls,
        }),
        json!({}),
        Arc::new(move |e| {
            let _ = events.send(e);
        }),
    );
    let ready = || loop {
        if matches!(event_rx.recv_timeout(Duration::from_secs(3)).unwrap(), HostEvent::Status(v) if v["state"] == "ready")
        {
            break;
        }
    };
    ready();
    let (generation, inbox) = control_rx.recv().unwrap();
    assert!(bridge::request(&engine, "main", "session/cancel", json!({"sessionId":"s"})).is_ok());
    for method in ["initialize", "shutdown", "approval/request", "not/a/method"] {
        assert!(bridge::request(&engine, "main", method, json!({})).is_err());
    }
    assert!(
        bridge::request(
            &engine,
            "approval",
            "session/cancel",
            json!({"sessionId":"s"})
        )
        .is_err()
    );
    let before = frames.lock().unwrap().len();
    assert!(
        bridge::request(
            &engine,
            "main",
            "hoh/model/probe",
            json!({"baseUrl":"https://example.invalid","model":"m"})
        )
        .is_err()
    );
    assert_eq!(
        frames.lock().unwrap().len(),
        before,
        "unconfirmed privileged method never reaches engine"
    );
    let gate_before = frames.lock().unwrap().len();
    assert!(
        bridge::request_with_approval(
            &engine,
            "main",
            "hoh/model/probe",
            json!({"baseUrl":"https://example.invalid","model":"m"}),
            |_| Err("rejected by native preflight".into()),
        )
        .is_err()
    );
    assert_eq!(
        frames.lock().unwrap().len(),
        gate_before,
        "the helper used by the Tauri command does not send before consent"
    );
    inbox
        .send(Message::Incoming(
            generation,
            classify(framing::notification(
                "approval.resolved",
                json!({"approvalId":"a","outcome":"rejected"}),
            ))
            .unwrap(),
        ))
        .unwrap();
    loop {
        if matches!(event_rx.recv_timeout(Duration::from_secs(3)).unwrap(), HostEvent::Notification(v) if v["method"] == "approval.resolved")
        {
            break;
        }
    }
    inbox.send(Message::Disconnected(generation)).unwrap();
    ready();
    assert_eq!(launches.load(Ordering::SeqCst), 2);
    engine.stop();
    assert!(
        engine
            .request("session/cancel", json!({"sessionId":"s"}))
            .is_err()
    );
}

#[test]
fn legacy_handshake_cannot_enable_window_requests() {
    let frames = Arc::new(Mutex::new(Vec::new()));
    let (controls, _controls) = mpsc::channel();
    let (events, rx) = mpsc::channel();
    let engine = Engine::start(
        Arc::new(MemoryConnector {
            host: false,
            frames: frames.clone(),
            launches: Arc::new(AtomicUsize::new(0)),
            controls,
        }),
        json!({}),
        Arc::new(move |e| {
            let _ = events.send(e);
        }),
    );
    loop {
        if let HostEvent::Status(v) = rx.recv_timeout(Duration::from_secs(3)).unwrap() {
            assert_ne!(v["state"], "ready");
            if v["state"] == "restarting" {
                break;
            }
        }
    }
    assert!(
        engine
            .request("session/cancel", json!({"sessionId":"s"}))
            .is_err()
    );
    engine.stop();
    assert!(
        frames
            .lock()
            .unwrap()
            .iter()
            .all(|f| f["method"] == "initialize" || f["method"] == "shutdown")
    );
}

#[test]
fn real_stub_child_accepts_host_handshake_serves_settings_and_stops() {
    let path = std::env::var_os("HOH_TEST_ENGINE")
        .expect("set HOH_TEST_ENGINE to the built hoh-window-stub");
    let (events, rx) = mpsc::channel();
    let cwd = std::env::current_dir().unwrap();
    let init = json!({"cwd":cwd,"provider":"mock","model":"mock","capabilities":{"approvals":"host"},"hoh":{"protocolVersion":1,"approvalKey":"42".repeat(32)}});
    let engine = Engine::start(
        Arc::new(ProcessConfig {
            path: path.into(),
            args: vec![],
            env: vec![],
            stderr_file: None,
            cwd,
            provider: "mock".into(),
            model: "mock".into(),
        }),
        init,
        Arc::new(move |e| {
            let _ = events.send(e);
        }),
    );
    loop {
        if matches!(rx.recv_timeout(Duration::from_secs(30)).unwrap(), HostEvent::Status(v) if v["state"] == "ready")
        {
            break;
        }
    }
    let settings = bridge::request(&engine, "main", "hoh/settings/get", json!({})).unwrap();
    hoh_protocol::validate_result("hoh/settings/get", &settings).unwrap();
    let opened = bridge::request(
        &engine,
        "main",
        "hoh/employee/open",
        json!({"id":"employee-1"}),
    )
    .unwrap();
    let session = opened["sessionId"].as_str().unwrap();
    bridge::request(
        &engine,
        "main",
        "session/resume",
        json!({"sessionId":session}),
    )
    .unwrap();
    let prompt = bridge::request(&engine, "main", "session/prompt", json!({"sessionId":session,"contentBlocks":[{"type":"text","text":"Reply with exactly: SDK snapshot OK"}]})).unwrap();
    assert!(!prompt["messageId"].as_str().unwrap().is_empty());
    bridge::request(
        &engine,
        "main",
        "session/wait",
        json!({"sessionId":session}),
    )
    .unwrap();
    loop {
        if let HostEvent::Notification(v) = rx.recv_timeout(Duration::from_secs(30)).unwrap() {
            assert!(
                !serde_json::to_string(&v)
                    .unwrap()
                    .contains(&"42".repeat(32))
            );
            if v["method"] == "session.event" && v["params"]["event"]["type"] == "assistant/message"
            {
                let text: String = v["params"]["event"]["data"]["message"]["content"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|b| b["type"] == "text")
                    .filter_map(|b| b["text"].as_str())
                    .collect();
                assert_eq!(text, "SDK snapshot OK");
                break;
            }
        }
    }
    engine.stop();
    assert!(engine.request("hoh/settings/get", json!({})).is_err());
}

#[test]
fn window_may_call_exactly_the_four_executor_methods_and_no_new_ones() {
    let mut executor: Vec<&str> = bridge::WINDOW_METHODS
        .iter()
        .copied()
        .filter(|method| method.starts_with("hoh/executor/"))
        .collect();
    executor.sort_unstable();
    assert_eq!(
        executor,
        [
            "hoh/executor/access/get",
            "hoh/executor/access/set",
            "hoh/executor/discover",
            "hoh/executor/select"
        ]
    );
    // 47 methods before the executor ones; a new window method must be a deliberate change of this number.
    assert_eq!(bridge::WINDOW_METHODS.len(), 47 + 4);
    for method in ["hoh/executor/access/get", "hoh/executor/discover"] {
        assert!(!bridge::needs_approval(
            method,
            &json!({"executor":"claude"})
        ));
    }
}
