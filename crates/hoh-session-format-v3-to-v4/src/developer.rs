use crate::helpers::*;
use hoh_session_format::{SessionFormatEvent, SessionFormatJsonObject};
use serde_json::Value;
fn assert_tool_change(block: &Value, developer: bool) -> Result {
    let Some(block) = block
        .as_object()
        .filter(|b| matches!(kind(b), "tool-addition" | "tool-removal"))
    else {
        return Ok(());
    };
    let tag = kind(block);
    if !developer {
        return Err(error(format!("format v4 {tag} requires developer role")));
    }
    if nonempty(block.get("toolName")).is_none() {
        return Err(error(format!(
            "format v4 {tag} requires a nonempty toolName"
        )));
    }
    if tag == "tool-addition" && block.contains_key("tool") {
        return Err(error(
            "format v4 tool-addition must omit inline tool definitions",
        ));
    }
    Ok(())
}
fn assert_content(value: Option<&Value>, developer: bool) -> Result {
    if let Some(blocks) = value.and_then(Value::as_array) {
        for block in blocks {
            assert_tool_change(block, developer)?;
        }
    }
    Ok(())
}
fn assert_developer_message(message: &SessionFormatJsonObject) -> Result {
    assert_content(message.get("content"), true)?;
    if nonempty(message.get("id")).is_none()
        || !field(message, "content").is_array()
        || !field(message, "source")
            .as_object()
            .and_then(|s| nonempty(s.get("kind")))
            .is_some_and(|k| k != "plugin")
    {
        return Err(error(
            "format v4 developer message requires id, role, content, and a producer-owned source",
        ));
    }
    Ok(())
}
fn assert_ordinary_message(value: Option<&Value>) -> Result {
    if let Some(message) = value.and_then(Value::as_object) {
        if field(message, "role") == "developer" {
            return Err(error(
                "format v4 developer/message and developer role must occur together",
            ));
        }
        assert_content(message.get("content"), false)?;
    }
    Ok(())
}
pub fn assert_v4_developer_data(event: &SessionFormatEvent) -> Result {
    let Some(data) = field(event, "data").as_object() else {
        return if kind(event) == "developer/message" {
            Err(error("format v4 developer/message data must be an object"))
        } else {
            Ok(())
        };
    };
    if kind(event) == "developer/message" {
        let message = field(data, "message")
            .as_object()
            .filter(|m| field(m, "role") == "developer")
            .ok_or_else(|| {
                error("format v4 developer/message requires turn, step, and a developer message")
            })?;
        for key in ["turn", "step"] {
            if count(data.get(key), &format!("developer/message {key}"))? == 0 {
                return Err(error(format!("developer/message {key} must be positive")));
            }
        }
        assert_developer_message(message)?;
        let additions = field(message, "content").as_array().is_some_and(|a| {
            a.iter()
                .any(|b| b.get("type").is_some_and(|t| t == "tool-addition"))
        });
        if additions {
            count(data.get("headerSeq"), "developer/message headerSeq")?;
        } else if data.contains_key("headerSeq") {
            return Err(error(
                "format v4 developer/message must omit headerSeq without tool additions",
            ));
        }
    } else {
        match kind(event) {
            "user/message" => assert_ordinary_message(Some(&Value::Object(data.clone())))?,
            "system/message" | "assistant/message" | "tool/result" => {
                assert_ordinary_message(data.get("message"))?
            }
            "agent/inbox/spliced" | "session/title-llm-request" => {
                let key = if kind(event) == "agent/inbox/spliced" {
                    "inserted"
                } else {
                    "messages"
                };
                if let Some(messages) = field(data, key).as_array() {
                    for message in messages {
                        assert_ordinary_message(Some(message))?;
                    }
                }
            }
            _ => {}
        }
    }
    if kind(event) == "compaction/summary" {
        assert_content(data.get("summary"), false)?;
        assert_content(data.get("rawOutput"), false)?;
    }
    if matches!(kind(event), "assistant/message" | "assistant/attempt")
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
                assert_tool_change(field(chunk, "block"), false)?;
            }
            if kind(chunk) == "block-start"
                && matches!(
                    field(chunk, "blockType").as_str(),
                    Some("tool-addition" | "tool-removal")
                )
            {
                return Err(error("format v4 tool-change blocks require developer role"));
            }
        }
    }
    if kind(event) == "request/header"
        && let Some(tools) = field(data, "header").get("tools").and_then(Value::as_array)
    {
        for tool in tools {
            if let Some(tool) = tool.as_object()
                && tool.contains_key("deferLoading")
                && field(tool, "deferLoading") != true
            {
                return Err(error(
                    "format v4 tool deferLoading must be true when present",
                ));
            }
        }
    }
    Ok(())
}
