use crate::helpers::*;
use hoh_session_format::{SessionFormatEvent, SessionFormatJsonObject};
use serde_json::Value;
fn extensions(
    value: &SessionFormatJsonObject,
    fields: &[&str],
    owner: &str,
    target: &mut SessionFormatJsonObject,
) {
    for (key, value) in value {
        if !fields.contains(&key.as_str()) {
            target.insert(format!("plugin:{owner}:{key}"), value.clone());
        }
    }
}
pub fn lift_tool_result(mut event: SessionFormatEvent) -> Result<SessionFormatEvent> {
    if kind(&event) != "tool/result" {
        return Ok(event);
    }
    let Some(data) = field(&event, "data").as_object() else {
        return Ok(event);
    };
    let Some(message) = field(data, "message")
        .as_object()
        .filter(|m| field(m, "role") == "user")
    else {
        return Ok(event);
    };
    let source = field(message, "source").as_object();
    let call_id = source.and_then(|s| nonempty(s.get("callId")));
    let wrapper = field(message, "content")
        .as_array()
        .filter(|a| a.len() == 1)
        .and_then(|a| a[0].as_object());
    if nonempty(message.get("id")).is_none()
        || !source.is_some_and(|s| field(s, "kind") == "tool")
        || call_id.is_none()
        || !wrapper.is_some_and(|w| {
            field(w, "type") == "tool-result"
                && strict_eq(w.get("toolCallId"), source.and_then(|s| s.get("callId")))
        })
    {
        return Err(error(format!(
            "{} requires exactly one tool-result wrapper matching its tool source",
            subject(&event, 3)
        )));
    }
    // The complete wrapper check above proves both optional values exist.
    let wrapper = wrapper.expect("validated wrapper");
    let call_id = call_id.expect("validated call id");
    let is_error = wrapper.get("isError");
    if is_error.is_some_and(|v| !v.is_boolean()) {
        return Err(error(format!(
            "{} tool-result isError must be boolean",
            subject(&event, 3)
        )));
    }
    let content = field(wrapper, "content").as_array().ok_or_else(|| {
        error(format!(
            "{} tool-result content must be an array",
            subject(&event, 3)
        ))
    })?;
    if content
        .iter()
        .any(|b| b.get("type").is_some_and(|t| t == "tool-result"))
    {
        return Err(refusal(format!(
            "{} contains a nested tool-result unsupported by this converter",
            subject(&event, 3)
        )));
    }
    let mut target = SessionFormatJsonObject::new();
    target.insert("role".into(), "tool".into());
    target.insert("source".into(), field(message, "source").clone());
    target.insert("toolCallId".into(), call_id.into());
    target.insert("content".into(), Value::Array(content.clone()));
    if let Some(value) = is_error {
        target.insert("isError".into(), value.clone());
    }
    target.insert("id".into(), field(message, "id").clone());
    extensions(
        message,
        &["id", "role", "source", "content"],
        "message",
        &mut target,
    );
    extensions(
        wrapper,
        &["type", "toolCallId", "content", "isError"],
        "result",
        &mut target,
    );
    let mut data = data.clone();
    data.insert("message".into(), Value::Object(target));
    event.insert("data".into(), Value::Object(data));
    Ok(event)
}
pub fn assert_v4_tool_result_message(event: &SessionFormatEvent) -> Result {
    if kind(event) != "tool/result" {
        return Ok(());
    }
    let subject = subject(event, 4);
    let data = field(event, "data")
        .as_object()
        .ok_or_else(|| error(format!("{subject} data must be an object")))?;
    let message = field(data, "message")
        .as_object()
        .ok_or_else(|| error(format!("{subject} message must be an object")))?;
    if nonempty(message.get("id")).is_none() {
        return Err(error(format!(
            "{subject} requires a first-class message with a string id"
        )));
    }
    if field(message, "role") != "tool" {
        return Err(error(format!("{subject} requires a tool-role message")));
    }
    let source = field(message, "source").as_object();
    if nonempty(message.get("toolCallId")).is_none()
        || !strict_eq(
            source.and_then(|s| s.get("callId")),
            message.get("toolCallId"),
        )
    {
        return Err(error(format!(
            "{subject} requires toolCallId matching its tool source"
        )));
    }
    if !source.is_some_and(|s| field(s, "kind") == "tool") {
        return Err(error(format!("{subject} requires a tool source")));
    }
    let content = field(message, "content")
        .as_array()
        .ok_or_else(|| error(format!("{subject} requires array content")))?;
    if content
        .iter()
        .any(|b| b.get("type").is_some_and(|t| t == "tool-result"))
    {
        return Err(error(format!(
            "{subject} content must not contain a released tool-result wrapper"
        )));
    }
    if message.get("isError").is_some_and(|v| !v.is_boolean()) {
        return Err(error(format!(
            "{subject} isError must be boolean when present"
        )));
    }
    if data.contains_key("error") && field(message, "isError") != true {
        return Err(error(format!(
            "{subject} carries error metadata for a non-error tool result"
        )));
    }
    Ok(())
}
