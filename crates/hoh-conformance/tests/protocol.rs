//! Review 222017 №1, №3, №4: the ACP client against a scripted agent (`sh`), with the texts of
//! `harness.ts`.
use hoh_conformance::{
    corpus,
    protocol::{self, Rpc},
};
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::process::Command;

fn scenario() -> corpus::Scenario {
    corpus::discover(Path::new(
        "/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness",
    ))
    .unwrap()
    .into_iter()
    .find(|s| s.suite == "acp")
    .unwrap()
}

fn agent(script: &str, scripted: Value) -> (tokio::process::Child, Rpc) {
    let mut child = Command::new("sh")
        .args(["-c", script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let rpc = Rpc::new(&mut child, &scenario(), &scripted, Duration::from_secs(10)).unwrap();
    (child, rpc)
}

fn note(n: u32) -> String {
    json!({"jsonrpc":"2.0","method":"session/update","params":{"n":n}}).to_string()
}

/// normalize.spec.ts:252 for the live client: blank lines between frames are not frames.
#[tokio::test]
async fn blank_lines_between_frames_are_ignored() {
    let script = format!("printf '%s\\n\\n   \\n%s\\n' '{}' '{}'", note(1), note(2));
    let (_child, mut rpc) = agent(&script, json!({}));
    rpc.until(|n| n.len() == 2).await.unwrap();
    assert_eq!(rpc.frames.len(), 2);
}

/// harness.ts:495-502: the agent keeps talking while the step waits for its marker.
#[tokio::test]
async fn an_agent_writing_more_than_a_pipe_before_the_marker_is_not_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ready");
    // 3000 lines of ~100 bytes (300 KB) before the marker; the shell can only touch it once all
    // of them were read by the client.
    let script = format!(
        "i=0; while [ $i -lt 3000 ]; do printf '%s\\n' '{}'; i=$((i+1)); done; touch {}; sleep 5",
        json!({"jsonrpc":"2.0","method":"session/update","params":{"pad":"x".repeat(60)}}),
        marker.display()
    );
    let (_child, mut rpc) = agent(&script, json!({}));
    let step = json!({"path": "ready", "timeoutMs": 8000});
    let waited = tokio::time::timeout(
        Duration::from_secs(20),
        rpc.serving(async { protocol::wait_file(dir.path(), &step, Duration::from_secs(8)).await }),
    )
    .await
    .expect("the step must not hang");
    waited.unwrap();
    assert!(rpc.notifications.len() > 2000);
}

async fn run_steps(script: &str, scripted: Value, steps: Value) -> Result<(), String> {
    let dir = tempfile::tempdir().unwrap();
    let (_child, mut rpc) = agent(script, scripted);
    let input = json!({"steps": steps});
    protocol::acp(
        &mut rpc,
        &input,
        dir.path(),
        dir.path(),
        Duration::from_secs(5),
    )
    .await
}

#[tokio::test]
async fn steps_that_need_a_session_fail_at_once_with_the_harness_text() {
    for (op, extra) in [
        ("prompt", json!({"text":"x"})),
        ("promptContent", json!({"content":[]})),
        ("promptAndCancel", json!({"text":"x"})),
        ("waitForTurnEnd", json!({})),
    ] {
        let mut step = json!({"op": op});
        step.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let started = std::time::Instant::now();
        let error = run_steps("sleep 30", json!({}), json!([step]))
            .await
            .unwrap_err();
        assert_eq!(error, format!("snapshot-harness: {op} before newSession"));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{op} waited for a timeout"
        );
    }
}

#[tokio::test]
async fn a_missing_file_names_the_path_and_the_step_limit() {
    let error = run_steps(
        "sleep 30",
        json!({}),
        json!([{"op":"waitForFile","path":"nope","timeoutMs":200}]),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error,
        "snapshot-harness: workspace file \"nope\" did not appear within 200ms"
    );
}

