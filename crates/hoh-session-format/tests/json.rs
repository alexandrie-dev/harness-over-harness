//! Port of `tests/json.spec.ts`.

use hoh_session_format::{
    SessionFormatHeader, inspect_session_format_version, session_format_count,
    session_format_safe_integer, snapshot_session_format_header, snapshot_session_format_json,
    snapshot_session_format_json_labeled,
};
use serde_json::{Number, Value, json};

fn header_of(value: Value) -> SessionFormatHeader {
    value.as_object().cloned().expect("object literal")
}

#[test]
fn refuses_negative_zero_that_json_cannot_preserve() {
    // The other cases of the source (non-finite number, undefined member, sparse array, symbol
    // member, non-enumerable member, array property) cannot be built as a `serde_json::Value`.
    let negative_zero = Value::Number(Number::from_f64(-0.0).expect("finite"));
    let error = snapshot_session_format_json_labeled(&negative_zero, "payload").unwrap_err();
    assert_eq!(error.to_string(), "payload is not lossless JSON");
    let nested = json!({ "value": [negative_zero] });
    let error = snapshot_session_format_json_labeled(&nested, "payload").unwrap_err();
    assert_eq!(error.to_string(), "payload is not lossless JSON");
}

#[test]
fn detaches_and_retains_repeated_non_cyclic_values_and_proto_keys() {
    let shared = json!({ "value": 1 });
    let mut source: Value = serde_json::from_str(r#"{"__proto__":{"safe":true}}"#).expect("json");
    source["values"] = json!([shared.clone(), shared]);

    let snapshot = snapshot_session_format_json(&source).expect("lossless");

    assert_eq!(snapshot, source);
    let keys: Vec<&String> = snapshot.as_object().expect("object").keys().collect();
    assert_eq!(keys, ["__proto__", "values"]);
    assert_eq!(snapshot["__proto__"], json!({ "safe": true }));
}

#[test]
fn refuses_invalid_scalar_coordinates() {
    // Cycles and custom prototypes cannot be expressed in a `serde_json::Value`.
    let error = session_format_count(&json!(-1), "count").unwrap_err();
    assert!(error.to_string().contains("non-negative"));
    let error = session_format_safe_integer(&json!(1.5), "integer").unwrap_err();
    assert!(error.to_string().contains("safe integer"));
    let error = inspect_session_format_version(&json!([])).unwrap_err();
    assert!(error.to_string().contains("header"));
}

#[test]
fn counts_and_integers_follow_javascript_number_rules() {
    assert_eq!(session_format_count(&json!(0), "n").expect("zero"), 0);
    assert_eq!(session_format_count(&json!(3.0), "n").expect("integral"), 3);
    let negative_zero = Value::Number(Number::from_f64(-0.0).expect("finite"));
    assert!(session_format_count(&negative_zero, "n").is_err());
    assert!(session_format_safe_integer(&negative_zero, "n").is_err());
    assert!(session_format_count(&json!(9_007_199_254_740_992_u64), "n").is_err());
    assert_eq!(
        session_format_count(&json!(9_007_199_254_740_991_u64), "n").expect("max safe"),
        9_007_199_254_740_991
    );
    assert_eq!(
        session_format_safe_integer(&json!(-5), "n").expect("neg"),
        -5
    );
    assert!(session_format_safe_integer(&json!(-9_007_199_254_740_992_i64), "n").is_err());
    assert!(session_format_count(&json!("1"), "n").is_err());
    assert!(session_format_count(&Value::Null, "n").is_err());
}

#[test]
fn refuses_a_header_snapshot_with_missing_members() {
    // The `null` header of the source is not representable: a header is a JSON object here.
    let error = snapshot_session_format_header(&header_of(json!({
        "version": 1, "id": "missing-seeded", "createdAt": 1, "delegationDepth": 0,
    })))
    .unwrap_err();
    assert!(error.to_string().contains("isSeeded"));
}

#[test]
fn accepts_a_complete_header_and_keeps_member_order() {
    let header = header_of(json!({
        "version": 1, "id": "a", "createdAt": 1, "isSeeded": false, "delegationDepth": 0,
        "extra": { "z": 1, "a": 2 },
    }));
    let snapshot = snapshot_session_format_header(&header).expect("valid");
    assert_eq!(snapshot, header);
    let keys: Vec<&String> = snapshot.keys().collect();
    assert_eq!(
        keys,
        [
            "version",
            "id",
            "createdAt",
            "isSeeded",
            "delegationDepth",
            "extra"
        ]
    );
}
