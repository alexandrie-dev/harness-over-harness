use crate::helpers::*;
use hoh_session_format::{SessionFormatEvent, SessionFormatJsonObject};
use serde_json::Value;
/// Transform only the declared first-party message slots.
pub fn map_event_messages(
    mut event: SessionFormatEvent,
    mut transform: impl FnMut(SessionFormatJsonObject) -> Result<SessionFormatJsonObject>,
) -> Result<SessionFormatEvent> {
    let event_type = kind(&event).to_owned();
    let Some(data) = event.get_mut("data").and_then(Value::as_object_mut) else {
        return Ok(event);
    };
    if event_type == "user/message" {
        *data = transform(std::mem::take(data))?;
    } else if matches!(
        event_type.as_str(),
        "developer/message" | "system/message" | "assistant/message" | "tool/result"
    ) {
        let message = data
            .get_mut("message")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| error(format!("{event_type} requires a message")))?;
        *message = transform(std::mem::take(message))?;
    } else if let Some(key) = match event_type.as_str() {
        "agent/inbox/spliced" => Some("inserted"),
        "session/title-llm-request" => Some("messages"),
        _ => None,
    } {
        let messages = data
            .get_mut(key)
            .and_then(Value::as_array_mut)
            .ok_or_else(|| error(format!("{event_type} requires message array")))?;
        for value in messages {
            let message = value
                .as_object_mut()
                .ok_or_else(|| error(format!("{event_type} requires message objects")))?;
            *message = transform(std::mem::take(message))?;
        }
    }
    Ok(event)
}
fn producer_kind(plugin: &str, role: Option<&Value>) -> String {
    match plugin {
        "@deepseek-ai/dsh-system-prompt" if role.is_some_and(|r| r == "system") => {
            "system-prompt".into()
        }
        "@deepseek-ai/dsh-system-prompt" => "runtime-context".into(),
        "compact" => "compact-checkpoint".into(),
        "tools-code-mode" | "tools-ptc" => "ptc-mode".into(),
        "dsh-compaction-basic" => "compact-basic".into(),
        "agent-instructions"
        | "session-reference"
        | "team-message"
        | "goal"
        | "skill-invocation"
        | "skill-catalog"
        | "coordinator"
        | "subagent-report"
        | "subagent-settled"
        | "webhook"
        | "agent-message"
        | "model-selection"
        | "plan-mode"
        | "time-context"
        | "tmux-context"
        | "user-approval"
        | "repeat-tool-reminder"
        | "tool-cordis"
        | "cordis-host-runner"
        | "tool-goal"
        | "tool-jobs"
        | "hooks-codex"
        | "hooks-claude-code"
        | "schedule"
        | "dsh-session-title-llm" => plugin.into(),
        _ => format!("plugin:{plugin}"),
    }
}
pub fn rewrite_plugin_source(
    mut source: SessionFormatJsonObject,
    seq: u64,
    role: Option<&Value>,
) -> Result<SessionFormatJsonObject> {
    let plugin = field(&source, "plugin").as_str().ok_or_else(|| {
        error(format!(
            "plugin source at seq {seq} is not canonical: plugin requires a string"
        ))
    })?;
    let kind = producer_kind(plugin, role);
    if source.len() == 2 {
        let mut result = SessionFormatJsonObject::new();
        result.insert("kind".into(), kind.into());
        return Ok(result);
    }
    source.shift_remove("plugin");
    if source.contains_key("kind") {
        source.insert("kind".into(), kind.into());
    }
    Ok(source)
}
pub fn rewrite_v3_message_source(
    source: SessionFormatJsonObject,
    seq: u64,
    role: Option<&Value>,
) -> Result<SessionFormatJsonObject> {
    let kind = nonempty(source.get("kind")).ok_or_else(|| {
        error(format!(
            "message source at seq {seq} requires a nonempty kind"
        ))
    })?;
    if kind == "plugin" {
        rewrite_plugin_source(source, seq, role)
    } else {
        Ok(source)
    }
}
