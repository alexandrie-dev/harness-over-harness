use hoh_protocol::{approval::*, framing::RequestFrame, transport::FrameReader, *};
use serde_json::{Value, json};
use std::io::{BufReader, Cursor};

const KEY_HEX: &str = "2b60f8989cf1e8a3ddf60735427531e71bb462960be719332c43f0851c5e5484";
fn key() -> [u8; 32] {
    key_from_hex(KEY_HEX).unwrap()
}
fn request() -> HostApprovalRequest {
    serde_json::from_str(include_str!(
        "../../../protocol/fixtures/host-approval.request.json"
    ))
    .unwrap()
}
fn decision(request: &HostApprovalRequest) -> HostApprovalDecision {
    HostApprovalDecision {
        approval_id: request.approval_id.clone(),
        outcome: ApprovalOutcome::AllowedOnce,
        mac: mac(&key(), request, "allowed-once"),
    }
}
#[test]
fn independently_pinned_random_key_node_vector_covers_escapes_astral_float_and_arrays() {
    // Independent generation, run once and retain the resulting nonzero key:
    // node -e 'const c=require("node:crypto"); const key=c.randomBytes(32); const sort=x=>Array.isArray(x)?x.map(sort):x&&typeof x==="object"?Object.fromEntries(Object.entries(x).sort(([a],[b])=>Buffer.compare(Buffer.from(a),Buffer.from(b))).map(([k,v])=>[k,sort(v)])):x;
    // const fixture=JSON.parse(require("node:fs").readFileSync("protocol/fixtures/host-approval.request.json")); const action=fixture.action;
    // const actionHash=c.createHash("sha256").update(JSON.stringify(sort(action))).digest("hex");
    // const r={approvalId:"vector-random-1",outcome:"allowed-once",actionHash,risk:"high",humanText:fixture.humanText,expiresAt:1795000300000};
    // console.log(key.toString("hex"),actionHash,c.createHmac("sha256",key).update(JSON.stringify(r)).digest("hex"));'
    // Retained key KEY_HEX above; reproduce the fixed vector with Buffer.from(KEY_HEX,"hex").
    let request = request();
    let answer = decision(&request);
    assert_eq!(
        request.action_hash,
        "05eb3cf1d33d211485a5515edec13609965ad1f93c011e7b071801b8fd477e27"
    );
    assert_eq!(
        answer.mac,
        "3176b2f8f5891275b11b2d39f503cc99be2fd1b7e392a7742e508f8bb2627126"
    );
    verify_decision(&key(), &request, &answer, request.expires_at - 1000).unwrap();
    let mut wrong = key();
    wrong[0] ^= 0x80;
    assert!(verify_decision(&wrong, &request, &answer, request.expires_at - 1000).is_err());
}
#[test]
fn every_signed_field_identity_hash_expiry_and_new_request_replay_are_checked() {
    let original = request();
    let decision = decision(&original);
    let mut changed = original.clone();
    changed.approval_id = "approval-2".into();
    assert!(verify_decision(&key(), &changed, &decision, original.expires_at - 1000).is_err());
    let mut changed = original.clone();
    changed.human_text = "Другой текст".into();
    assert!(verify_decision(&key(), &changed, &decision, original.expires_at - 1000).is_err());
    let mut changed = original.clone();
    changed.risk = Risk::Low;
    assert!(verify_decision(&key(), &changed, &decision, original.expires_at - 1000).is_err());
    let mut changed = original.clone();
    changed.expires_at += 1;
    assert!(verify_decision(&key(), &changed, &decision, original.expires_at - 1000).is_err());
    let mut changed = original.clone();
    changed.action["params"]["version"] = json!(2);
    assert!(verify_decision(&key(), &changed, &decision, original.expires_at - 1000).is_err());
    changed.action_hash = action_hash(&changed.action);
    assert!(verify_decision(&key(), &changed, &decision, original.expires_at - 1000).is_err());
    assert!(verify_decision(&key(), &original, &decision, original.expires_at).is_err());
    let mut wrong = decision;
    wrong.approval_id = "wrong".into();
    assert!(verify_decision(&key(), &original, &wrong, original.expires_at - 1000).is_err());
}
#[test]
fn host_mode_requires_host_shapes_and_patterns() {
    let request = request();
    validate_params_host("approval/request", &serde_json::to_value(&request).unwrap()).unwrap();
    validate_result_host(
        "approval/request",
        &serde_json::to_value(decision(&request)).unwrap(),
    )
    .unwrap();
    assert!(validate_result_host("approval/request", &json!({"outcome":"allowed-once"})).is_err());
    assert!(
        validate_params_host("approval/request", &json!({"sessionId":"s","toolName":"t"})).is_err()
    );
    for bad in ["", "x|y", &"a".repeat(129)] {
        let mut changed = request.clone();
        changed.approval_id = bad.into();
        assert!(
            validate_params_host("approval/request", &serde_json::to_value(changed).unwrap())
                .is_err()
        );
    }
    let mut wrong = decision(&request);
    wrong.mac = "A".repeat(64);
    assert!(
        validate_result_host("approval/request", &serde_json::to_value(wrong).unwrap()).is_err()
    );
}
#[test]
fn typed_dsh_no_argument_frames_and_integral_fields() {
    for frame in [
        json!({"jsonrpc":"2.0","id":1,"method":"shutdown","params":{}}),
        json!({"jsonrpc":"2.0","id":1,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","id":1,"method":"hoh/employee/list"}),
    ] {
        let parsed: RequestFrame = serde_json::from_value(frame).unwrap();
        assert!(matches!(
            parsed.request,
            Request::Shutdown | Request::EmployeeList(_)
        ));
    }
    for max in [json!(0), json!(-1), json!(1.5), json!(9007199254740992u64)] {
        assert!(
            validate_params(
                "initialize",
                &json!({"cwd":"/w","provider":"p","model":"m","maxTokens":max})
            )
            .is_err()
        );
    }
    validate_params(
        "initialize",
        &json!({"cwd":"/w","provider":"p","model":"m","maxTokens":1}),
    )
    .unwrap();
    assert!(
        validate_params(
            "session/query",
            &json!({"sessionId":"s","contentBlocks":[],"extra":1})
        )
        .is_err()
    );
    let dimensions: Dimensions = serde_json::from_value(json!({"width":0,"height":1})).unwrap();
    assert_eq!(
        serde_json::to_string(&dimensions).unwrap(),
        r#"{"width":0,"height":1}"#
    );
    assert!(serde_json::from_value::<Dimensions>(json!({"width":1.5,"height":1})).is_err());
    let _: ApprovalOutcome = serde_json::from_value(json!("unavailable")).unwrap();
    let _:InitializeResult=serde_json::from_value(json!({"serverInfo":{"name":"n","version":"v"},"hoh":{"protocolVersion":1,"approvals":"host"}})).unwrap();
}

#[test]
fn session_prompt_interaction_mode_is_a_closed_wire_choice() {
    let base = json!({"sessionId":"s","contentBlocks":[]});
    validate_params("session/prompt", &base).unwrap();
    for mode in ["default", "plan"] {
        let mut params = base.clone();
        params["interactionMode"] = json!(mode);
        validate_params("session/prompt", &params).unwrap();
        let decoded: SessionPromptParams = serde_json::from_value(params).unwrap();
        assert!(decoded.interaction_mode.is_some());
    }
    for invalid in [json!("Plan"), json!("future-mode"), json!(1), Value::Null] {
        let mut params = base.clone();
        params["interactionMode"] = invalid;
        assert!(validate_params("session/prompt", &params).is_err());
        assert!(serde_json::from_value::<SessionPromptParams>(params).is_err());
    }
}

#[test]
fn title_prompt_has_a_protocol_boundary() {
    let accepted = "x".repeat(EXECUTOR_TITLE_PROMPT_MAX_CHARS);
    validate_params(
        "hoh/executor/title/generate",
        &json!({"executor":"claude","prompt":accepted}),
    )
    .unwrap();
    let bound = json!({"executor":"pi","prompt":"Name the work","sessionId":"chat-1"});
    assert!(validate_params("hoh/executor/title/generate", &bound).is_ok());
    for session_id in [Value::Null, json!(1)] {
        let mut bad = bound.clone();
        bad["sessionId"] = session_id;
        assert!(validate_params("hoh/executor/title/generate", &bad).is_err());
    }
    for id in [
        Value::Null,
        json!(""),
        json!("x".repeat(129)),
        json!("auto-1"),
    ] {
        let cancel = json!({"cancellationId":id});
        let mut generate = bound.clone();
        generate["cancellationId"] = id.clone();
        assert_eq!(
            validate_params("hoh/executor/title/generate", &generate).is_ok(),
            id == "auto-1"
        );
        assert_eq!(
            validate_params("hoh/executor/title/cancel", &cancel).is_ok(),
            id == "auto-1"
        );
    }
    let refused = "x".repeat(EXECUTOR_TITLE_PROMPT_MAX_CHARS + 1);
    assert!(
        validate_params(
            "hoh/executor/title/generate",
            &json!({"executor":"claude","prompt":refused})
        )
        .is_err()
    );
}
#[test]
fn network_line_limit_bounds_buffer_and_recovers_stdio_remains_unlimited() {
    let text = format!(
        "{{\"method\":\"{}\"}}\n{{\"method\":\"ok\"}}\n",
        "x".repeat(5000)
    );
    let mut limited =
        FrameReader::with_line_limit(BufReader::with_capacity(7, Cursor::new(&text)), Some(100));
    assert_eq!(
        limited.read().unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
    assert!(limited.read().unwrap().is_some());
    let mut stdio = FrameReader::new(Cursor::new(text));
    assert!(stdio.read().unwrap().is_some());
}
