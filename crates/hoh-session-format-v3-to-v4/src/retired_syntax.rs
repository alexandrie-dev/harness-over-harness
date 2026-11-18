use crate::helpers::*;
use serde_json::Value;
fn assert_block(block: &Value, subject: &str) -> Result {
    if block.get("type").is_some_and(|t| t == "tool-result") {
        return Err(error(format!(
            "{subject} must not contain a released tool-result wrapper"
        )));
    }
    Ok(())
}
fn assert_content(content: Option<&Value>, subject: &str) -> Result {
    if let Some(blocks) = content.and_then(Value::as_array) {
        for b in blocks {
            assert_block(b, subject)?;
        }
    }
    Ok(())
}
fn assert_message_content(message: Option<&Value>, subject: &str) -> Result {
    if let Some(message) = message.and_then(Value::as_object) {
        assert_content(message.get("content"), subject)?;
    }
    Ok(())
}
pub fn assert_v4_retired_syntax(row: &Value) -> Result {
    let Some(row) = row.as_object() else {
        return Ok(());
    };
    if matches!(kind(row), "tool/code-dispatch-start" | "tool/code-dispatch")
        && field(row, "ignorable") != true
    {
        return Err(refusal(format!(
            "format v4 rejects retired event type {}",
            text(row.get("type"))
        )));
    }
    let data = field(row, "data").as_object();
    if kind(row) == "request/header" {
        let header = data
            .and_then(|d| field(d, "header").as_object())
            .ok_or_else(|| error("format v4 request/header requires data and header objects"))?;
        if header.contains_key("system") {
            return Err(error(
                "format v4 request/header rejects retired header.system",
            ));
        }
        return Ok(());
    }
    let Some(data) = data else {
        return Ok(());
    };
    let subject = format!("{} content", subject(row, 4));
    match kind(row) {
        "user/message" => assert_content(data.get("content"), &subject)?,
        "developer/message" | "assistant/message" | "team/message/queued" => {
            assert_message_content(data.get("message"), &subject)?
        }
        "agent/inbox/spliced" | "session/title-llm-request" => {
            let key = if kind(row) == "agent/inbox/spliced" {
                "inserted"
            } else {
                "messages"
            };
            if let Some(messages) = field(data, key).as_array() {
                for m in messages {
                    assert_message_content(Some(m), &subject)?;
                }
            }
        }
        "compaction/summary" => {
            assert_content(data.get("summary"), &subject)?;
            assert_content(data.get("rawOutput"), &subject)?;
        }
        "tool/ptc-dispatch" => assert_content(data.get("content"), &subject)?,
        _ => {}
    }
    if matches!(kind(row), "assistant/message" | "assistant/attempt")
        && let Some(stream) = field(data, "stream").as_array()
    {
        for entry in stream {
            if !entry.get("type").is_some_and(|t| t == "chunk") {
                continue;
            }
            let Some(chunk) = entry.get("chunk").and_then(Value::as_object) else {
                continue;
            };
            if kind(chunk) == "block-end" {
                assert_block(field(chunk, "block"), &subject)?;
            }
            if kind(chunk) == "block-start" && field(chunk, "blockType") == "tool-result" {
                return Err(error(format!(
                    "{subject} must not contain a released tool-result wrapper"
                )));
            }
        }
    }
    Ok(())
}
