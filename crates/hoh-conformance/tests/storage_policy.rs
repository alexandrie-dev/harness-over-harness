//! `storage-policy.spec.ts` (8 cases) against `storage::assert_session_fixture_storage`.
use hoh_conformance::storage::assert_session_fixture_storage;
use serde_json::{Value, json};
use std::path::Path;

fn system() -> Value {
    json!({"type":"system/message","data":{"message":{"role":"system","content":[{"type":"text","text":"{{system}}"}]}}})
}
fn request() -> Value {
    json!({"type":"request/header","data":{"header":{"tools":"{{tools}}"}}})
}
fn historical_request() -> Value {
    json!({"type":"request/header","data":{"header":{"system":"{{system}}","tools":"{{tools}}"}}})
}

fn fixture(id: u64, version: u64, events: &[Value]) -> String {
    let header = json!({"type":"session","version":version,"id":format!("{{{{session:{id}}}}}"),
        "createdAt":0,"delegationDepth":id-1});
    std::iter::once(&header)
        .chain(events)
        .map(|r| format!("{r}\n"))
        .collect()
}

/// `registerGuard`: the guard result, and the directory must be byte-identical afterwards.
fn guard(child: &[Value], historical: Option<&[Value]>) -> Result<(), String> {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pin");
    std::fs::create_dir(&dir).unwrap();
    let write = |name: &str, text: String| std::fs::write(dir.join(name), text).unwrap();
    write("session.v4.jsonl", fixture(1, 4, &[system(), request()]));
    // Each role selects its highest generation; the older child predates system/message.
    write("session.1.v1.jsonl", fixture(2, 1, &[request()]));
    write("session.1.v4.jsonl", fixture(2, 4, child));
    if let Some(events) = historical {
        let mut text = format!(
            "{}\n",
            json!({"type":"session","id":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","createdAt":0})
        );
        for event in events {
            text.push_str(&format!("{event}\n"));
        }
        write("session.2.jsonl", text);
    }
    let contents = |dir: &Path| -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        names
            .iter()
            .map(|n| std::fs::read_to_string(dir.join(n)).unwrap())
            .collect()
    };
    let before = contents(&dir);
    let result = assert_session_fixture_storage(&dir, "pin");
    assert_eq!(contents(&dir), before, "the guard must not write");
    result
}

#[test]
fn checks_only_the_highest_generation_of_every_parent_and_child_role() {
    guard(&[system(), request()], None).unwrap();
}

#[test]
fn preserves_a_selected_historical_child_without_imposing_current_prompt_or_identity_storage() {
    guard(&[system(), request()], Some(&[historical_request()])).unwrap();
}

#[test]
fn rejects_a_selected_historical_child_with_an_unknown_tool_or_a_noncanonical_cwd() {
    for (event, message) in [
        (
            json!({"type":"tool/result","data":{"error":{"code":"UNKNOWN_TOOL"}}}),
            "contains UNKNOWN_TOOL",
        ),
        (
            json!({"type":"user/message","data":{"text":"/private{{cwd}}/file"}}),
            "carries a non-canonical macOS cwd token",
        ),
    ] {
        let error =
            guard(&[system(), request()], Some(&[historical_request(), event])).unwrap_err();
        assert!(
            error.contains(&format!("pin/session.2.jsonl {message}")),
            "{error}"
        );
    }
}

#[test]
fn rejects_an_unredacted_identity_in_a_selected_current_child_beside_historical_residue() {
    let mut unredacted = system();
    unredacted["data"]["message"]["id"] = json!("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
    let error = guard(&[unredacted, request()], Some(&[historical_request()])).unwrap_err();
    assert!(error.contains("identity redaction fixed point"), "{error}");
}

#[test]
fn rejects_a_selected_child_with_a_missing_prompt_unscrubbed_prompt_or_unscrubbed_tools() {
    let leaked = json!({"type":"system/message","data":{"message":{"role":"system",
        "content":[{"type":"text","text":"leaked child prompt"}]}}});
    let cases: [(Vec<Value>, &str); 3] = [
        (
            vec![request()],
            "has a request/header with no preceding system/message",
        ),
        (
            vec![leaked, request()],
            "carries an unscrubbed system prompt",
        ),
        (
            vec![
                system(),
                json!({"type":"request/header","data":{"header":{"tools":[]}}}),
            ],
            "carries unscrubbed tool schemas",
        ),
    ];
    for (events, message) in cases {
        let error = guard(&events, None).unwrap_err();
        assert!(
            error.contains(&format!("pin/session.1.v4.jsonl {message}")),
            "{error}"
        );
    }
}
