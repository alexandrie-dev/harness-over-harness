//! Immutable application of the image occurrences recorded by image/offload.

use crate::ImageOffloadError;
use hoh_error::BoxError;
use hoh_session::SessionError;
use serde_json::Value;

/// Project selected image occurrences to offloaded blocks.
///
/// `message` is the message projected before this decision; `indexes` are the nonempty, strictly
/// increasing depth-first image indexes. The result has the same identity (every other field and
/// block is unchanged) with the selected images marked `offloaded: true`; `message` itself is not
/// touched, so an `Err` leaves nothing partially applied.
///
/// Fails when a selected occurrence is missing or already offloaded.
pub fn offload_message_images(message: &Value, indexes: &[u64]) -> Result<Value, BoxError> {
    let mut message = message.clone();
    let mut image_index = 0u64;
    let mut selected = 0usize;
    // Session message admission proves content is an array before projection.
    let content = message
        .get_mut("content")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            ImageOffloadError::boxed("image/offload: message content must be an array")
        })?;
    for block in content {
        if block.is_null() {
            return Err(Box::new(SessionError::TypeError(
                "Cannot read properties of null (reading 'type')".into(),
            )));
        }
        if block.get("type").and_then(Value::as_str) == Some("image") {
            if indexes.get(selected) == Some(&image_index) {
                if block.get("offloaded") == Some(&Value::Bool(true)) {
                    return Err(ImageOffloadError::boxed(format!(
                        "image/offload: image index {image_index} is already offloaded"
                    )));
                }
                // `{ ...block, offloaded: true }`: an existing key keeps its place, a new one goes last.
                block["offloaded"] = Value::Bool(true);
                selected += 1;
            }
            image_index += 1;
        }
    }
    if selected != indexes.len() {
        return Err(ImageOffloadError::boxed(format!(
            "image/offload: image index {} does not exist",
            indexes[selected]
        )));
    }
    Ok(message)
}
