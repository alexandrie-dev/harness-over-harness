use crate::{helpers::*, sources::map_event_messages};
use hoh_session_format::{SessionFormatEvent, SessionFormatJsonObject};
use serde_json::Value;
const V3_BLOCK_TYPES: &[&str] = &[
    "text",
    "reasoning",
    "image",
    "file",
    "tool-call",
    "tool-result",
];
fn migrate_block(mut value: Value, subject: &str) -> Result<Value> {
    let tag = value.get("type").and_then(Value::as_str).ok_or_else(|| {
        error(format!(
            "{subject} requires content blocks with string type tags"
        ))
    })?;
    if !V3_BLOCK_TYPES.contains(&tag) {
        let tag = format!("plugin:{tag}");
        value["type"] = tag.into();
    }
    Ok(value)
}
pub fn migrate_v3_content(value: Option<&Value>, subject: &str) -> Result<Vec<Value>> {
    let blocks = value
        .and_then(Value::as_array)
        .ok_or_else(|| error(format!("{subject} content must be an array")))?;
    blocks
        .iter()
        .enumerate()
        .map(|(i, b)| migrate_block(b.clone(), &format!("{subject}[{i}]")))
        .collect()
}
fn migrate_message(
    mut message: SessionFormatJsonObject,
    subject: &str,
) -> Result<SessionFormatJsonObject> {
    let content = migrate_v3_content(message.get("content"), subject)?;
    message.insert("content".into(), content.into());
    Ok(message)
}
fn migrate_chunk(mut value: Value, subject: &str) -> Result<Value> {
    let Some(chunk) = value.as_object_mut() else {
        return Ok(value);
    };
    match kind(chunk) {
        "block-end" => {
            let block = migrate_block(field(chunk, "block").clone(), &format!("{subject}.block"))?;
            chunk.insert("block".into(), block);
        }
        "block-start" => {
            let tag = field(chunk, "blockType")
                .as_str()
                .ok_or_else(|| error(format!("{subject} blockType must be a string")))?;
            if !V3_BLOCK_TYPES.contains(&tag) {
                let tag = format!("plugin:{tag}");
                chunk.insert("blockType".into(), tag.into());
            }
        }
        _ => {}
    }
    Ok(value)
}
pub fn migrate_v3_event_content(event: SessionFormatEvent) -> Result<SessionFormatEvent> {
    let subject = subject(&event, 3);
    let event_type = kind(&event).to_owned();
    let mut mapped = map_event_messages(event, |m| migrate_message(m, &subject))?;
    let Some(data) = mapped.get_mut("data").and_then(Value::as_object_mut) else {
        return Ok(mapped);
    };
    let mut fields = vec![];
    if event_type == "compaction/summary" {
        fields.push("summary");
        if data.contains_key("rawOutput") {
            fields.push("rawOutput");
        }
    } else if event_type == "tool/ptc-dispatch" {
        fields.push("content");
    } else if event_type == "team/message/queued"
        && let Some(message) = data.get_mut("message").and_then(Value::as_object_mut)
    {
        *message = migrate_message(std::mem::take(message), &subject)?;
    }
    for key in fields {
        let content = migrate_v3_content(data.get(key), &format!("{subject}.{key}"))?;
        data.insert(key.into(), content.into());
    }
    if event_type == "request/header"
        && let Some(tools) = data
            .get("header")
            .and_then(|h| h.get("tools"))
            .and_then(Value::as_array)
    {
        for (i, tool) in tools.iter().enumerate() {
            if tool
                .as_object()
                .is_some_and(|t| t.contains_key("deferLoading"))
            {
                return Err(refusal(format!(
                    "{subject}.header.tools[{i}] contains deferLoading, which is only defined in V4"
                )));
            }
        }
    }
    if matches!(
        event_type.as_str(),
        "assistant/message" | "assistant/attempt"
    ) && let Some(stream) = data.get_mut("stream").and_then(Value::as_array_mut)
    {
        for (i, entry) in stream.iter_mut().enumerate() {
            if entry.get("type").is_some_and(|t| t == "chunk")
                && let Some(chunk) = entry.get_mut("chunk")
            {
                *chunk = migrate_chunk(std::mem::take(chunk), &format!("{subject}.stream[{i}]"))?;
            }
        }
    }
    Ok(mapped)
}
