//! Select and log permanent image omissions in current model-request order.

use hoh_error::BoxError;
use hoh_session::{Session, SessionError, SessionSeq};
use serde_json::{Map, Value, json};

/// Record one decision omitting the oldest retained input-image occurrences.
/// Assistant nodes carry model output and are excluded. Image indexes count
/// every occurrence, including previously offloaded ones, within each message.
///
/// `source_event_seqs` are the input message events in the failed request's order and `count` is
/// the number of additional retained occurrences the adapter needs omitted. Returns whether any
/// occurrence remained to offload.
pub fn offload_oldest_images(
    session: &Session,
    source_event_seqs: &[SessionSeq],
    mut count: u64,
) -> Result<bool, BoxError> {
    let mut targets: Vec<Value> = Vec::new();
    for &seq in source_event_seqs {
        if count == 0 {
            break;
        }
        // Current surface nodes index the durable log; a missing event is a TypeError in the source.
        let event = session.event_at(seq).ok_or_else(|| {
            SessionError::TypeError("Cannot read properties of undefined (reading 'type')".into())
        })?;
        if !matches!(event.event_type(), "user/message" | "tool/result") {
            continue;
        }
        // Both input node types produce a message.
        let message = session.derive_event_message(&event)?.ok_or_else(|| {
            SessionError::TypeError("Cannot read properties of null (reading 'content')".into())
        })?;
        let mut image_indexes: Vec<u64> = Vec::new();
        let mut image_index = 0u64;
        let content = message["content"]
            .as_array()
            .ok_or_else(|| SessionError::TypeError("message.content is not iterable".into()))?;
        for block in content {
            if count == 0 {
                break;
            }
            if block.is_null() {
                return Err(Box::new(SessionError::TypeError(
                    "Cannot read properties of null (reading 'type')".into(),
                )));
            }
            if block.get("type").and_then(Value::as_str) == Some("image") {
                if block.get("offloaded") != Some(&Value::Bool(true)) {
                    image_indexes.push(image_index);
                    count -= 1;
                }
                image_index += 1;
            }
        }
        if !image_indexes.is_empty() {
            let mut target = Map::new();
            target.insert("seq".into(), json!(u64::from(seq)));
            target.insert("imageIndexes".into(), json!(image_indexes));
            targets.push(Value::Object(target));
        }
    }
    if targets.is_empty() {
        return Ok(false);
    }
    session.append("image/offload", json!({ "targets": targets }), None)?;
    Ok(true)
}
