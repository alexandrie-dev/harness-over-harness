use crate::helpers::*;
use serde_json::Value;
pub fn assert_v4_fork_result(row: &Value) -> Result {
    let Some(row) = row.as_object().filter(|r| kind(r) == "tool/result") else {
        return Ok(());
    };
    let Some(data) = field(row, "data").as_object() else {
        return Ok(());
    };
    let Some(err) = field(data, "error")
        .as_object()
        .filter(|e| field(e, "code") == "TOOL_NOT_STARTED")
    else {
        return Ok(());
    };
    let Some(message) = field(data, "message").as_object() else {
        return Ok(());
    };
    let Some(id) = field(message, "id")
        .as_str()
        .filter(|id| id.starts_with("forked-tool-result-"))
    else {
        return Ok(());
    };
    let source = field(message, "source").as_object();
    let call_id = source.and_then(|s| s.get("callId"));
    let prefix = format!("forked-tool-result-{}-", text(call_id));
    let sequence = id.strip_prefix(&prefix).and_then(canonical_suffix);
    let replacement = field(row, "surfaceOp")
        .get("op")
        .is_some_and(|op| op == "replace");
    let text_block = field(message, "content")
        .as_array()
        .filter(|a| a.len() == 1)
        .and_then(|a| a[0].as_object());
    let refs = row.get("sourceEventSeqs");
    if call_id.and_then(Value::as_str).is_none()
        || sequence.is_none()
        || (if replacement {
            row.get("seq")
                .and_then(Value::as_f64)
                .is_none_or(|n| sequence.is_some_and(|s| s as f64 >= n))
        } else {
            !sequence.is_some_and(|n| number_eq(row.get("seq"), n))
        })
        || field(err, "name") != "ToolNotStartedError"
        || (!replacement && (refs.is_some() || field(row, "surfaceOp") != "append"))
        || (replacement
            && !refs
                .and_then(Value::as_array)
                .is_some_and(|a| a.len() == 1 && sequence.is_some_and(|n| number_eq(a.first(), n))))
        || field(message, "role") != "tool"
        || field(message, "isError") != true
        || !strict_eq(message.get("toolCallId"), call_id)
        || !source.is_some_and(|s| field(s, "kind") == "tool")
        || !text_block.is_some_and(|b| field(b, "type") == "text" && field(b, "text").is_string())
    {
        return Err(error("invalid V4 not-started fork result"));
    }
    Ok(())
}
