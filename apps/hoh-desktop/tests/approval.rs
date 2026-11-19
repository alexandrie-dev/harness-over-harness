use hoh_desktop::{
    approval::{Broker, Resolution, now_ms},
    engine::{Engine, HostEvent, ProcessConfig},
};
use hoh_protocol::{ApprovalOutcome, HostApprovalDecision, HostApprovalRequest};
use serde_json::{Value, json};
use std::{
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

fn wait_request(events: &mpsc::Receiver<HostEvent>, secret: &str) -> (u64, Value, String, Value) {
    loop {
        match events.recv_timeout(Duration::from_secs(30)).unwrap() {
            HostEvent::Request {
                generation,
                id,
                method,
                params,
            } => {
                assert!(!serde_json::to_string(&params).unwrap().contains(secret));
                return (generation, id, method, params);
            }
            HostEvent::Notification(frame) => {
                assert!(!serde_json::to_string(&frame).unwrap().contains(secret));
            }
            HostEvent::Status(_) => {}
        }
    }
}

fn wait_ready(engine: &Arc<Engine>) {
    let until = Instant::now() + Duration::from_secs(30);
    while Instant::now() < until {
        if engine.request("hoh/settings/get", json!({})).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("stub did not become ready");
}

#[test]
fn real_stub_rejects_bad_mac_then_accepts_the_native_broker_mac() {
    let path = std::env::var_os("HOH_TEST_ENGINE")
        .expect("set HOH_TEST_ENGINE to the built hoh-window-stub");
    let cwd = std::env::current_dir().unwrap();
    let (events, event_rx) = mpsc::channel();
    let engine = Engine::start(
        Arc::new(ProcessConfig {
            path: path.into(),
            args: vec![],
            cwd: cwd.clone(),
            provider: "mock".into(),
            model: "mock".into(),
        }),
        json!({
            "cwd":cwd, "provider":"mock", "model":"mock",
            "capabilities":{"approvals":"host"},
            "hoh":{"protocolVersion":1,"approvalKey":"42".repeat(32)}
        }),
        Arc::new(move |event| {
            let _ = events.send(event);
        }),
    );
    wait_ready(&engine);

    let rejected_secret = "must-not-survive-invalid-mac";
    let request_engine = engine.clone();
    let rejected = thread::spawn(move || {
        request_engine.request(
            "hoh/model/key/set",
            json!({
                "apiKey":rejected_secret,
                "baseUrl":"https://api.example.test/v1"
            }),
        )
    });
    let (generation, id, method, params) = wait_request(&event_rx, rejected_secret);
    assert_eq!(method, "approval/request");
    let request: HostApprovalRequest = serde_json::from_value(params).unwrap();
    let invalid = HostApprovalDecision {
        approval_id: request.approval_id,
        outcome: ApprovalOutcome::AllowedOnce,
        mac: "00".repeat(32),
    };
    engine
        .respond(
            generation,
            hoh_protocol::framing::success(id, serde_json::to_value(invalid).unwrap()),
        )
        .unwrap();
    assert!(rejected.join().unwrap().is_err());

    wait_ready(&engine);
    let accepted_secret = "accepted-only-after-native-mac";
    let request_engine = engine.clone();
    let accepted = thread::spawn(move || {
        request_engine.request(
            "hoh/model/key/set",
            json!({
                "apiKey":accepted_secret,
                "baseUrl":"https://api.example.test/v1"
            }),
        )
    });
    let (generation, id, method, params) = wait_request(&event_rx, accepted_secret);
    let broker = Broker::new([0x42; 32]).unwrap();
    let open = broker
        .begin_engine(generation, id, &method, params, now_ms())
        .unwrap();
    let approval_id = open.view.approval_id.clone();
    broker
        .current(
            &open.label,
            now_ms(),
            Instant::now().checked_sub(Duration::from_secs(2)).unwrap(),
        )
        .unwrap();
    let outcome = broker
        .decide(
            &open.label,
            &approval_id,
            ApprovalOutcome::AllowedOnce,
            now_ms(),
            Instant::now(),
        )
        .unwrap();
    assert!(outcome.command_result.is_ok());
    match outcome.resolution {
        Resolution::Engine {
            generation,
            id,
            decision: Ok(decision),
        } => engine
            .respond(
                generation,
                hoh_protocol::framing::success(id, serde_json::to_value(decision).unwrap()),
            )
            .unwrap(),
        _ => panic!("native broker did not produce an engine decision"),
    }
    assert_eq!(accepted.join().unwrap().unwrap(), json!({}));
    engine.stop();
}
