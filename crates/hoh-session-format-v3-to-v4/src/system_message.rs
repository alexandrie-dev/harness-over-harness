use crate::helpers::*;
use serde_json::Value;
fn record_v4<'a>(
    v: Option<&'a Value>,
    s: &str,
) -> Result<&'a hoh_session_format::SessionFormatJsonObject> {
    record(v, &format!("format v4 {s}"))
}
fn string(v: Option<&Value>, s: &str, nonempty: bool) -> Result {
    if !v
        .and_then(Value::as_str)
        .is_some_and(|v| !nonempty || !v.is_empty())
    {
        return Err(error(format!(
            "format v4 {s} requires a string{}",
            if nonempty { " with content" } else { "" }
        )));
    }
    Ok(())
}
fn positive(v: Option<&Value>, s: &str) -> Result {
    if count(v, s)? == 0 {
        return Err(error(format!("format v4 {s} must be positive")));
    }
    Ok(())
}
fn image(value: Option<&Value>) -> Result {
    let a = record_v4(value, "system image attachment")?;
    string(a.get("attachmentId"), "system image attachmentId", true)?;
    if !matches!(
        field(a, "mediaType").as_str(),
        Some("image/png" | "image/jpeg" | "image/webp" | "image/gif")
    ) {
        return Err(error(
            "format v4 system image requires a supported mediaType",
        ));
    }
    count(a.get("bytes"), "system image bytes")?;
    positive(a.get("width"), "system image width")?;
    positive(a.get("height"), "system image height")?;
    if a.contains_key("name") {
        string(a.get("name"), "system image name", false)?;
    }
    if let Some(original) = a.get("originalDimensions") {
        let o = record_v4(Some(original), "system image originalDimensions")?;
        positive(o.get("width"), "system image original width")?;
        positive(o.get("height"), "system image original height")?;
    }
    Ok(())
}
pub fn assert_v4_system_message_fields(row: &Value) -> Result {
    let Some(row) = row.as_object().filter(|r| kind(r) == "system/message") else {
        return Ok(());
    };
    let data = record_v4(row.get("data"), "system/message data")?;
    positive(data.get("turn"), "system/message turn")?;
    positive(data.get("step"), "system/message step")?;
    let message = record_v4(data.get("message"), "system message")?;
    string(message.get("id"), "system message id", true)?;
    if field(message, "role") != "system" {
        return Err(error("format v4 system message requires system role"));
    }
    let content = field(message, "content")
        .as_array()
        .ok_or_else(|| error("format v4 system message requires content array"))?;
    for value in content {
        let b = record_v4(Some(value), "system content block")?;
        string(b.get("type"), "system content type", true)?;
        match kind(b) {
            "text" | "reasoning" => string(b.get("text"), "system content text", false)?,
            "tool-call" => {
                string(b.get("id"), "system tool-call id", true)?;
                string(b.get("name"), "system tool-call name", true)?;
                string(b.get("arguments"), "system tool-call arguments", false)?;
            }
            "image" => image(b.get("attachment"))?,
            "tool-result" => {
                return Err(error(
                    "format v4 system content rejects retired tool-result wrappers",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}
