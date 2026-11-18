use crate::{helpers::*, sources::map_event_messages};
use hoh_session_format::{SessionFormatEvent, SessionFormatJsonObject};
use serde_json::Value;
fn source(message: &SessionFormatJsonObject) -> Result {
    if !field(message, "source")
        .as_object()
        .and_then(|v| nonempty(v.get("kind")))
        .is_some_and(|k| k != "plugin")
    {
        return Err(error(
            "format v4 message requires a producer-owned source kind",
        ));
    }
    Ok(())
}
pub fn assert_v4_message_sources(event: &SessionFormatEvent) -> Result {
    map_event_messages(event.clone(), |message| {
        source(&message)?;
        Ok(message)
    })?;
    Ok(())
}
pub fn assert_v4_source_row_admission(row: &Value) -> Result {
    let Some(row) = row.as_object() else {
        return Ok(());
    };
    let Some(data) = field(row, "data").as_object() else {
        return Ok(());
    };
    let messages: Vec<&Value> = match kind(row) {
        "user/message" => {
            if field(data, "source")
                .get("kind")
                .is_some_and(|k| k == "plugin")
            {
                source(data)?;
            }
            return Ok(());
        }
        "system/message" | "assistant/message" | "tool/result" => vec![field(data, "message")],
        "agent/inbox/spliced" => field(data, "inserted")
            .as_array()
            .map(|a| a.iter().collect())
            .unwrap_or_default(),
        "session/title-llm-request" => field(data, "messages")
            .as_array()
            .map(|a| a.iter().collect())
            .unwrap_or_default(),
        _ => vec![],
    };
    for message in messages {
        if let Some(message) = message.as_object()
            && field(message, "source")
                .get("kind")
                .is_some_and(|k| k == "plugin")
        {
            source(message)?;
        }
    }
    Ok(())
}
