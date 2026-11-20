//! Review round 1: pinned tool-schema check (№3), per-request SDK timeout (№6), clocks without `data` (№7).
use hoh_conformance::{
    compare::restore_pinned_tool_schemas, corpus::Scenario, normalize::Normalizer, protocol::Rpc,
};
use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tokio::process::Command;

fn schemas() -> Value {
    json!([{"name":"read","parameters":{}},{"name":"write","parameters":{}}])
}

#[test]
fn a_pinned_header_accepts_the_token_or_the_ordered_names_and_gets_the_schemas() {
    for tools in [json!("{{tools}}"), json!(["read", "write"])] {
        let mut header = json!({"tools": tools});
        restore_pinned_tool_schemas(&mut header, &schemas()).unwrap();
        assert_eq!(header["tools"], schemas());
    }
    let mut header = json!({"a": 1});
    restore_pinned_tool_schemas(&mut header, &json!([])).unwrap();
    assert_eq!(header, json!({"a": 1}));
}

#[test]
fn a_corrupted_pin_is_an_error() {
    let expected =
        "pinned request header tools must equal {{tools}} or the ordered sidecar tool names";
    for tools in [
        json!(["write", "read"]),
        json!(["read"]),
        json!("other"),
        json!([1, 2]),
        json!(null),
    ] {
        let mut header = json!({"tools": tools});
        let error = restore_pinned_tool_schemas(&mut header, &schemas()).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
    // A pin without `tools` while the sidecar is not empty used to be accepted silently.
    let error = restore_pinned_tool_schemas(&mut json!({}), &schemas()).unwrap_err();
    assert!(error.contains(expected), "{error}");
    assert!(restore_pinned_tool_schemas(&mut json!([]), &schemas()).is_err());
}

#[test]
fn clocks_do_not_add_data_to_rows_without_it() {
    let n = Normalizer::new().unwrap();
    for kind in [
        "job/settled",
        "job/registered",
        "hook/result",
        "goal/change",
        "subagent/catalog",
    ] {
        let mut row = json!({"type": kind, "time": 5});
        n.clocks(&mut row).unwrap();
        assert_eq!(row, json!({"type": kind, "time": 0}), "{kind}");
    }
    let mut row = json!({"type":"job/settled","time":5,"data":{"startedAt":7,"id":"j"}});
    n.clocks(&mut row).unwrap();
    assert_eq!(row["data"], json!({"startedAt":0,"id":"j"}));
}

fn scenario() -> Scenario {
    Scenario {
        suite: "sdk".into(),
        name: "x".into(),
        dir: ".".into(),
        manifest: json!({}),
    }
}

#[tokio::test]
async fn a_request_has_its_own_deadline_that_notifications_do_not_extend() {
    // The peer answers nothing and only emits a notification every 100 ms.
    let mut child = Command::new("sh")
        .args([
            "-c",
            "while :; do echo '{\"jsonrpc\":\"2.0\",\"method\":\"tick\"}'; sleep 0.1; done",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut rpc = Rpc::new(
        &mut child,
        &scenario(),
        &json!({}),
        Duration::from_millis(500),
    )
    .unwrap();
    let started = std::time::Instant::now();
    let error = tokio::time::timeout(Duration::from_secs(10), rpc.request("ping", json!({})))
        .await
        .expect("the request must time out by itself")
        .unwrap_err();
    assert_eq!(error, "protocol response timeout");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn several_requests_are_not_bounded_as_a_whole() {
    // Each answer takes 300 ms; four of them exceed the 1 s per-request limit together.
    let mut child = Command::new("sh")
        .args(["-c", "i=1; while read l; do sleep 0.3; echo \"{\\\"jsonrpc\\\":\\\"2.0\\\",\\\"id\\\":$i,\\\"result\\\":{}}\"; i=$((i+1)); done"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut rpc = Rpc::new(&mut child, &scenario(), &json!({}), Duration::from_secs(1)).unwrap();
    for _ in 0..4 {
        rpc.request("ping", json!({})).await.unwrap();
    }
}

/// Review 213901 №2: a `--v1` selection that narrows to nothing is an error, not "run everything".
#[test]
fn an_empty_v1_selection_is_an_error_not_every_scenario() {
    let list = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/V1-SCENARIOS.tsv"
    );
    let out = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    for narrowing in [["--scenario", "nope"], ["--suite", "acp"]] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_hoh-conformance"))
            .args(["--hoh", env!("CARGO_BIN_EXE_hoh-conformance-fake")])
            .args([
                "--refs",
                "/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness",
            ])
            .args(["--v1", list, "--group", "all", "--out"])
            .arg(out.path())
            .args(narrowing)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{narrowing:?} must fail");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("no scenarios selected"),
            "{narrowing:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Review 220412 №5: a body is never run in place on a current-thread runtime, where it would
/// freeze the timers of the other scenarios; on a multi-thread runtime a slow body leaves them alive.
#[tokio::test]
async fn a_body_is_refused_on_a_current_thread_runtime_without_running() {
    let ran = std::cell::Cell::new(false);
    let result = hoh_conformance::runner::off_the_workers(|| ran.set(true));
    assert!(result.unwrap_err().contains("multi-thread"));
    assert!(!ran.get(), "the body must not run on the only worker");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_slow_body_does_not_stop_the_timers_of_other_scenarios() {
    let ticks = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = ticks.clone();
    let timer = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    });
    hoh_conformance::runner::off_the_workers(|| std::thread::sleep(Duration::from_millis(1000)))
        .unwrap();
    assert!(ticks.load(std::sync::atomic::Ordering::SeqCst) >= 5);
    timer.abort();
}

/// Review 222017 №5: two bodies that block at once on a two-worker runtime still leave the timers of
/// the other scenarios running (`block_in_place` hands the worker's queue over).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bodies_blocking_every_worker_do_not_delay_the_timers_of_other_scenarios() {
    let started = std::time::Instant::now();
    let bodies: Vec<_> = (0..2)
        .map(|_| {
            tokio::spawn(async {
                hoh_conformance::runner::off_the_workers(|| {
                    std::thread::sleep(Duration::from_millis(1500))
                })
                .unwrap()
            })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let timer = std::time::Instant::now();
    tokio::time::timeout(
        Duration::from_millis(300),
        tokio::time::sleep(Duration::from_millis(200)),
    )
    .await
    .expect("the neighbour's 300 ms deadline must fire on time, not after the bodies");
    assert!(timer.elapsed() < Duration::from_millis(600));
    for body in bodies {
        body.await.unwrap();
    }
    assert!(started.elapsed() >= Duration::from_millis(1500));
}
