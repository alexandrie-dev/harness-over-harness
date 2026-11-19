//! packages/sdk/server/src/query-content.ts. Untrusted, merge-extensible routing admission.
use crate::error::type_error;
use hoh_error::BoxError;
use hoh_util_values::js_trim;
use serde_json::{Value, json};

// Protocol's closed ContentBlock enum cannot retain declaration-merged plugin blocks.
// SHIM(hoh-protocol): retain Value ONLY on this extensible input boundary until it has an open block.
pub fn query_content(value: &Value) -> Result<Vec<Value>, BoxError> {
    let Some(values) = value.as_array().filter(|v| !v.is_empty()) else {
        return Err(type_error(
            "session/query requires a non-empty contentBlocks array",
        ));
    };
    let mut blocks = Vec::with_capacity(values.len());
    for value in values {
        let Some(object) = value
            .as_object()
            .filter(|v| v.get("type").is_some_and(Value::is_string))
        else {
            return Err(type_error("SDK content blocks require a string type"));
        };
        let tag = object["type"].as_str().unwrap_or_default();
        if tag == "text" {
            if !object.get("text").is_some_and(Value::is_string)
                || object.keys().any(|k| k != "type" && k != "text")
            {
                return Err(type_error(
                    "SDK text blocks require only type and string text",
                ));
            }
            blocks.push(value.clone());
        } else if tag == "image" && object.contains_key("data") {
            if !object["data"].is_string()
                || !matches!(
                    object.get("mimeType").and_then(Value::as_str),
                    Some("image/png" | "image/jpeg" | "image/webp" | "image/gif")
                )
                || object.get("name").is_some_and(|v| !v.is_string())
                || object
                    .keys()
                    .any(|k| !matches!(k.as_str(), "type" | "data" | "mimeType" | "name"))
            {
                return Err(type_error(
                    "SDK encoded images require string data, a supported mimeType and an optional string name",
                ));
            }
            let mut block =
                json!({"type":"image", "data":object["data"], "mimeType":object["mimeType"]});
            if let Some(name) = object.get("name") {
                block["name"] = name.clone();
            }
            blocks.push(block);
        } else {
            if tag == "image" && !object.get("attachment").is_some_and(Value::is_object) {
                return Err(type_error(
                    "SDK image blocks require encoded data or a durable attachment reference",
                ));
            }
            blocks.push(value.clone());
        }
    }
    if blocks
        .iter()
        .all(|b| b["type"] == "text" && b["text"].as_str().is_some_and(|s| js_trim(s).is_empty()))
    {
        return Err(type_error(
            "session/query requires non-blank text or non-text content",
        ));
    }
    Ok(blocks)
}
/// Index preserves the source's identity check, including two identical text blocks.
pub fn query_command_images(
    blocks: &[Value],
    text_index: usize,
) -> Result<Vec<crate::shim::CommandImageAttachment>, BoxError> {
    blocks.iter().enumerate().filter(|(i, _)| *i != text_index).map(|(_, block)| {
        if block["type"] != "image" || block.get("data").is_none() {
            return Err(type_error("Native SDK commands require one text block and optional encoded image blocks"));
        }
        let image = hoh_attachment::EncodedImageAttachment {
            data: block["data"].as_str().ok_or_else(|| type_error("Native SDK commands require one text block and optional encoded image blocks"))?.to_owned(),
            media_type: serde_json::from_value(block["mimeType"].clone()).map_err(|_| type_error("Native SDK commands require one text block and optional encoded image blocks"))?,
            name: block.get("name").and_then(Value::as_str).map(str::to_owned),
        };
        Ok(crate::shim::CommandImageAttachment { image })
    }).collect()
}
