use hoh_protocol::{approval::*, framing::RequestFrame, transport::FrameReader, *};
use serde_json::json;
use std::io::{BufReader, Cursor};

fn request() -> HostApprovalRequest {
    let action = json!({"method":"hoh/task/rollback","params":{"version":1,"id":"task-1"}});
    serde_json::from_value(
        json!({"sessionId":"session-1","toolName":"hoh/task/rollback",
        "approvalId":"approval-1","risk":"high","humanText":"Разрешить?",
        "actionHash":action_hash(&action),"expiresAt":1795000300000u64,"action":action}),
    )
    .unwrap()
}
fn decision(request: &HostApprovalRequest) -> HostApprovalDecision {
    HostApprovalDecision {
        approval_id: request.approval_id.clone(),
        outcome: ApprovalOutcome::AllowedOnce,
        mac: mac(&[0; 32], request, "allowed-once"),
    }
}
#[test]
fn independently_pinned_node_crypto_record_and_action_vectors() {
    // Computed once independently with Node's crypto, not this verifier:
    // node -e 'const c=require("node:crypto"); const a={method:"hoh/task/rollback",params:{id:"task-1",version:1}};
    // const actionHash=c.createHash("sha256").update(JSON.stringify(a)).digest("hex");
    // const r={approvalId:"approval-1",outcome:"allowed-once",actionHash,risk:"high",humanText:"Разрешить?",expiresAt:1795000300000};
    // console.log(actionHash,c.createHmac("sha256",Buffer.alloc(32)).update(JSON.stringify(r)).digest("hex"));'
    let request = request();
    assert_eq!(
        request.action_hash,
        "60391614a9f19e32559dbd539714ae262c5906196598afd11b534f77d79d7cb8"
    );
    let decision = decision(&request);
    assert_eq!(
        decision.mac,
        "92f5deff51b364e66a8b24599629c16caea9ba49f3b13dc1ea9a74c576c7e049"
    );
    verify_decision(&[0; 32], &request, &decision, 1795000299999).unwrap();
}
#[test]
fn every_signed_field_identity_hash_expiry_and_new_request_replay_are_checked() {
    let original = request();
    let decision = decision(&original);
    let mut changed = original.clone();
    changed.approval_id = "approval-2".into();
    assert!(verify_decision(&[0; 32], &changed, &decision, 0).is_err());
    let mut changed = original.clone();
    changed.human_text = "Другой текст".into();
    assert!(verify_decision(&[0; 32], &changed, &decision, 0).is_err());
    let mut changed = original.clone();
    changed.risk = Risk::Low;
    assert!(verify_decision(&[0; 32], &changed, &decision, 0).is_err());
    let mut changed = original.clone();
    changed.expires_at += 1;
    assert!(verify_decision(&[0; 32], &changed, &decision, 0).is_err());
    let mut changed = original.clone();
    changed.action["params"]["version"] = json!(2);
    assert!(verify_decision(&[0; 32], &changed, &decision, 0).is_err());
    changed.action_hash = action_hash(&changed.action);
    assert!(verify_decision(&[0; 32], &changed, &decision, 0).is_err());
    assert!(verify_decision(&[0; 32], &original, &decision, original.expires_at).is_err());
    let mut wrong = decision;
    wrong.approval_id = "wrong".into();
    assert!(verify_decision(&[0; 32], &original, &wrong, 0).is_err());
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
