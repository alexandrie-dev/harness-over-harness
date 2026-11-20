//! `identity.spec.ts` (4 `it`) against `Normalizer::identities`.
use hoh_conformance::{normalize::Normalizer, records};
use serde_json::{Value, json};

const PARENT: &str = "11111111-1111-4111-8111-111111111111";
const CHILD: &str = "22222222-2222-4222-8222-222222222222";
const MESSAGE: &str = "33333333-3333-4333-8333-333333333333";
const APPROVAL: &str = "44444444-4444-4444-8444-444444444444";
const RUN: &str = "55555555-5555-4555-8555-555555555555";
const OTHER: &str = "66666666-6666-4666-8666-666666666666";
const PROSE: &str = "77777777-7777-4777-8777-777777777777";

/// `redactSessionSnapshotIds`: JSONL texts in, JSONL texts out (a trailing newline is kept as it was).
fn redact(logs: &[String]) -> Vec<String> {
    let mut parsed: Vec<Vec<Value>> = logs.iter().map(|l| records(l).unwrap()).collect();
    Normalizer::new().unwrap().identities(&mut parsed);
    parsed
        .iter()
        .zip(logs)
        .map(|(rows, source)| {
            let mut text = rows
                .iter()
                .map(hoh_util_values::js_json_stringify)
                .collect::<Vec<_>>()
                .join("\n");
            if source.ends_with('\n') {
                text.push('\n');
            }
            text
        })
        .collect()
}

fn lines(rows: &[Value]) -> String {
    rows.iter()
        .map(hoh_util_values::js_json_stringify)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn preserves_feedback_versions_and_target_relationships_without_redacting_unrelated_prose() {
    let source = lines(&[
        json!({"type":"session","id":PARENT}),
        json!({"type":"assistant/message","data":{"message":{"id":MESSAGE,"role":"assistant","content":[],"source":{}}}}),
        json!({"type":"feedback/message-put","data":{"sessionId":PARENT,"item":{"messageId":MESSAGE,"version":APPROVAL,"note":PROSE}}}),
        json!({"type":"feedback/message-put","data":{"sessionId":PARENT,"item":{"messageId":MESSAGE,"version":RUN}}}),
        json!({"type":"feedback/message-delete","data":{"sessionId":PARENT,"messageId":MESSAGE}}),
        json!({"type":"example","data":{"version":PROSE}}),
    ]);
    let output = redact(&[source]).remove(0);
    assert!(output.contains("\"version\":\"{{id:1}}\""));
    assert!(output.contains("\"version\":\"{{id:2}}\""));
    assert_eq!(output.matches("\"messageId\":\"{{message:1}}\"").count(), 3);
    assert!(output.contains(PROSE));
    assert_eq!(redact(std::slice::from_ref(&output)), [output]);
}

#[test]
fn preserves_typed_relationships_across_parent_and_child_logs() {
    let parent = lines(&[
        json!({"type":"session","id":PARENT,"createdAt":1,"cwd":"/tmp/work"}),
        json!({"type":"agent/inbox/spliced","data":{"inserted":[{
            "role":"user",
            "content":[{"type":"text","text":format!("keep unrelated {PROSE}; session {CHILD}")}],
            "source":{"kind":"user"},"id":MESSAGE}]}}),
        json!({"type":"approval/asked","data":{"id":APPROVAL}}),
        json!({"type":"tool-workflow/run-start","data":{"runId":RUN}}),
        json!({"type":"example","data":{"requestId":OTHER,"echoed":OTHER}}),
    ]) + "\n";
    let child = lines(&[
        json!({"type":"session","id":CHILD,"parentSession":PARENT,"createdAt":2,"cwd":"/tmp/work"}),
        json!({"type":"user/message","data":{"role":"user","content":[],"source":{"kind":"user"},"id":MESSAGE}}),
    ]) + "\n";
    let redacted = redact(&[parent, child]);
    assert!(redacted[0].contains("\"id\":\"{{session:1}}\""));
    assert!(redacted[1].contains("\"id\":\"{{session:2}}\""));
    assert!(redacted[1].contains("\"parentSession\":\"{{session:1}}\""));
    assert_eq!(redacted.join("\n").matches("{{message:1}}").count(), 2);
    assert!(redacted[0].contains("\"id\":\"{{approval:1}}\""));
    assert!(redacted[0].contains("\"runId\":\"{{workflow:1}}\""));
    assert!(redacted[0].contains("\"requestId\":\"{{id:1}}\""));
    assert!(redacted[0].contains("\"echoed\":\"{{id:1}}\""));
    assert!(redacted[0].contains(PROSE));
    assert!(redacted[0].contains("session {{session:2}}"));
    assert_eq!(redact(&redacted), redacted);
}

#[test]
fn classifies_semantic_text_plus_command_rpc_and_retry_identity_fields() {
    let semantic = "88888888-8888-4888-8888-888888888888";
    let anonymous = "99999999-9999-4999-8999-999999999999";
    let retry = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let source = lines(&[
        json!({"type":"not-a-session","data":{"value":"plain"}}),
        json!({"type":"example","data":{
            "commandId":"command-7","rpcId":"rpc-9","retryId":retry,"requestId":"stable-readable-id",
            "text":format!("Retain this as message {semantic}. Anonymous user: {anonymous}")}}),
    ]);
    let redacted = redact(&[source]).remove(0);
    assert!(redacted.contains("\"commandId\":\"{{command:1}}\""));
    assert!(redacted.contains("\"rpcId\":\"{{rpc:1}}\""));
    assert!(redacted.contains("\"retryId\":\"{{retry:1}}\""));
    assert!(redacted.contains("as message {{message:1}}"));
    assert!(redacted.contains("Anonymous user: {{id:1}}"));
    assert!(redacted.contains("\"requestId\":\"stable-readable-id\""));
    assert!(!redacted.ends_with('\n'));
}

#[test]
fn keeps_a_canonical_token_first_seen_through_a_generic_id_key() {
    let canonical = "{{message:7}}";
    let next = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    let user = |id: &str| json!({"type":"user/message","data":{"role":"user","content":[],"source":{"kind":"user"},"id":id}});
    let source = lines(&[
        json!({"type":"example","data":{"requestId":canonical}}),
        user(canonical),
        user(next),
    ]) + "\n";
    let redacted = redact(&[source]).remove(0);
    assert_eq!(redacted.matches("{{message:7}}").count(), 2);
    assert!(redacted.contains("\"id\":\"{{message:8}}\""));
    assert!(!redacted.contains("{{id:"));
}