#[tokio::test]
async fn a_new_session_that_should_have_been_rejected_says_so() {
    let script =
        r#"read l; printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"sessionId":"s"}}'; sleep 5"#;
    let error = run_steps(script, json!({}), json!([{"op":"newSessionExpectError"}]))
        .await
        .unwrap_err();
    assert_eq!(
        error,
        "snapshot-harness: expected session/new to be rejected but it succeeded"
    );
}

/// harness.ts:316-319: the agent is answered `cancelled`, the run is rejected after the step.
#[tokio::test]
async fn a_scripted_permission_kind_the_agent_never_offered_is_answered_cancelled_and_rejects_the_run()
 {
    let script = r#"read l
printf '%s\n' '{"jsonrpc":"2.0","id":"p","method":"session/request_permission","params":{"options":[{"kind":"allow_once","optionId":"a"},{"kind":"reject_once","optionId":"r"}]}}'
read answer
case "$answer" in *cancelled*) ;; *) exit 9;; esac
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"sessionId":"s"}}'
sleep 5"#;
    let error = run_steps(
        script,
        json!({"permissionAnswers":[{"kind":"allow_always"}]}),
        json!([{"op":"newSession"}]),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error,
        "snapshot-harness: scripted permission answer allow_always not among the offered options [allow_once, reject_once]"
    );
}

/// Review 222017 №6: a child fixture path holding `:` is an error, not silently no child fixtures.
#[test]
fn a_child_fixture_path_with_a_colon_is_an_error() {
    use std::path::PathBuf;
    let files = [
        PathBuf::from("/a/session.jsonl"),
        PathBuf::from("/b:c/session.1.jsonl"),
    ];
    let error = hoh_conformance::runner::environment(
        &scenario(),
        &files,
        Path::new("/w"),
        Path::new("/s"),
        Path::new("/x"),
        &json!({}),
    )
    .unwrap_err();
    assert!(error.contains("DSH_SNAPSHOT_CHILD_FILES"), "{error}");
}

/// Review 224637 №3: the step's own `timeoutMs` bounds `waitForTurnEnd` too.
#[tokio::test]
async fn wait_for_turn_end_uses_the_timeout_of_its_step() {
    let script =
        r#"read l; printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"sessionId":"s"}}'; sleep 5"#;
    let started = std::time::Instant::now();
    let error = run_steps(
        script,
        json!({}),
        json!([{"op":"newSession"},{"op":"waitForTurnEnd","timeoutMs":50}]),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error,
        "snapshot-harness: session \"s\" did not persist turn/end within 50ms"
    );
    assert!(started.elapsed() < Duration::from_secs(3));
}

/// Review 230043 №2: a permission request that arrives together with the marker is still answered.
#[tokio::test]
async fn a_permission_request_racing_the_marker_is_answered() {
    for round in 0..25 {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ready");
        let script = format!(
            r#"printf '%s\n' '{{"jsonrpc":"2.0","id":"p","method":"session/request_permission","params":{{"options":[{{"kind":"allow_once","optionId":"a"}}]}}}}'
touch {}
read answer
case "$answer" in *selected*) printf '%s\n' '{{"jsonrpc":"2.0","method":"answered","params":{{}}}}';; esac
sleep 5"#,
            marker.display()
        );
        let (_child, mut rpc) = agent(
            &script,
            json!({"permissionAnswers":[{"kind":"allow_once"}]}),
        );
        let step = json!({"path": "ready", "timeoutMs": 8000});
        rpc.serving(protocol::wait_file(
            dir.path(),
            &step,
            Duration::from_secs(8),
        ))
        .await
        .unwrap();
        // The step goes on (the cancel and the prompt response are read by the caller).
        tokio::time::timeout(
            Duration::from_secs(3),
            rpc.until(|n| n.iter().any(|x| x["method"] == "answered")),
        )
        .await
        .unwrap_or_else(|_| panic!("round {round}: the agent never got its answer"))
        .unwrap();
    }
}
