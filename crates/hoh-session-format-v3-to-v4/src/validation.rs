use crate::{
    developer::assert_v4_developer_data, facts::catalog_fact, fork_result::assert_v4_fork_result,
    helpers::*, message_sources::assert_v4_message_sources,
    relationships::assert_v4_lifecycle_relationships, retired_syntax::assert_v4_retired_syntax,
    system_message::assert_v4_system_message_fields, tool_role::assert_v4_tool_result_message,
};
use hoh_session_format::{
    SessionFormatArtifact, SessionFormatEvent, SessionFormatHeader, SessionFormatJsonObject,
};
use serde_json::Value;
use std::collections::HashSet;
/// Validate an untrusted native V4 header value before narrowing to an object.
pub fn assert_released_v4_header_value(value: &Value) -> Result {
    let h = value
        .as_object()
        .ok_or_else(|| error("expected format v4 header"))?;
    assert_released_v4_header(h)
}
pub fn assert_released_v4_header(header: &SessionFormatHeader) -> Result {
    if !number_eq(header.get("version"), 4) {
        return Err(error("expected format v4 header"));
    }
    let required = ["version", "id", "createdAt", "isSeeded", "delegationDepth"];
    let allowed = [
        "version",
        "id",
        "createdAt",
        "isSeeded",
        "delegationDepth",
        "cwd",
        "parentSession",
        "origin",
        "agentPreset",
    ];
    if let Some(missing) = required.iter().find(|k| !header.contains_key(**k)) {
        return Err(error(format!(
            "format v4 header lacks required field {missing}"
        )));
    }
    // Object.keys puts canonical array-index keys before other own properties.
    let mut keys: Vec<_> = header.keys().collect();
    keys.sort_by_key(|k| {
        k.parse::<u32>()
            .ok()
            .filter(|n| *n < u32::MAX && n.to_string() == **k)
            .map_or((1, 0), |n| (0, n))
    });
    if let Some(key) = keys.into_iter().find(|k| !allowed.contains(&k.as_str())) {
        return Err(error(format!(
            "format v4 header has unexpected field {key}"
        )));
    }
    if !field(header, "id").is_string() {
        return Err(error("format v4 header id must be a string"));
    }
    count(header.get("createdAt"), "format v4 header createdAt")?;
    count(
        header.get("delegationDepth"),
        "format v4 header delegationDepth",
    )?;
    if !field(header, "isSeeded").is_boolean() {
        return Err(error("format v4 header isSeeded must be boolean"));
    }
    if let Some(cwd) = header.get("cwd")
        && !absolute_cwd(cwd)
    {
        return Err(error("format v4 header cwd must be absolute"));
    }
    for key in ["parentSession", "agentPreset"] {
        if header.get(key).is_some_and(|v| !v.is_string()) {
            return Err(error(format!("format v4 header {key} must be a string")));
        }
    }
    if header.get("origin").is_some_and(|v| v != "subagent") {
        return Err(error("format v4 header origin must be \"subagent\""));
    }
    Ok(())
}
pub fn restore_released_v4_artifact(
    artifact: SessionFormatArtifact,
    known_event_types: &HashSet<String>,
) -> Result<SessionFormatArtifact> {
    assert_released_v4_header(&artifact.header)?;
    let cut = hoh_session_format::session_format_count_u64(
        artifact.inherited_event_count,
        "format v4 inherited event count",
    )?;
    if cut > artifact.events.len() as u64 {
        return Err(error("format v4 inherited event count exceeds its events"));
    }
    if field(&artifact.header, "isSeeded") != true && cut != 0 {
        return Err(error("unseeded format v4 Session has inherited events"));
    }
    let mut last_inherited_marker = None;
    for (index, event) in artifact.events.iter().enumerate() {
        let known = known_event_types.contains(kind(event));
        if !known && field(event, "ignorable") != true {
            return Err(refusal(format!(
                "format v4 contains unknown event type {} at seq {index}",
                quoted(event.get("type"))
            )));
        }
        if !number_eq(event.get("seq"), index as u64) {
            return Err(error(format!("format v4 event {index} is not dense")));
        }
        if !known {
            continue;
        }
        let row = Value::Object(event.clone());
        assert_v4_retired_syntax(&row)?;
        assert_v4_system_message_fields(&row)?;
        assert_v4_tool_result_message(event)?;
        assert_v4_fork_result(&row)?;
        if kind(event) == "session/end-seed"
            && field(event, "data")
                .get("inherited")
                .is_some_and(|v| v == true)
        {
            last_inherited_marker = Some(index as u64);
        }
    }
    if field(&artifact.header, "isSeeded") == true && last_inherited_marker != Some(cut) {
        return Err(error(
            "format v4 seeded header disagrees with its last inherited end-seed marker",
        ));
    }
    if field(&artifact.header, "isSeeded") != true && last_inherited_marker.is_some() {
        return Err(error(
            "format v4 unseeded Session contains an inherited end-seed marker",
        ));
    }
    assert_released_v4_relationships(&artifact, known_event_types)?;
    Ok(artifact)
}
pub fn validate_delivery_accepted(
    event: &SessionFormatEvent,
    current_version: u64,
) -> Result<Option<&str>> {
    if kind(event) != "session-log-deepseek/delivery-accepted" {
        return Ok(None);
    }
    let data = field(event, "data")
        .as_object()
        .ok_or_else(|| error("delivery-accepted data must be an object"))?;
    let version = if data.contains_key("sessionFormatVersion") {
        count(
            data.get("sessionFormatVersion"),
            "delivery sessionFormatVersion",
        )?
    } else {
        0
    };
    if version != current_version {
        return Ok(None);
    }
    let through = count(data.get("throughSeq"), "delivery throughSeq")?;
    if event
        .get("seq")
        .and_then(Value::as_f64)
        .is_some_and(|n| through as f64 >= n)
    {
        return Err(error("delivery throughSeq must precede its marker"));
    }
    let id = nonempty(data.get("sessionId"))
        .ok_or_else(|| error("delivery requires a nonempty Session id"))?;
    Ok(Some(id))
}
pub fn assert_released_v4_relationships(
    artifact: &SessionFormatArtifact,
    known_event_types: &HashSet<String>,
) -> Result {
    let mut ids = HashSet::new();
    let mut names: Vec<&Value> = vec![];
    for event in &artifact.events {
        if !known_event_types.contains(kind(event)) {
            continue;
        }
        assert_v4_developer_data(event)?;
        assert_v4_message_sources(event)?;
        if let Some(id) = validate_delivery_accepted(event, 4)?
            && event
                .get("seq")
                .and_then(Value::as_f64)
                .is_some_and(|n| n >= artifact.inherited_event_count as f64)
            && field(&artifact.header, "id") != id
        {
            return Err(error(
                "current-generation delivery marker names the wrong Session",
            ));
        }
        if kind(event) == "subagent/catalog"
            && event
                .get("seq")
                .and_then(Value::as_f64)
                .is_some_and(|n| n >= artifact.inherited_event_count as f64)
        {
            let fact = native_catalog_fact(field(event, "data"))?;
            let id = text(fact.get("childId"));
            if !ids.insert(id.clone()) {
                return Err(error(format!("duplicate catalog child {id}")));
            }
            if let Some(name) = fact.get("name") {
                if names.iter().any(|prior| strict_eq(Some(prior), Some(name))) {
                    return Err(error(format!(
                        "duplicate catalog child name {}",
                        text(Some(name))
                    )));
                }
                names.push(name);
            }
        }
    }
    assert_v4_lifecycle_relationships(artifact, known_event_types)
}
fn native_catalog_fact(value: &Value) -> Result<&SessionFormatJsonObject> {
    let Some(v) = value.as_object().filter(|v| number_eq(v.get("version"), 3)) else {
        return catalog_fact(value);
    };
    let name = field(v, "name").as_str();
    let valid_name = name.is_some_and(|name| {
        let b = name.as_bytes();
        !b.is_empty()
            && b.len() <= 32
            && b[0].is_ascii_lowercase()
            && b[1..]
                .iter()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
            && name != "lead"
    });
    if !field(v, "childId").is_string()
        || !matches!(field(v, "mode").as_str(), Some("continuable" | "external"))
        || !valid_name
        || (field(v, "mode") == "continuable" && !field(v, "label").is_string())
        || v.get("label").is_some_and(|v| !v.is_string())
    {
        return Err(error(
            "subagent/catalog requires a supported versioned catalog fact",
        ));
    }
    count(v.get("childCreatedAt"), "catalog child creation time")?;
    Ok(v)
}
