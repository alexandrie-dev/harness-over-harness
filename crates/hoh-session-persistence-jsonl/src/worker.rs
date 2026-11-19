//! Native verifier request and response boundary, replacing the Node worker entry.
use crate::{
    Result,
    error::error,
    format::JsonlCompression,
    generation::{JsonlExpectedPrefix, JsonlVerifiedGeneration, verify_jsonl_current_generation},
};
use serde_json::Value;
#[derive(Clone, Debug)]
pub struct VerificationRequest {
    pub path: String,
    pub compression: JsonlCompression,
    pub expected_id: String,
    pub expected_event_count: usize,
    pub expected_prefix: Option<JsonlExpectedPrefix>,
}
pub fn parse_request(value: &Value) -> Result<VerificationRequest> {
    if !value.is_object() {
        return Err(error("migration verifier request must be an object"));
    }
    let malformed = || error("migration verifier request is malformed");
    let path = value["path"].as_str().ok_or_else(malformed)?.to_owned();
    let compression = match value["compression"].as_str() {
        Some("none") => JsonlCompression::None,
        Some("zstd") => JsonlCompression::Zstd,
        _ => return Err(malformed()),
    };
    let expected_id = value["expectedId"]
        .as_str()
        .ok_or_else(malformed)?
        .to_owned();
    let expected_event_count = hoh_session_format::session_format_count(
        &value["expectedEventCount"],
        "expectedEventCount",
    )
    .map_err(|_| malformed())? as usize;
    let expected_prefix = if let Some(prefix) = value.get("expectedPrefix") {
        let bytes = hoh_session_format::session_format_count(&prefix["bytes"], "prefix bytes")
            .map_err(|_| malformed())? as usize;
        let digest = prefix["digest"].as_str().ok_or_else(malformed)?;
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(malformed());
        }
        Some(JsonlExpectedPrefix {
            bytes,
            digest: digest.into(),
        })
    } else {
        None
    };
    Ok(VerificationRequest {
        path,
        compression,
        expected_id,
        expected_event_count,
        expected_prefix,
    })
}
pub async fn verify(request: &VerificationRequest) -> Result<JsonlVerifiedGeneration> {
    verify_jsonl_current_generation(
        &request.path,
        request.compression,
        &request.expected_id,
        request.expected_event_count,
        request.expected_prefix.as_ref(),
    )
    .await
}
