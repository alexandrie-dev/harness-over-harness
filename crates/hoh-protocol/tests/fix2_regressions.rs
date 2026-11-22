use hoh_protocol::{approval::*, framing::*, transport::*, *};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;
const KEY_HEX: &str = "2b60f8989cf1e8a3ddf60735427531e71bb462960be719332c43f0851c5e5484";
fn request() -> HostApprovalRequest {
    serde_json::from_str(include_str!(
        "../../../protocol/fixtures/host-approval.request.json"
    ))
    .unwrap()
}
#[test]
fn weak_keys_are_rejected_and_all_secret_debug_paths_are_redacted() {
    for bad in [
        "",
        "abcd",
        &"00".repeat(32),
        &"42".repeat(31),
        &"42".repeat(33),
    ] {
        assert!(key_from_hex(bad).is_err());
    }
    key_from_hex(KEY_HEX).unwrap();
    let frame = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "cwd":"/w","provider":"p","model":"m","hoh":{"protocolVersion":1,"approvalKey":KEY_HEX}}});
    let typed: Wire<RequestFrame> = serde_json::from_value(frame.clone()).unwrap();
    let incoming = classify(frame).unwrap();
    assert!(!format!("{typed:?}").contains(KEY_HEX));
    assert!(!format!("{incoming:?}").contains(KEY_HEX));
    let secret = "model-api-key-do-not-log";
    let frame = json!({"jsonrpc":"2.0","id":2,"method":"hoh/model/key/set","params":{"apiKey":secret,"baseUrl":"https://api.example.test/v1"}});
    let typed: Wire<RequestFrame> = serde_json::from_value(frame.clone()).unwrap();
    assert!(!format!("{typed:?}").contains(secret));
    assert!(!format!("{:?}", classify(frame).unwrap()).contains(secret));
    let decision: HostApprovalDecision = serde_json::from_str(include_str!(
        "../../../protocol/fixtures/host-approval.response.json"
    ))
    .unwrap();
    assert!(!format!("{decision:?}").contains(&decision.mac));
    let legacy: ApprovalRequestResult =
        serde_json::from_value(json!({"outcome":"allowed-once","mac":decision.mac})).unwrap();
    assert!(!format!("{legacy:?}").contains(decision.mac.as_str()));
    let response: Wire<ResponseFrame> = serde_json::from_value(json!({
        "jsonrpc":"2.0","id":2,"result":{"apiKey":secret,"mac":decision.mac}}))
    .unwrap();
    assert!(!format!("{response:?}").contains(secret));
    assert!(!format!("{response:?}").contains(decision.mac.as_str()));
    let error: RpcError =
        serde_json::from_value(json!({"code":-32000,"message":"failed","data":{"apiKey":secret}}))
            .unwrap();
    assert!(!format!("{error:?}").contains(secret));
}
#[test]
fn privileged_filter_and_predicate_cannot_drift_and_cover_rights_expansion() {
    let triggering = json!({"decision":"yes","baseUrl":"http://example.test",
        "patch":{"permissions":{},"taskIds":["task-1"]},"security":{},"model":{"baseUrl":"http://example.test"}});
    let listed: BTreeSet<_> = PRIVILEGED_METHODS.iter().copied().collect();
    let triggered: BTreeSet<_> = REQUEST_METHODS
        .iter()
        .copied()
        .filter(|m| requires_host_approval(m, &triggering))
        .collect();
    assert_eq!(listed, triggered);
    assert_eq!(listed.len(), PRIVILEGED_METHODS.len());
    for (method, params) in [
        (
            "session/working-directory/set",
            json!({"sessionId":"s","path":"/tmp"}),
        ),
        (
            "hoh/model/probe",
            json!({"baseUrl":"http://remote","model":"m"}),
        ),
        (
            "hoh/settings/set",
            json!({"model":{"baseUrl":"http://remote","model":"m"}}),
        ),
        (
            "hoh/employee/update",
            json!({"id":"e","rev":1,"patch":{"taskIds":["new-task"]}}),
        ),
        (
            "hoh/model/key/set",
            json!({"apiKey":"fixture-secret","baseUrl":"https://api.example.test/v1"}),
        ),
        (
            "hoh/settings/rules/set",
            json!({"internet":"public","folder":"/work"}),
        ),
        // The access mode of an external harness and the executor of new chats: only the user.
        (
            "hoh/executor/access/set",
            json!({"executor":"claude","mode":"full"}),
        ),
        (
            "hoh/executor/access/set",
            json!({"executor":"claude","mode":"confirm"}),
        ),
        (
            "hoh/executor/select",
            json!({"executor":"claude","model":"default"}),
        ),
        ("hoh/executor/select", json!({"executor":null,"model":"m"})),
    ] {
        assert!(listed.contains(method));
        assert!(requires_host_approval(method, &params), "{method}");
    }
    assert!(!requires_host_approval(
        "hoh/employee/update",
        &json!({"patch":{"role":"Reader"}})
    ));
    assert!(!requires_host_approval(
        "hoh/settings/set",
        &json!({"view":{"theme":"dark","fontSize":16}})
    ));
    // Reading what is chosen and what is installed changes nothing.
    for method in ["hoh/executor/access/get", "hoh/executor/discover"] {
        assert!(!requires_host_approval(
            method,
            &json!({"executor":"claude"})
        ));
    }
}
#[test]
fn dialog_text_and_expiry_admission_prevent_spoofing_and_unbounded_grants() {
    for bad in [
        "".into(),
        "a".repeat(4097),
        "🚀".repeat(2049),
        "a\nb".into(),
        "a\tb".into(),
        "a\u{202e}b".into(),
        "a\u{2066}b".into(),
        "a\u{200b}b".into(),
        "a\u{180b}b".into(),
        "a\u{180f}b".into(),
        "a\u{2800}b".into(),
        "a\u{feff}b".into(),
        "a\u{e0001}b".into(),
    ] {
        let mut req = request();
        req.human_text = bad;
        assert!(validate_host_request(&req).is_err());
    }
    validate_human_text(&"a".repeat(4096)).unwrap();
    validate_human_text(&"🚀".repeat(2048)).unwrap();
    validate_human_text("Применить \"план\" \\ 🚀?").unwrap();
    for bad in [0, 9_007_199_254_740_992] {
        let mut req = request();
        req.expires_at = bad;
        assert!(validate_host_request(&req).is_err());
    }
    let mut bad = serde_json::to_value(request()).unwrap();
    bad["risk"] = json!("extreme");
    assert!(serde_json::from_value::<HostApprovalRequest>(bad).is_err());
    let key = key_from_hex(KEY_HEX).unwrap();
    let mut req = request();
    let now = req.expires_at - 1000;
    req.expires_at = now + 300001;
    let decision = HostApprovalDecision {
        approval_id: req.approval_id.clone(),
        outcome: ApprovalOutcome::AllowedOnce,
        mac: mac(&key, &req, "allowed-once"),
    };
    assert!(verify_decision(&key, &req, &decision, now).is_err());
}
#[test]
fn canonical_sort_is_normatively_utf8_bytes_including_non_bmp_keys() {
    let value = json!({"𐀀":1,"":2});
    assert_eq!(
        String::from_utf8(canonical_action(&value)).unwrap(),
        "{\"\":2,\"𐀀\":1}"
    );
    // UTF-16 code-unit sorting would put the U+10000 surrogate pair before U+E000.
    assert!("𐀀".encode_utf16().collect::<Vec<_>>() < "".encode_utf16().collect::<Vec<_>>());
}
#[test]
fn native_fixture_roundtrip_uses_struct_order_and_detects_a_swapped_type() {
    let bytes = include_str!("../../../protocol/fixtures/host-approval.request.json");
    let req: HostApprovalRequest = serde_json::from_str(bytes).unwrap();
    assert_eq!(format!("{}\n", serde_json::to_string(&req).unwrap()), bytes);
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Swapped<'a> {
        tool_name: &'a str,
        session_id: &'a str,
        #[serde(flatten)]
        rest: serde_json::Map<String, Value>,
    }
    let mut rest = serde_json::to_value(&req)
        .unwrap()
        .as_object()
        .unwrap()
        .clone();
    rest.remove("sessionId");
    rest.remove("toolName");
    let swapped = Swapped {
        tool_name: &req.tool_name,
        session_id: &req.session_id,
        rest,
    };
    assert_ne!(
        format!("{}\n", serde_json::to_string(&swapped).unwrap()),
        bytes
    );
    let answer: HostApprovalDecision = serde_json::from_str(include_str!(
        "../../../protocol/fixtures/host-approval.response.json"
    ))
    .unwrap();
    verify_decision(
        &key_from_hex(KEY_HEX).unwrap(),
        &req,
        &answer,
        req.expires_at - 1000,
    )
    .unwrap();
}
#[test]
fn credential_action_discloses_only_the_key_hash_and_new_results_are_empty() {
    let secret = SecretString::from("fixture-api-key");
    let key_hash = action_hash(&json!({"apiKey":secret.expose()}));
    let action = json!({"method":"hoh/model/key/set","params":{"keyHash":key_hash,"baseUrl":"https://api.example.test/v1"}});
    let mut req = request();
    req.action = action;
    req.action_hash = action_hash(&req.action);
    validate_host_request(&req).unwrap();
    assert!(
        !serde_json::to_string(&req)
            .unwrap()
            .contains(secret.expose())
    );
    req.action["params"] = json!({"apiKey":secret.expose()});
    req.action_hash = action_hash(&req.action);
    assert!(validate_host_request(&req).is_err());
    for (method, params) in [
        (
            "hoh/model/key/set",
            json!({"apiKey":secret.expose(),"baseUrl":"https://api.example.test/v1"}),
        ),
        (
            "hoh/settings/rules/set",
            json!({"internet":"ask","folder":"/work"}),
        ),
    ] {
        validate_params(method, &params).unwrap();
        validate_result(method, &json!({})).unwrap();
    }
}
