//! `harness.spec.ts` 686–1350: the input steps of the ACP client against a scripted agent (`sh`)
//! and pre-seeded durable logs (the TS fake agent persists them on cancel; here they exist up front).
use hoh_conformance::{
    corpus,
    protocol::{self, Rpc},
};
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::process::Command;

/// A scripted agent: initialize / session/new (id `s`) / prompts, `hang` holds until cancel,
/// `boom` fails, anything else streams `thinking about it` and ends.
const AGENT: &str = r#"held=
while read line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id";;
    *'"method":"session/new"'*)
      case "$line" in
        *additionalDirectories*|*rejectall*) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32602,"message":"unsupported workspace scope"}}\n' "$id";;
        *) printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"s"}}\n' "$id";;
      esac;;
    *'"method":"session/prompt"'*)
      case "$line" in
        *hang*) held=$id;;
        *boom*) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32603,"message":"model exploded"}}\n' "$id";;
        *) printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"thinking about it"}}}}\n'
           printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id";;
      esac;;
    *'"method":"session/cancel"'*)
      if [ -n "$held" ]; then printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"cancelled"}}\n' "$held"; held=; fi;;
  esac
done"#;

fn scenario() -> corpus::Scenario {
    corpus::discover(Path::new(
        "/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness",
    ))
    .unwrap()
    .into_iter()
    .find(|s| s.suite == "acp")
    .unwrap()
}

struct World {
    dir: tempfile::TempDir,
}

