//! A real child process that ignores `shutdown` is stopped within the grace period and leaves no
//! process behind (the engine's whole process group is killed and reaped).
use hoh_desktop::engine::{Engine, HostEvent, ProcessConfig};
use serde_json::json;
use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

fn alive(pid: i32) -> bool {
    // Signal 0 only probes. A zombie that nobody reaped would still count as alive.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[test]
fn an_engine_that_ignores_shutdown_is_killed_after_the_grace_period_without_orphans() {
    let dir = std::env::temp_dir().join(format!("hoh-lifecycle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let pids = dir.join("pids");
    // Answers `initialize`, then ignores TERM and stdin, and keeps a grandchild alive.
    let script = r#"
        read -r line
        id=$(printf '%s' "$line" | sed -E 's/.*"id":"([^"]*)".*/\1/')
        printf '{"jsonrpc":"2.0","id":"%s","result":{"serverInfo":{"name":"fake","version":"1"},"hoh":{"protocolVersion":1,"approvals":"host"}}}\n' "$id"
        trap '' TERM HUP INT
        sleep 600 &
        echo "$$ $!" > "$PIDS"
        while true; do sleep 1; done
    "#;
    let (events, rx) = mpsc::channel();
    let engine = Engine::start(
        Arc::new(ProcessConfig {
            path: "/bin/bash".into(),
            args: vec!["-c".into(), script.into()],
            env: vec![("PIDS".into(), pids.to_str().unwrap().into())],
            stderr_file: None,
            cwd: dir.clone(),
            provider: "mock".into(),
            model: "mock".into(),
        }),
        json!({"cwd":dir,"provider":"mock","model":"mock","capabilities":{"approvals":"host"},
               "hoh":{"protocolVersion":1,"approvalKey":"42".repeat(32)}}),
        Arc::new(move |event| {
            let _ = events.send(event);
        }),
    );
    loop {
        if matches!(rx.recv_timeout(Duration::from_secs(30)).unwrap(), HostEvent::Status(v) if v["state"] == "ready")
        {
            break;
        }
    }
    let until = Instant::now() + Duration::from_secs(10);
    // The file is created before it is filled: wait for both pids.
    while std::fs::read_to_string(&pids).is_ok_and(|text| text.split_whitespace().count() < 2)
        || !pids.exists()
    {
        assert!(Instant::now() < until, "the fake engine did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    let text = std::fs::read_to_string(&pids).unwrap();
    let ids: Vec<i32> = text
        .split_whitespace()
        .map(|p| p.parse().unwrap())
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.iter().all(|pid| alive(*pid)));

    let started = Instant::now();
    engine.stop();
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(400),
        "it did not wait for shutdown: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(2500),
        "stop took {elapsed:?}"
    );
    // The grandchild is adopted by init once its parent is gone; init reaps it a moment later.
    let until = Instant::now() + Duration::from_secs(5);
    for pid in ids {
        while alive(pid) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!alive(pid), "process {pid} outlived the engine");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_engine_that_cannot_be_started_ends_in_a_reported_failure_with_a_reason() {
    let dir = std::env::temp_dir().join(format!("hoh-nostart-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (events, rx) = mpsc::channel();
    let engine = Engine::start(
        Arc::new(ProcessConfig {
            path: dir.join("no-such-engine"),
            args: vec![],
            env: vec![],
            stderr_file: None,
            cwd: dir.clone(),
            provider: "mock".into(),
            model: "mock".into(),
        }),
        json!({"cwd":dir}),
        Arc::new(move |event| {
            let _ = events.send(event);
        }),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    let message = loop {
        assert!(Instant::now() < deadline, "no failure was ever reported");
        if let Ok(HostEvent::Status(v)) = rx.recv_timeout(Duration::from_secs(1))
            && v["state"] == "failed"
        {
            break v["message"].as_str().unwrap().to_owned();
        }
    };
    assert!(message.contains("файл движка не найден"), "{message}");
    assert!(
        !message.contains("no-such-engine"),
        "the path must not reach the window"
    );
    engine.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
