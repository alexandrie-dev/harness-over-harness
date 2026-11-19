//! The «Подтверждения» section: the person's decisions, read back from the core's journal.
use hoh_desktop::approval::{Begun, Broker};
use hoh_protocol::ApprovalOutcome;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const NOW: u64 = 1_000_000;
const KEY: [u8; 32] = [0x44; 32];

fn waited() -> Instant {
    Instant::now().checked_sub(Duration::from_secs(2)).unwrap()
}

fn bash_request(id: &str, text: &str, reason: &str, now: u64) -> Value {
    let action = json!({"method": "bash", "params": {"command": "rm -f old.txt"}});
    json!({
        "sessionId": "window-main", "toolName": "bash", "approvalId": id, "risk": "high",
        "humanText": text, "reason": reason,
        "details": "curl https://root:hunter3@host.example/?sig=SECRET7 -o out.txt",
        "actionHash": hoh_protocol::approval::action_hash(&action),
        "expiresAt": now + 300_000, "action": action,
    })
}

fn decide_in_window(broker: &Broker, params: Value, outcome: ApprovalOutcome, now: u64) {
    let Begun::Window(open) = broker
        .begin_engine_or_grant(1, json!(7), "approval/request", params, now)
        .unwrap()
    else {
        panic!("window")
    };
    broker.current(&open.label, now, waited()).unwrap();
    broker
        .decide(
            &open.label,
            &open.view.approval_id,
            outcome,
            now,
            Instant::now(),
        )
        .unwrap();
}