impl World {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }
    fn cwd(&self) -> &Path {
        self.dir.path()
    }
    /// A persisted log under the sessions root (the root is the directory itself).
    fn log(&self, file: &str, lines: &[Value]) {
        let path = self.dir.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text: String = lines
            .iter()
            .map(|l| format!("{}\n", l.to_string().replace("{{SID}}", "s")))
            .collect();
        std::fs::write(path, text).unwrap();
    }
    fn main_log(&self, rest: &[Value]) {
        let mut lines =
            vec![json!({"type":"session","version":0,"id":"s","createdAt":1,"delegationDepth":0})];
        lines.extend_from_slice(rest);
        self.log("project/main/session.jsonl", &lines);
    }
    async fn run(&self, steps: Value) -> (Result<(), String>, String) {
        self.run_script(AGENT, steps).await
    }
    async fn run_script(&self, script: &str, steps: Value) -> (Result<(), String>, String) {
        let mut child = Command::new("sh")
            .args(["-c", script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut rpc =
            Rpc::new(&mut child, &scenario(), &json!({}), Duration::from_secs(10)).unwrap();
        let result = protocol::acp(
            &mut rpc,
            &json!({"steps": steps}),
            self.cwd(),
            self.cwd(),
            Duration::from_secs(5),
        )
        .await;
        (result, rpc.raw.clone())
    }
}

fn boot() -> Vec<Value> {
    vec![json!({"op":"initialize"}), json!({"op":"newSession"})]
}
fn with(extra: Value) -> Value {
    let mut steps = boot();
    steps.push(extra);
    Value::Array(steps)
}
fn with_all(extra: &[Value]) -> Value {
    let mut steps = boot();
    steps.extend_from_slice(extra);
    Value::Array(steps)
}
fn turn(kind: &str, seq: u32, data: Value) -> Value {
    json!({"type":kind,"seq":seq,"time":1,"data":data})
}
fn error_of(result: Result<(), String>) -> String {
    result.unwrap_err()
}

/// :686
#[tokio::test]
async fn prompt_and_cancel_waits_for_the_durable_turn_start_cancels_and_settles_the_prompt() {
    let w = World::new();
    w.main_log(&[turn("turn/start", 0, json!({"turn":1}))]);
    let (result, raw) = w
        .run(with(json!({"op":"promptAndCancel","text":"hang"})))
        .await;
    result.unwrap();
    assert!(raw.contains("\"stopReason\":\"cancelled\""), "{raw}");
}

/// :697
#[tokio::test]
async fn prompt_and_cancel_can_wait_for_cwd_relative_readiness_before_cancelling() {
    let w = World::new();
    std::fs::write(w.cwd().join("started.txt"), "started").unwrap();
    let step = json!({"op":"promptAndCancel","text":"hang","waitForFile":{"path":"started.txt"}});
    let (result, raw) = w.run(with(step)).await;
    result.unwrap();
    assert!(raw.contains("\"stopReason\":\"cancelled\""));
    let missing = World::new();
    let step = json!({"op":"promptAndCancel","text":"hang","waitForFile":{"path":"never.txt","timeoutMs":20}});
    let error = error_of(missing.run(with(step)).await.0);
    assert!(
        error.contains("workspace file \"never.txt\" did not appear within 20ms"),
        "{error}"
    );
}

/// :728
#[tokio::test]
async fn prompt_and_wait_for_agent_message_keeps_the_app_live_through_a_matching_update() {
    let w = World::new();
    let step =
        json!({"op":"promptAndWaitForAgentMessage","text":"go","waitForText":"thinking about it"});
    let (result, raw) = w.run(with(step)).await;
    result.unwrap();
    assert!(raw.contains("thinking about it"));
}

fn hold(text: &str) -> Value {
    json!({"op":"promptAndCancel","text":text,"waitForFile":{"path":"ready"}})
}
fn ready(w: &World) {
    std::fs::write(w.cwd().join("ready"), "").unwrap();
}

/// :743
#[tokio::test]
async fn wait_for_turn_end_holds_cancellation_open_through_the_persisted_closing_boundary() {
    let w = World::new();
    ready(&w);
    w.main_log(&[turn(
        "turn/end",
        1,
        json!({"turn":1,"reason":{"kind":"aborted"}}),
    )]);
    let (result, _) = w
        .run(with_all(&[hold("hang"), json!({"op":"waitForTurnEnd"})]))
        .await;
    result.unwrap();
}

/// :762
#[tokio::test]
async fn wait_for_inbox_message_holds_the_app_through_a_matching_durable_insertion() {
    let w = World::new();
    ready(&w);
    let spliced = turn(
        "agent/inbox/spliced",
        0,
        json!({"target":"next-turn","start":0,
            "inserted":[{"role":"user","content":[{"type":"text","text":"durable marker"}]}]}),
    );
    w.main_log(&[spliced]);
    let steps = with_all(&[
        hold("hang"),
        json!({"op":"waitForInboxMessage","text":"marker"}),
    ]);
    w.run(steps).await.0.unwrap();
}

/// :790
#[tokio::test]
async fn wait_for_inbox_message_times_out_when_the_log_or_the_matching_insertion_is_absent() {
    let step = json!({"op":"waitForInboxMessage","text":"missing","timeoutMs":20});
    let absent = World::new();
    ready(&absent);
    let error = error_of(absent.run(with_all(&[hold("hang"), step.clone()])).await.0);
    assert!(
        error.contains("did not persist expected inbox message within 20ms"),
        "{error}"
    );
    let unmatched = World::new();
    ready(&unmatched);
    unmatched.main_log(&[]);
    let error = error_of(unmatched.run(with_all(&[hold("hang"), step])).await.0);
    assert!(
        error.contains("did not persist expected inbox message within 20ms"),
        "{error}"
    );
}

/// :811
#[tokio::test]
async fn wait_for_title_after_turn_end_holds_the_app_through_a_standalone_durable_title() {
    let w = World::new();
    ready(&w);
    w.main_log(&[
        turn("turn/end", 1, json!({"turn":1,"reason":{"kind":"aborted"}})),
        turn("session/title", 2, json!({"title":"Late title"})),
    ]);
    w.run(with_all(&[
        hold("hang"),
        json!({"op":"waitForTitleAfterTurnEnd"}),
    ]))
    .await
    .0
    .unwrap();
}

/// :837
#[tokio::test]
async fn wait_for_turn_start_can_require_a_later_durable_turn_before_continuing() {
    let w = World::new();
    ready(&w);
    w.main_log(&[turn("turn/start", 0, json!({"turn":3}))]);
    let steps = with_all(&[
        hold("hang"),
        json!({"op":"waitForTurnStart","minimumTurn":3}),
    ]);
    w.run(steps).await.0.unwrap();
}

/// :862
#[tokio::test]
async fn wait_for_turn_start_rejects_missing_earlier_closed_and_malformed_durable_turns() {
    let missing = World::new();
    let error = error_of(
        missing
            .run(with(json!({"op":"waitForTurnStart","timeoutMs":200})))
            .await
            .0,
    );
    assert!(
        error.contains("did not persist turn/start within 200ms"),
        "{error}"
    );

    let earlier = World::new();
    ready(&earlier);
    earlier.main_log(&[turn("turn/start", 0, json!({"turn":1}))]);
    let step = json!({"op":"waitForTurnStart","minimumTurn":3,"timeoutMs":200});
    let error = error_of(earlier.run(with_all(&[hold("hang"), step])).await.0);
    assert!(
        error.contains("turn/start at or beyond turn 3 within 200ms"),
        "{error}"
    );

    let closed = World::new();
    ready(&closed);
    closed.main_log(&[
        turn("turn/start", 0, json!({"turn":1})),
        turn("turn/end", 1, json!({"turn":1,"reason":{"kind":"stop"}})),
    ]);
    let step = json!({"op":"waitForTurnStart","timeoutMs":200});
    let error = error_of(closed.run(with_all(&[hold("hang"), step])).await.0);
    assert!(
        error.contains("did not persist turn/start within 200ms"),
        "{error}"
    );

    for data in [json!({}), json!({"turn":0})] {
        let malformed = World::new();
        ready(&malformed);
        malformed.main_log(&[turn("turn/start", 0, data)]);
        let steps = with_all(&[hold("hang"), json!({"op":"waitForTurnStart"})]);
        let error = error_of(malformed.run(steps).await.0);
        assert!(
            error.contains("invalid persisted turn/start record"),
            "{error}"
        );
    }
}

/// :939
#[tokio::test]
async fn wait_for_turn_end_times_out_for_a_missing_log_and_an_open_logged_turn() {
    let missing = World::new();
    let error = error_of(
        missing
            .run(with(json!({"op":"waitForTurnEnd","timeoutMs":200})))
            .await
            .0,
    );
    assert!(
        error.contains("did not persist turn/end within 200ms"),
        "{error}"
    );
    let open = World::new();
    ready(&open);
    open.main_log(&[turn("turn/start", 0, json!({"turn":1}))]);
    let steps = with_all(&[hold("hang"), json!({"op":"waitForTurnEnd","timeoutMs":200})]);
    let error = error_of(open.run(steps).await.0);
    assert!(
        error.contains("did not persist turn/end within 200ms"),
        "{error}"
    );
}

/// :969
#[tokio::test]
async fn wait_for_goal_phase_requires_the_requested_durable_goal_phase() {
    let reached = World::new();
    ready(&reached);
    reached.main_log(&[
        turn("goal/change", 1, json!({})),
        turn("goal/change", 2, json!({"goal":{"phase":"active"}})),
    ]);
    let steps = with_all(&[
        hold("hang"),
        json!({"op":"waitForGoalPhase","phase":"active"}),
    ]);
    reached.run(steps).await.0.unwrap();
    let missing = World::new();
    let step = json!({"op":"waitForGoalPhase","phase":"blocked","timeoutMs":20});
    let error = error_of(missing.run(with(step)).await.0);
    assert!(
        error.contains("did not persist goal phase \"blocked\" within 20ms"),
        "{error}"
    );
}

/// :1001 — the deadline names its own wait (here over an empty sessions root; the TS case delays
/// the first `readdir`, which the Rust deadline also covers because it wraps the whole harvest).
#[tokio::test]
async fn every_wait_identifies_itself_when_its_deadline_passes() {
    let rows = [
        (
            json!({"op":"waitForTurnStart","timeoutMs":20}),
            "did not persist turn/start within 20ms",
        ),
        (
            json!({"op":"waitForGoalPhase","phase":"blocked","timeoutMs":20}),
            "did not persist goal phase \"blocked\" within 20ms",
        ),
        (
            json!({"op":"waitForTurnEnd","timeoutMs":20}),
            "did not persist turn/end within 20ms",
        ),
        (
            json!({"op":"waitForInboxMessage","text":"missing","timeoutMs":20}),
            "did not persist expected inbox message within 20ms",
        ),
        (
            json!({"op":"waitForSubagentTurnEnd","child":2,"timeoutMs":20}),
            "subagent child #2 did not persist closed turn 1 within 20ms",
        ),
        (
            json!({"op":"waitForTitleAfterTurnEnd","timeoutMs":20}),
            "did not persist session/title after turn/end within 20ms",
        ),
        (
            json!({"op":"waitForEventAfterTurnEnd","type":"user/message","timeoutMs":20}),
            "did not persist user/message after turn/end within 20ms",
        ),
    ];
    for (step, expected) in rows {
        let w = World::new();
        let error = error_of(w.run(with(step.clone())).await.0);
        assert!(error.contains(expected), "{step}: {error}");
    }
}

fn child_lines(order: &[&str]) -> Vec<Value> {
    let mut lines = vec![
        json!({"type":"session","version":0,"id":"child-1","createdAt":2,"parentSession":"s","delegationDepth":1}),
    ];
    for (seq, kind) in order.iter().enumerate() {
        lines.push(match *kind {
            "descriptor" => turn("subagent/descriptor", seq as u32, json!({})),
            "start" => turn("turn/start", seq as u32, json!({"turn":1})),
            "header" => turn(
                "request/header",
                seq as u32,
                json!({"header":{},"reason":"initial"}),
            ),
            _ => turn(
                "turn/end",
                seq as u32,
                json!({"turn":1,"reason":{"kind":"completed"}}),
            ),
        });
    }
    lines
}

/// :1047
#[tokio::test]
async fn wait_for_subagent_turn_end_requires_a_closed_child_work_turn() {
    let closed = World::new();
    ready(&closed);
    closed.main_log(&[turn(
        "turn/end",
        1,
        json!({"turn":1,"reason":{"kind":"aborted"}}),
    )]);
    closed.log(
        "project/child/session.jsonl",
        &child_lines(&["descriptor", "start", "header", "end"]),
    );
    closed
        .run(with_all(&[
            hold("hang"),
            json!({"op":"waitForSubagentTurnEnd"}),
        ]))
        .await
        .0
        .unwrap();
    let step = json!({"op":"waitForSubagentTurnEnd","minimumTurn":2,"timeoutMs":20});
    let error = error_of(closed.run(with_all(&[hold("hang"), step])).await.0);
    assert!(
        error.contains("subagent child #1 did not persist closed turn 2 within 20ms"),
        "{error}"
    );

    let seed_only = World::new();
    ready(&seed_only);
    seed_only.main_log(&[turn(
        "turn/end",
        1,
        json!({"turn":1,"reason":{"kind":"aborted"}}),
    )]);
    seed_only.log(
        "project/child/session.jsonl",
        &child_lines(&["start", "header", "end", "descriptor"]),
    );
    let step = json!({"op":"waitForSubagentTurnEnd","timeoutMs":20});
    let error = error_of(seed_only.run(with_all(&[hold("hang"), step])).await.0);
    assert!(
        error.contains("subagent child #1 did not persist closed turn 1 within 20ms"),
        "{error}"
    );
}

/// :1135
#[tokio::test]
async fn wait_for_title_after_turn_end_times_out_when_the_title_precedes_the_boundary() {
    let w = World::new();
    ready(&w);
    w.main_log(&[
        turn("session/title", 1, json!({"title":"Early title"})),
        turn("turn/end", 2, json!({"turn":1,"reason":{"kind":"aborted"}})),
    ]);
    let step = json!({"op":"waitForTitleAfterTurnEnd","timeoutMs":20});
    let error = error_of(w.run(with_all(&[hold("hang"), step])).await.0);
    assert!(
        error.contains("did not persist session/title after turn/end within 20ms"),
        "{error}"
    );
}

/// :1161
#[tokio::test]
async fn wait_for_event_after_turn_end_holds_for_a_typed_post_boundary_record_and_times_out_otherwise()
 {
    let user = |seq| {
        turn(
            "user/message",
            seq,
            json!({"content":[{"type":"text","text":"x"}],"source":{"kind":"user"}}),
        )
    };
    let late = World::new();
    ready(&late);
    late.main_log(&[
        turn("turn/end", 1, json!({"turn":1,"reason":{"kind":"aborted"}})),
        user(2),
    ]);
    let step = json!({"op":"waitForEventAfterTurnEnd","type":"user/message"});
    late.run(with_all(&[hold("hang"), step])).await.0.unwrap();
    let early = World::new();
    ready(&early);
    early.main_log(&[
        user(1),
        turn("turn/end", 2, json!({"turn":1,"reason":{"kind":"aborted"}})),
    ]);
    let step = json!({"op":"waitForEventAfterTurnEnd","type":"user/message","timeoutMs":20});
    let error = error_of(early.run(with_all(&[hold("hang"), step])).await.0);
    assert!(
        error.contains("did not persist user/message after turn/end within 20ms"),
        "{error}"
    );
}

/// :1211
#[tokio::test]
async fn prompt_expect_error_swallows_a_model_error_response_as_the_expected_outcome() {
    let w = World::new();
    let (result, raw) = w
        .run(with(json!({"op":"promptExpectError","text":"boom"})))
        .await;
    result.unwrap();
    assert!(raw.contains("model exploded"), "{raw}");
}

/// :1220
#[tokio::test]
async fn prompt_expect_error_throws_when_the_prompt_unexpectedly_succeeds() {
    let w = World::new();
    let error = error_of(
        w.run(with(json!({"op":"promptExpectError","text":"fine"})))
            .await
            .0,
    );
    assert!(error.contains("expected the prompt to fail"), "{error}");
}

/// :1261
#[tokio::test]
async fn new_session_expect_error_swallows_the_rejection_with_and_without_extra_dirs() {
    let w = World::new();
    let steps = json!([{"op":"initialize"},{"op":"newSessionExpectError","additionalDirectories":["/elsewhere"]}]);
    w.run(steps).await.0.unwrap();
    // The agent rejects any session/new naming `rejectall` in its cwd.
    let all = World::new();
    let steps = json!([{"op":"initialize"},{"op":"newSessionExpectError","additionalDirectories":["rejectall"]}]);
    let (result, raw) = all.run(steps).await;
    result.unwrap();
    assert!(raw.contains("unsupported workspace scope"), "{raw}");
}

/// :1279
#[tokio::test]
async fn new_session_expect_error_throws_when_session_new_unexpectedly_succeeds() {
    let w = World::new();
    let error = error_of(
        w.run(json!([{"op":"initialize"},{"op":"newSessionExpectError"}]))
            .await
            .0,
    );
    assert!(
        error.contains("expected session/new to be rejected"),
        "{error}"
    );
}

/// :1287
#[tokio::test]
async fn a_plain_cancel_step_is_forwarded_and_ignored_by_an_idle_agent() {
    let w = World::new();
    w.run(with(json!({"op":"cancel"}))).await.0.unwrap();
}

/// :1296
#[tokio::test]
async fn a_standalone_cancel_can_wait_for_cwd_relative_readiness() {
    let w = World::new();
    ready(&w);
    w.run(with(json!({"op":"cancel","waitForFile":{"path":"ready"}})))
        .await
        .0
        .unwrap();
}

/// :1309
#[tokio::test]
async fn wait_for_file_holds_the_next_input_step_behind_cwd_relative_readiness() {
    let w = World::new();
    ready(&w);
    w.run(with_all(&[
        json!({"op":"waitForFile","path":"ready"}),
        json!({"op":"cancel"}),
    ]))
    .await
    .0
    .unwrap();
}

/// :1322 — twelve rows, one per op.
#[tokio::test]
async fn every_session_step_is_rejected_before_new_session() {
    let rows = [
        json!({"op":"prompt","text":"x"}),
        json!({"op":"promptContent","content":[{"type":"text","text":"x"}]}),
        json!({"op":"promptAndWaitForAgentMessage","text":"x","waitForText":"later"}),
        json!({"op":"promptExpectError","text":"x"}),
        json!({"op":"promptAndCancel","text":"x"}),
        json!({"op":"waitForTurnStart"}),
        json!({"op":"waitForTurnEnd"}),
        json!({"op":"waitForGoalPhase","phase":"active"}),
        json!({"op":"waitForInboxMessage","text":"marker"}),
        json!({"op":"waitForTitleAfterTurnEnd"}),
        json!({"op":"waitForEventAfterTurnEnd","type":"user/message"}),
        json!({"op":"cancel"}),
    ];
    for step in rows {
        let op = step["op"].as_str().unwrap().to_owned();
        let w = World::new();
        let error = error_of(w.run(json!([{"op":"initialize"}, step])).await.0);
        assert!(
            error.contains(&format!("{op} before newSession")),
            "{op}: {error}"
        );
    }
}

/// :1343
#[tokio::test]
async fn an_unknown_input_op_is_rejected() {
    let w = World::new();
    let error = error_of(w.run(json!([{"op":"reticulate"}])).await.0);
    assert!(error.contains("unknown input op"), "{error}");
}

/// :516 — the client answers a permission request `cancelled`; the run's log is harvested.
#[tokio::test]
async fn drives_a_full_turn_with_a_permission_stub_and_harvests_the_log() {
    let w = World::new();
    w.log(
        "project/main/session.jsonl",
        &[
            json!({"type":"session","version":0,"id":"s","createdAt":42,"cwd":w.cwd().to_string_lossy()}),
            turn("turn/start", 1, json!({"turn":1})),
        ],
    );
    let script = r#"while read line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id";;
    *'"method":"session/new"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"s"}}\n' "$id";;
    *'"method":"session/prompt"'*)
      printf '{"jsonrpc":"2.0","id":"p","method":"session/request_permission","params":{"options":[{"kind":"allow_once","optionId":"a"}]}}\n'
      read answer
      case "$answer" in *cancelled*) printf '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"permission:cancelled"}}}}\n';; esac
      printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id";;
  esac
