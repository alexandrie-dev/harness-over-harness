//! Shared native-host approval verification. See PORT_NOTES.md for canonical bytes.
use crate::{ApprovalOutcome, HostApprovalDecision, HostApprovalRequest};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn key_from_hex(text: &str) -> Result<[u8; 32], String> {
    if !is_hash(text) {
        return Err("approvalKey requires 64 lowercase hex characters".into());
    }
    let mut key = [0; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte =
            u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).map_err(|e| e.to_string())?;
    }
    if key.iter().all(|byte| *byte == 0) {
        return Err("approvalKey must not be all zero".into());
    }
    Ok(key)
}
fn is_hash(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn valid_id(text: &str) -> bool {
    (1..=128).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}
/// Objects recursively sorted by UTF-8 key bytes. Compact JSON, UTF-8, no BOM/LF,
/// separators ',' and ':', serde_json string escapes. Array order is retained.
pub fn canonical_action(action: &Value) -> Vec<u8> {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut keys: Vec<_> = map.keys().collect();
                keys.sort();
                Value::Object(
                    keys.into_iter()
                        .map(|key| (key.clone(), sorted(&map[key])))
                        .collect(),
                )
            }
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_vec(&sorted(action)).expect("JSON values serialize")
}
pub fn action_hash(action: &Value) -> String {
    hex(&Sha256::digest(canonical_action(action)))
}
/// RFC2104 HMAC-SHA256, fixed 32-byte host key.
pub fn hmac(key: &[u8; 32], text: &str) -> String {
    let mut inner_pad = [0x36; 64];
    let mut outer_pad = [0x5c; 64];
    for (i, b) in key.iter().enumerate() {
        inner_pad[i] ^= b;
        outer_pad[i] ^= b;
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(text.as_bytes());
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner.finalize());
    hex(&outer.finalize())
}
/// MAC record has this FIXED key order; unlike action objects it is not sorted.
pub fn mac(key: &[u8; 32], request: &HostApprovalRequest, outcome: &str) -> String {
    let record = json!({"approvalId":request.approval_id,"outcome":outcome,"actionHash":request.action_hash,
        "risk":request.risk,"humanText":request.human_text,"expiresAt":request.expires_at});
    hmac(
        key,
        &serde_json::to_string(&record).expect("MAC record serializes"),
    )
}
pub fn verify(expected: &str, actual: &str) -> bool {
    expected.len() == 64
        && actual.len() == 64
        && expected
            .bytes()
            .zip(actual.bytes())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}
/// `details` is folded text for the window (a raw command, newlines allowed); only its size is bound.
pub const MAX_DETAILS_UNITS: usize = 8192;

pub fn validate_host_request(request: &HostApprovalRequest) -> Result<(), String> {
    validate_human_text(&request.human_text)?;
    if let Some(details) = &request.details
        && !(1..=MAX_DETAILS_UNITS).contains(&details.encode_utf16().count())
    {
        return Err("details requires 1..8192 UTF-16 code units".into());
    }
    if !(1..=9_007_199_254_740_991).contains(&request.expires_at) {
        return Err("expiresAt must be a positive safe integer".into());
    }
    if !valid_id(&request.approval_id) {
        return Err("invalid approvalId".into());
    }
    if !is_hash(&request.action_hash) {
        return Err("invalid actionHash".into());
    }
    let action = request
        .action
        .as_object()
        .ok_or("action must be an object")?;
    if action.len() != 2
        || action
            .get("method")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        || !action.get("params").is_some_and(Value::is_object)
    {
        return Err("action must contain exactly method and params".into());
    }
    if action["method"] == "hoh/model/key/set" {
        let params = action["params"].as_object().expect("validated object");
        if params.len() != 2
            || !params
                .get("keyHash")
                .and_then(Value::as_str)
                .is_some_and(is_hash)
            || !params
                .get("baseUrl")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
        {
            return Err("model key action requires keyHash and baseUrl, never apiKey".into());
        }
    }
    if request.action_hash != action_hash(&request.action) {
        return Err("actionHash mismatch".into());
    }
    Ok(())
}
/// UI bound is in UTF-16 code units, matching the JavaScript dialog.
pub fn validate_human_text(text: &str) -> Result<(), String> {
    if !(1..=4096).contains(&text.encode_utf16().count()) {
        return Err("humanText requires 1..4096 UTF-16 code units".into());
    }
    if text.chars().any(|c| {
        c.is_control()
            || (c.is_whitespace() && c != ' ')
            || matches!(c, '\u{00ad}' | '\u{034f}' | '\u{061c}' | '\u{115f}' | '\u{1160}' |
            '\u{17b4}'..='\u{17b5}' | '\u{180e}' | '\u{200b}'..='\u{200f}' |
            '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{3164}' |
            '\u{fe00}'..='\u{fe0f}' | '\u{feff}' | '\u{ffa0}' |
            '\u{fff9}'..='\u{fffb}' | '\u{1bca0}'..='\u{1bca3}' |
            '\u{1d173}'..='\u{1d17a}' | '\u{e0000}'..='\u{e007f}' |
            '\u{e0100}'..='\u{e01ef}')
    }) {
        return Err("humanText contains invisible formatting or controls".into());
    }
    Ok(())
}
pub fn validate_host_decision(decision: &HostApprovalDecision) -> Result<(), String> {
    if !valid_id(&decision.approval_id) {
        return Err("invalid approvalId".into());
    }
    if !is_hash(&decision.mac) {
        return Err("invalid mac".into());
    }
    if decision.outcome == ApprovalOutcome::Unavailable {
        return Err("unavailable is a log outcome, not host consent".into());
    }
    Ok(())
}
/// The caller must remove the pending request before calling this function:
/// valid, invalid and expired answers consume that request once.
pub fn verify_decision(
    key: &[u8; 32],
    request: &HostApprovalRequest,
    decision: &HostApprovalDecision,
    now: u64,
) -> Result<(), String> {
    if key.iter().all(|byte| *byte == 0) {
        return Err("weak approval key".into());
    }
    validate_host_request(request)?;
    validate_host_decision(decision)?;
    if request.approval_id != decision.approval_id {
        return Err("approvalId mismatch".into());
    }
    if now >= request.expires_at {
        return Err("approval expired".into());
    }
    if request.expires_at - now > 300_000 {
        return Err("approval lifetime exceeds five minutes".into());
    }
    let outcome = serde_json::to_value(&decision.outcome).expect("outcome");
    if !verify(&mac(key, request, outcome.as_str().unwrap()), &decision.mac) {
        return Err("invalid approval mac".into());
    }
    Ok(())
}
