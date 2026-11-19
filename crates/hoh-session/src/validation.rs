use crate::surface::{
    is_surface_eligible_type, nonempty, validate_session_event_data, validate_surface_metadata,
};
use crate::types::{seq_number, signed_number};
use crate::{Result, SESSION_FORMAT_VERSION, SessionError, SessionEvent, SessionId};
use serde_json::Value;
fn header_number(v: &Value) -> Option<u64> {
    signed_number(v).filter(|n| *n >= 0).map(|n| n as u64)
}
pub(crate) fn js_field(v: Option<&Value>) -> String {
    v.map(hoh_util_values::js_string)
        .unwrap_or_else(|| "undefined".into())
}
pub(crate) fn validate_header(id: &SessionId, h: &Value) -> Result<()> {
    let fail = |s: &str| SessionError::message(s);
    if !h.is_object() {
        return Err(fail("session header is not a plain JSON record"));
    }
    if h.get("seedLength").is_some() {
        return Err(fail("session header has invalid field \"seedLength\""));
    }
    if h.get("version").and_then(Value::as_f64) != Some(SESSION_FORMAT_VERSION as f64) {
        return Err(fail(&format!(
            "session header version must be {SESSION_FORMAT_VERSION}, got {}",
            js_field(h.get("version"))
        )));
    }
    if h["id"].as_str() != Some(id.as_str()) {
        return Err(fail(&format!(
            "session header id \"{}\" does not match session id \"{id}\"",
            js_field(h.get("id"))
        )));
    }
    if h.get("createdAt").and_then(header_number).is_none() {
        return Err(fail(
            "session header createdAt must be a non-negative safe integer",
        ));
    }
    if let Some(cwd) = h.get("cwd") {
        let cwd = cwd
            .as_str()
            .ok_or_else(|| fail("session header cwd must be a string"))?;
        if !hoh_util_workspace_path::node_path::is_absolute(cwd) {
            return Err(fail(&format!(
                "session header cwd must be an absolute path, got \"{cwd}\""
            )));
        }
    }
    if h.get("parentSession").is_some_and(|v| !v.is_string()) {
        return Err(fail("session header parentSession must be a string"));
    }
    if !h["isSeeded"].is_boolean() {
        return Err(fail("session header isSeeded must be a boolean"));
    }
    if h.get("origin").is_some_and(|v| v != "subagent") {
        return Err(fail("session header origin must be \"subagent\""));
    }
    if h.get("delegationDepth")
        .is_some_and(|v| header_number(v).is_none())
    {
        return Err(fail(
            "session header delegationDepth must be a non-negative safe integer",
        ));
    }
    if h.get("agentPreset").is_some_and(|v| !v.is_string()) {
        return Err(fail("session header agentPreset must be a string"));
    }
    Ok(())
}
pub(crate) fn assert_message_event_shape(e: &SessionEvent, subject: &str) -> Result<()> {
    if !is_surface_eligible_type(e.event_type()) {
        return Ok(());
    }
    let message = e.message().unwrap_or(&Value::Null);
    let fail = |s: &str| SessionError::message(format!("{subject} {s}"));
    if !message.is_object() || !nonempty(&message["id"]) {
        return Err(fail("lacks an identified message"));
    }
    let role = match e.event_type() {
        "user/message" => "user",
        "developer/message" => "developer",
        "system/message" => "system",
        "assistant/message" => "assistant",
        "tool/result" => "tool",
        _ => return Ok(()),
    };
    if message["role"] != role {
        return Err(fail(&format!("message must have role \"{role}\"")));
    }
    let source = &message["source"];
    if !source.is_object() || !nonempty(&source["kind"]) {
        return Err(fail("message has invalid source"));
    }
    if !message["content"].is_array() {
        return Err(fail("message has invalid content"));
    }
    if role == "system" && source["kind"] != "system-prompt" {
        return Err(fail("message must have system-prompt source"));
    }
    if role == "assistant" && (source["kind"] != "model" || !has_provider_model(source)) {
        return Err(fail("message must have model source"));
    }
    if role == "tool" {
        if source["kind"] != "tool" || !nonempty(&source["callId"]) {
            return Err(fail("message must have tool source"));
        }
        if message["toolCallId"] != source["callId"] {
            return Err(fail("message has mismatched tool call ids"));
        }
    }
    Ok(())
}
fn has_provider_model(v: &Value) -> bool {
    v.is_object() && nonempty(&v["provider"]) && nonempty(&v["model"])
}
pub(crate) fn assert_envelope(e: &SessionEvent, index: usize) -> Result<()> {
    let v = e.as_value();
    let invalid = || {
        SessionError::message(format!(
            "seed event at index {index} has an invalid event envelope"
        ))
    };
    let object = v.as_object().ok_or_else(invalid)?;
    if object.keys().any(|k| {
        !matches!(
            k.as_str(),
            "type" | "seq" | "time" | "data" | "surfaceOp" | "sourceEventSeqs" | "ignorable"
        )
    }) || !v["type"].is_string()
        || seq_number(&v["seq"]).is_none()
        || signed_number(&v["time"]).is_none()
        || !object.contains_key("data")
        || v.get("ignorable").is_some_and(|v| v != true)
    {
        return Err(invalid());
    }
    let subject = format!("seed {} at index {index}", e.event_type());
    validate_session_event_data(e, &subject)?;
    if e.event_type() == "request/header" {
        let header = &e.data()["header"];
        let config = &header["config"];
        if !has_provider_model(config) {
            return Err(SessionError::message(format!(
                "{subject} lacks provider/model"
            )));
        }
        if config.get("reasoningEffort").is_some_and(|v| !nonempty(v)) {
            return Err(SessionError::message(format!(
                "{subject} has an invalid reasoningEffort"
            )));
        }
        if let Some(defaults) = header.get("adapterDefaults") {
            let valid = defaults.as_object().is_some_and(|m| {
                m.iter().all(|(k, v)| {
                    matches!(k.as_str(), "reasoningEffort" | "maxTokens")
                        && v == true
                        && config.get(k).is_some()
                })
            });
            if !valid {
                return Err(SessionError::message(format!(
                    "{subject} has invalid adapterDefaults"
                )));
            }
        }
        if !matches!(
            e.data()["reason"].as_str(),
            Some("initial" | "resume" | "change" | "series")
        ) {
            return Err(SessionError::message(format!(
                "{subject} has an invalid reason"
            )));
        }
        if e.data().get("startsSeries").is_some_and(|v| v != true) {
            return Err(SessionError::message(format!(
                "{subject} has an invalid startsSeries marker"
            )));
        }
    }
    assert_message_event_shape(e, &subject)?;
    if matches!(e.event_type(), "assistant/attempt" | "assistant/message")
        && (seq_number(&e.data()["turn"]).is_none()
            || seq_number(&e.data()["step"]).is_none()
            || !e.data()["stream"].is_array())
    {
        return Err(SessionError::message(format!(
            "{subject} has invalid settlement fields"
        )));
    }
    Ok(())
}
pub fn adopt_session_event(event: SessionEvent) -> Result<SessionEvent> {
    let subject = format!(
        "session event at seq {}",
        js_field(event.as_value().get("seq"))
    );
    validate_session_event_data(&event, &subject)?;
    validate_surface_metadata(&event)?;
    assert_message_event_shape(&event, &subject)?;
    Ok(event)
}
pub fn snapshot_session_event(event: &SessionEvent) -> Result<SessionEvent> {
    adopt_session_event(event.clone())
}