#[test]
fn the_section_lists_the_persons_decisions_newest_first_and_survives_a_restart() {
    let path = std::env::temp_dir().join(format!("hoh-history-{}.log", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let broker = Broker::new(KEY).unwrap().with_audit_file(path.clone());

    // a settings action: the person's decision, then the core signs the engine's request on it
    let params = json!({"apiKey": "sk-1", "baseUrl": "http://127.0.0.1:8080/v1"});
    let (open, _receiver) = broker
        .begin_preflight("hoh/model/key/set", &params, NOW)
        .unwrap();
    broker.current(&open.label, NOW, waited()).unwrap();
    broker
        .decide(
            &open.label,
            &open.view.approval_id,
            ApprovalOutcome::AllowedOnce,
            NOW,
            Instant::now(),
        )
        .unwrap();
    broker.arm_in_flight("hoh/model/key/set", &params, 1, "native-1-1", NOW);
    let action = json!({"method": "hoh/model/key/set", "params": {
        "keyHash": hoh_protocol::approval::action_hash(&json!({"apiKey": "sk-1"})),
        "baseUrl": "http://127.0.0.1:8080/v1"}});
    let signed = broker
        .begin_engine_or_grant(1, json!(8), "approval/request", json!({
            "sessionId": "host", "toolName": "hoh/model/key/set", "approvalId": "e1", "risk": "high",
            "humanText": "Сохранить ключ", "expiresAt": NOW + 300_000,
            "actionHash": hoh_protocol::approval::action_hash(&action), "action": action,
        }), NOW + 1)
        .unwrap();
    assert!(matches!(signed, Begun::Signed(_)));

    // an action of the assistant: rejected in the approval window, with an address that holds a secret
    decide_in_window(
        &broker,
        bash_request(
            "e2",
            "Выполнить команду: загрузить https://user:hunter2@host.example/x?token=SECRET9",
            "Команда: curl https://host.example/?api_key=SECRET8",
            NOW + 10,
        ),
        ApprovalOutcome::Rejected,
        NOW + 10,
    );

    // the app is started again: the same file, a new broker
    let again = Broker::new(KEY).unwrap().with_audit_file(path.clone());
    let history = again.history(10);
    assert_eq!(history.len(), 2, "{history:?}");
    assert_eq!(history[0]["asker"], "assistant");
    assert_eq!(history[0]["outcome"], "rejected");
    assert_eq!(history[0]["method"], "bash");
    assert_eq!(history[0]["at"], NOW + 10);
    assert_eq!(history[1]["asker"], "you");
    assert_eq!(history[1]["outcome"], "allowed-once");
    assert_eq!(history[1]["method"], "hoh/model/key/set");
    let shown = serde_json::to_string(&history).unwrap();
    assert_eq!(
        history[0]["details"], "curl https://root:•••@host.example/?sig=••• -o out.txt",
        "the raw command is kept for the folded details, with the secret closed"
    );
    for secret in [
        "hunter2", "hunter3", "SECRET9", "SECRET8", "SECRET7", "sk-1",
    ] {
        assert!(!shown.contains(secret), "{secret} in {shown}");
    }
    assert!(
        shown.contains("user:•••@host.example") && shown.contains("api_key=•••"),
        "{shown}"
    );
    assert_eq!(again.history(1).len(), 1, "the limit applies");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn without_a_journal_file_the_section_is_empty_not_an_error() {
    assert!(Broker::new(KEY).unwrap().history(10).is_empty());
}

fn journal_line(index: usize) -> String {
    format!(
        "{{\"at\":{index},\"who\":\"human\",\"via\":\"approval-window\",\"method\":\"bash\",\"actionHash\":\"{}\",\"humanText\":\"действие n{index}\",\"outcome\":\"rejected\",\"note\":\"отклонено человеком\"}}\n",
        "0".repeat(64)
    )
}

#[test]
fn a_big_journal_is_read_from_its_end_and_only_the_limit_is_taken() {
    use hoh_desktop::approval::read_journal_tail;
    let path = std::env::temp_dir().join(format!("hoh-bigjournal-{}.log", std::process::id()));
    let mut text = String::new();
    for index in 0..12_000 {
        text.push_str(&journal_line(index));
    }
    assert!(text.len() > 2 * 1024 * 1024);
    std::fs::write(&path, &text).unwrap();
    let broker = Broker::new(KEY).unwrap().with_audit_file(path.clone());
    let history = broker.history(200);
    assert_eq!(history.len(), 200);
    assert_eq!(
        history[0]["humanText"], "действие n11999",
        "the newest comes first"
    );
    assert_eq!(history[199]["humanText"], "действие n11800");

    // before any parsing: the lines looked at are limited, and so are the bytes read
    assert_eq!(read_journal_tail(&path, 1 << 20, 5).len(), 5);
    let line = journal_line(0).len();
    let window = read_journal_tail(&path, 64 * 1024, usize::MAX);
    assert!(
        window.len() <= 64 * 1024 / line,
        "{} lines from 64 KiB",
        window.len()
    );
    assert!(
        window.len() + 2 > 64 * 1024 / (line + 9),
        "{} lines, line {line}",
        window.len()
    );
    assert_eq!(window[0]["at"], 11_999);
    assert!(
        window
            .iter()
            .all(|entry| entry["at"].as_u64().unwrap() > 11_000),
        "older lines were not read"
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn a_broken_line_loses_only_itself() {
    let path = std::env::temp_dir().join(format!("hoh-brokenjournal-{}.log", std::process::id()));
    let mut bytes = Vec::new();
    bytes.extend_from_slice(journal_line(1).as_bytes());
    bytes.extend_from_slice(b"\xff\xfe not json at all \xc3(\n"); // not UTF-8, not JSON
    bytes.extend_from_slice(journal_line(2).as_bytes());
    // a good record with one damaged byte inside a string: still readable, lossily
    let mut damaged = journal_line(3).into_bytes();
    let at = damaged
        .windows(12)
        .position(|w| w == "действ".as_bytes())
        .unwrap()
        + 1;
    damaged[at] = 0xff;
    bytes.extend_from_slice(&damaged);
    bytes.extend_from_slice(b"{\"at\":4,\"who\":\"human\"\n"); // cut off
    bytes.extend_from_slice(journal_line(5).as_bytes());
    std::fs::write(&path, bytes).unwrap();
    let history = Broker::new(KEY)
        .unwrap()
        .with_audit_file(path.clone())
        .history(10);
    let times: Vec<u64> = history.iter().map(|e| e["at"].as_u64().unwrap()).collect();
    assert_eq!(times, vec![5, 3, 2, 1], "{history:?}");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn a_window_that_begins_exactly_at_a_line_keeps_that_line() {
    use hoh_desktop::approval::read_journal_tail;
    let path = std::env::temp_dir().join(format!("hoh-aligned-{}.log", std::process::id()));
    // equal-length lines: the file is 100 lines long and every line starts at a multiple of `size`
    let line = |index: usize| journal_line(100_000 + index);
    let size = line(0).len() as u64;
    let text: String = (0..100).map(line).collect();
    std::fs::write(&path, &text).unwrap();
    let at = |entries: &[serde_json::Value]| -> Vec<u64> {
        entries
            .iter()
            .map(|e| e["at"].as_u64().unwrap() - 100_000)
            .collect()
    };

    // the window starts exactly where line 90 starts: lines 90..=99 are whole, none is lost
    let aligned = read_journal_tail(&path, 10 * size, usize::MAX);
    assert_eq!(at(&aligned), (90..100).rev().collect::<Vec<_>>());
    // one byte earlier or later it starts inside a line: that cut line is dropped
    let inside = read_journal_tail(&path, 10 * size + 1, usize::MAX);
    assert_eq!(
        at(&inside),
        (90..100).rev().collect::<Vec<_>>(),
        "line 89 is cut, 90 is whole"
    );
    let later = read_journal_tail(&path, 10 * size - 1, usize::MAX);
    assert_eq!(
        at(&later),
        (91..100).rev().collect::<Vec<_>>(),
        "line 90 is cut and dropped"
    );
    // the whole file: nothing is dropped
    assert_eq!(
        read_journal_tail(&path, u64::MAX / 2, usize::MAX).len(),
        100
    );
    assert_eq!(read_journal_tail(&path, 0, usize::MAX).len(), 0);
    std::fs::remove_file(path).unwrap();
}
