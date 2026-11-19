use hoh_protocol::{framing::*, *};
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, path::PathBuf, process::Command};
mod support;

#[test]
fn generator_keeps_shared_frame_fields_beside_method_variants() {
    let scratch = std::env::temp_dir().join(format!(
        "hoh-protocol-generator-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&scratch).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(scratch.clone());
    let method = json!({
        "type":"object",
        "properties":{"method":{"const":"ping"},"params":{"type":"object","additionalProperties":false}},
        "required":["method","params"]
    });
    let frame = json!({
        "type":"object",
        "properties":{"jsonrpc":{"const":"2.0"},"id":{"type":["string","number"]}},
        "required":["jsonrpc","id"],
        "oneOf":[method.clone()]
    });
    for (name, schema) in [
        ("RequestFrame", frame),
        ("Request", method.clone()),
        ("Notification", method),
    ] {
        fs::write(scratch.join(format!("{name}.json")), schema.to_string()).unwrap();
    }
    let output = scratch.join("protocol.ts");
    let status = Command::new("node")
        .arg(root().join("scripts/gen-protocol-ts.mjs"))
        .arg(format!("--schema-dir={}", scratch.display()))
        .arg(format!("--output={}", output.display()))
        .status()
        .unwrap();
    assert!(status.success());
    let generated = fs::read_to_string(output).unwrap();
    let frame = generated
        .lines()
        .find(|line| line.starts_with("export type RequestFrame = "))
        .unwrap();
    assert!(frame.contains(r#""jsonrpc": "2.0";"#), "{frame}");
    assert!(frame.contains(r#""id": (string | number);"#), "{frame}");
    assert!(frame.contains(r#""method": "ping";"#), "{frame}");
    assert!(
        frame.contains(" & "),
        "shared envelope and method are both required: {frame}"
    );
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn every_fixture_is_typed_and_round_trips_byte_for_byte() {
    let mut requests = BTreeSet::new();
    let mut results = BTreeSet::new();
    let mut notifications = BTreeSet::new();
    for entry in fs::read_dir(root().join("protocol/fixtures")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let bytes = fs::read_to_string(&path).unwrap();
        let value: Value = serde_json::from_str(&bytes).unwrap();
        let encoded = if path.file_name().unwrap() == "host-approval.request.json" {
            let typed: HostApprovalRequest = serde_json::from_str(&bytes).unwrap();
            approval::validate_host_request(&typed).unwrap();
            serde_json::to_string(&typed).unwrap()
        } else if path.file_name().unwrap() == "host-approval.response.json" {
            let typed: HostApprovalDecision = serde_json::from_str(&bytes).unwrap();
            approval::validate_host_decision(&typed).unwrap();
            serde_json::to_string(&typed).unwrap()
        } else if value.get("method").is_some() && value.get("id").is_some() {
            let wire: Wire<RequestFrame> =
                serde_json::from_str(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let method = wire.typed().request.method();
            validate_params(method, value.get("params").unwrap_or(&json!({}))).unwrap();
            requests.insert(method.to_owned());
            serde_json::to_string(wire.typed()).unwrap()
        } else if let Some(method) = value.get("method").and_then(Value::as_str) {
            let wire: Wire<NotificationFrame> = serde_json::from_str(&bytes).unwrap();
            validate_notification(method, &value["params"]).unwrap();
            notifications.insert(method.to_owned());
            serde_json::to_string(wire.typed()).unwrap()
        } else {
            let wire: Wire<ResponseFrame> = serde_json::from_str(&bytes).unwrap();
            if let Some(result) = value.get("result") {
                let method = value["id"].as_str().unwrap();
                validate_result(method, result)
                    .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                results.insert(method.to_owned());
            }
            let mut typed = serde_json::to_value(wire.typed()).unwrap();
            if let Some(result) = value.get("result") {
                typed["result"] =
                    support::typed_result(value["id"].as_str().unwrap(), result.clone());
            }
            serde_json::to_string(&typed).unwrap()
        };
        assert_eq!(format!("{encoded}\n"), bytes, "{}", path.display());
    }
    let expected: BTreeSet<_> = REQUEST_METHODS.iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(requests, expected);
    assert_eq!(results, expected);
    assert_eq!(
        notifications,
        NOTIFICATION_METHODS
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    );
}

#[test]
fn schemas_and_generated_types_are_current() {
    let window = fs::read_to_string(root().join("ui/src/protocol.ts")).unwrap();
    for forbidden in [
        "HostApprovalRequest",
        "HostApprovalDecision",
        "approvalKey",
        "initialize",
        "shutdown",
        "approval/request",
        "user-question/ask",
    ] {
        assert!(
            !window.contains(forbidden),
            "native capability leaked into window: {forbidden}"
        );
    }
    let native = fs::read_to_string(root().join("protocol/core.ts")).unwrap();
    assert!(native.contains("HostApprovalRequest") && native.contains("approvalKey"));
    let schema_dir = root().join("protocol/schema");
    let expected = schemas();
    let actual: BTreeSet<_> = fs::read_dir(&schema_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(
        actual,
        expected
            .iter()
            .map(|(name, _)| format!("{name}.json"))
            .collect()
    );
    for (name, schema) in expected {
        assert!(
            fs::read_to_string(schema_dir.join(format!("{name}.json"))).unwrap()
                == format!("{}\n", serde_json::to_string_pretty(&schema).unwrap()),
            "stale schema {name}; run hoh-protocol-schema then gen-protocol-ts.mjs"
        );
    }
    let status = Command::new("node")
        .arg(root().join("scripts/gen-protocol-ts.mjs"))
        .arg("--check")
        .status()
        .expect("Node 20 is required for protocol generation");
    assert!(status.success(), "protocol.ts is stale");
}

#[test]
fn sdk_optionality_and_received_order_survive() {
    let source = r#"{"jsonrpc":"2.0","method":"session.query.completed","params":{"sessionId":"s","commandId":"c","finishReason":"completed","finalResponse":"ok"},"extension":{"z":1,"a":2}}"#;
    let wire: Wire<NotificationFrame> = serde_json::from_str(source).unwrap();
    assert_eq!(serde_json::to_string(&wire).unwrap(), source);
    let minimal = json!({"cwd":"/w","provider":"p","model":"m"});
    let typed: InitializeParams = serde_json::from_value(minimal.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), minimal);
    validate_result("approval/request", &json!({"outcome":"rejected"})).unwrap();
    validate_result("user-question/ask", &json!({"cancelled":true})).unwrap();
    assert!(validate_result("user-question/ask", &json!({"cancelled":false})).is_err());
    assert!(validate_result("approval/request", &json!({"outcome":"yes"})).is_err());
    for block in [
        json!({"type":"image","data":"AA==","mimeType":"image/png"}),
        json!({"type":"image","attachment":{"attachmentId":"a","mediaType":"image/png","bytes":1,"width":1,"height":1}}),
        json!({"type":"tool-call","id":"c","name":"f","arguments":"{}"}),
        json!({"type":"file","attachment":{"attachmentId":"a","name":"a.txt","bytes":1}}),
    ] {
        assert!(serde_json::from_value::<SdkPromptContentBlock>(block).is_ok());
    }
}