done"#;
    let (result, raw) = w
        .run_script(script, with(json!({"op":"prompt","text":"go"})))
        .await;
    result.unwrap();
    assert!(raw.contains("permission:cancelled"), "{raw}");
    let logs = hoh_conformance::runner::harvest(w.cwd(), "acp").unwrap();
    assert_eq!(logs.len(), 1);
    let header: Value = serde_json::from_str(logs[0].lines().next().unwrap()).unwrap();
    assert_eq!(
        (header["id"].as_str(), header["createdAt"].as_u64()),
        (Some("s"), Some(42))
    );
    assert!(logs[0].contains("turn/start"));
}

/// :547 — structured prompt content reaches the wire block for block.
#[tokio::test]
async fn a_structured_prompt_content_step_is_not_flattened() {
    let w = World::new();
    let script = r#"while read line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id";;
    *'"method":"session/new"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"s"}}\n' "$id";;
    *'"method":"session/prompt"'*)
      case "$line" in *'"text":"before"'*'"type":"image"'*'"mimeType":"image/png"'*'"text":"after"'*) ;; *) exit 9;; esac
      printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id";;
  esac
done"#;
    let step = json!({"op":"promptContent","content":[
        {"type":"text","text":"before"},
        {"type":"image","data":"AQ==","mimeType":"image/png"},
        {"type":"text","text":"after"}]});
    let (result, raw) = w.run_script(script, with(step)).await;
    result.unwrap();
    assert!(raw.contains("\"stopReason\":\"end_turn\""), "{raw}");
}

