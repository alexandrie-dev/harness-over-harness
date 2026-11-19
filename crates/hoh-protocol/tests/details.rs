//! The optional `details` of a host approval request: absent unless set, outside the action hash
//! and the decision MAC, bounded in size.
use hoh_protocol::{approval::*, *};
use serde_json::Value;

const KEY_HEX: &str = "2b60f8989cf1e8a3ddf60735427531e71bb462960be719332c43f0851c5e5484";

fn request() -> HostApprovalRequest {
    serde_json::from_str(include_str!(
        "../../../protocol/fixtures/host-approval.request.json"
    ))
    .unwrap()
}

#[test]
fn a_request_without_details_serializes_without_the_key_and_old_json_still_parses() {
    let request = request();
    assert_eq!(request.details, None);
    let value = serde_json::to_value(&request).unwrap();
    assert!(value.get("details").is_none(), "{value}");
    assert!(value.get("reason").is_none(), "{value}");
    let again: HostApprovalRequest = serde_json::from_value(value).unwrap();
    assert_eq!(again, request);
}

#[test]
fn details_round_trip_by_that_exact_name_and_stay_outside_hash_and_mac() {
    let plain = request();
    let mut with = plain.clone();
    with.details = Some("Команда: rm old.txt\nвторая строка".into());
    let value = serde_json::to_value(&with).unwrap();
    assert_eq!(value["details"], "Команда: rm old.txt\nвторая строка");
    let back: HostApprovalRequest = serde_json::from_value(value).unwrap();
    assert_eq!(back, with);
    assert!(validate_host_request(&with).is_ok());
    // The hash is of `action` only and the MAC covers id, outcome, hash, risk, text, expiry.
    assert_eq!(with.action_hash, plain.action_hash);
    let key = key_from_hex(KEY_HEX).unwrap();
    assert_eq!(
        mac(&key, &with, "allowed-once"),
        mac(&key, &plain, "allowed-once")
    );
}

#[test]
fn details_must_be_non_empty_and_bounded() {
    let mut request = request();
    request.details = Some(String::new());
    assert!(validate_host_request(&request).is_err());
    request.details = Some("x".repeat(MAX_DETAILS_UNITS));
    assert!(validate_host_request(&request).is_ok());
    request.details = Some("x".repeat(MAX_DETAILS_UNITS + 1));
    assert!(validate_host_request(&request).is_err());
    // An explicit null is not an optional value (the protocol's rule for every optional member).
    let mut value = serde_json::to_value(self::request()).unwrap();
    value["details"] = Value::Null;
    assert!(serde_json::from_value::<HostApprovalRequest>(value).is_err());
}