/// :565 — override and child fixture paths ride the child's environment (children joined by the path delimiter).
#[test]
fn override_and_child_fixture_paths_are_forwarded_into_the_child_env() {
    use std::path::PathBuf;
    let mut s = scenario();
    s.manifest["replay"] = json!({"override": true});
    let files = [
        PathBuf::from("/d/session.jsonl"),
        PathBuf::from("/d/session.1.jsonl"),
        PathBuf::from("/d/session.2.jsonl"),
    ];
    let env = hoh_conformance::runner::environment(
        &s,
        &files,
        Path::new("/w"),
        Path::new("/s"),
        Path::new("/x"),
        &json!({}),
    )
    .unwrap();
    assert_eq!(
        env["DSH_SNAPSHOT_CHILD_FILES"],
        "/d/session.1.jsonl:/d/session.2.jsonl"
    );
    assert_eq!(env["DSH_SNAPSHOT_FILE"], "/d/session.jsonl");
    assert!(env["DSH_SNAPSHOT_OVERRIDE"].ends_with("replay.override.json"));
}

/// :632 — the committed workspace directory is seeded into the temp cwd.
#[test]
fn the_workspace_dir_is_seeded_into_the_temp_cwd_before_the_run() {
    let source = tempfile::tempdir().unwrap();
    std::fs::create_dir(source.path().join("workspace")).unwrap();
    std::fs::write(source.path().join("workspace/seeded.txt"), "hello").unwrap();
    let mut s = scenario();
    s.dir = source.path().to_path_buf();
    let cwd = tempfile::tempdir().unwrap();
    hoh_conformance::workspace::prepare(
        &s,
        cwd.path(),
        Path::new("/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness"),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(cwd.path().join("seeded.txt")).unwrap(),
        "hello"
    );
}
