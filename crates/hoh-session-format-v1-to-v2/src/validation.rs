use crate::dispositions::RELEASED_V2_EVENT_DISPOSITIONS;
use crate::helpers::*;
use hoh_session_format::{
    SessionFormatArtifact, SessionFormatHeader, SessionFormatJsonObject, session_format_count,
    session_format_count_u64, session_format_safe_integer,
};
use hoh_session_format_v0_to_v1::{
    ReleasedRelationshipExtensions, assert_released_artifact_relationships,
};
use serde_json::Value;
use std::collections::HashSet;

pub fn released_v2_record<'a>(
    value: &'a Value,
    label: &str,
) -> Result<&'a SessionFormatJsonObject> {
    value
        .as_object()
        .ok_or_else(|| error(format!("{label} must be an object")))
}
pub fn assert_released_v2_keys(
    value: &SessionFormatJsonObject,
    required: &[&str],
    optional: &[&str],
    label: &str,
) -> Result {
    if let Some(missing) = required.iter().find(|key| !value.contains_key(**key)) {
        return Err(error(format!("{label} lacks required field {missing}")));
    }
    if let Some(unexpected) = ordered_keys(value)
        .into_iter()
        .find(|key| !required.contains(&key.as_str()) && !optional.contains(&key.as_str()))
    {
        return Err(error(format!("{label} has unexpected field {unexpected}")));
    }
    Ok(())
}
pub fn assert_released_v2_header(header: &SessionFormatHeader) -> Result {
    assert_header(header)
}
pub fn assert_released_v2_header_value(value: &Value) -> Result {
    assert_header(released_v2_record(value, "format v2 header")?)
}
fn assert_header(header: &SessionFormatHeader) -> Result {
    assert_released_v2_keys(
        header,
        &["version", "id", "createdAt", "isSeeded", "delegationDepth"],
        &["cwd", "parentSession", "origin", "agentPreset"],
        "format v2 header",
    )?;
    if !number_eq(field(header, "version"), 2) {
        return Err(error("expected format v2 header"));
    }
    if !field(header, "id").is_string() {
        return Err(error("format v2 header id must be a string"));
    }
    session_format_count(field(header, "createdAt"), "format v2 header createdAt")?;
    session_format_count(
        field(header, "delegationDepth"),
        "format v2 header delegationDepth",
    )?;
    if !field(header, "isSeeded").is_boolean() {
        return Err(error("format v2 header isSeeded must be boolean"));
    }
    if let Some(cwd) = header.get("cwd")
        && !cwd
            .as_str()
            .is_some_and(hoh_util_workspace_path::node_path::is_absolute)
    {
        return Err(error("format v2 header cwd must be absolute"));
    }
    for key in ["parentSession", "agentPreset"] {
        if header.contains_key(key) && !field(header, key).is_string() {
            return Err(error(format!("format v2 header {key} must be a string")));
        }
    }
    if header.contains_key("origin") && field(header, "origin") != "subagent" {
        return Err(error("format v2 header origin must be \"subagent\""));
    }
    Ok(())
}
pub fn restore_released_v2_artifact(
    artifact: SessionFormatArtifact,
    known_event_types: &HashSet<String>,
    relationship_header_version: Option<u64>,
) -> Result<SessionFormatArtifact> {
    validate(
        &artifact,
        true,
        Some(known_event_types),
        relationship_header_version,
    )?;
    Ok(artifact)
}
pub fn assert_released_v2_physical_artifact(artifact: &SessionFormatArtifact) -> Result {
    validate(artifact, false, None, None)
}
pub(crate) fn validate_current(
    artifact: &SessionFormatArtifact,
    known: &HashSet<String>,
    version: Option<u64>,
) -> Result {
    validate(artifact, true, Some(known), version)
}
fn validate(
    artifact: &SessionFormatArtifact,
    current: bool,
    known: Option<&HashSet<String>>,
    relationship_version: Option<u64>,
) -> Result {
    assert_released_v2_header(&artifact.header)?;
    let cut = session_format_count_u64(
        artifact.inherited_event_count,
        "format v2 inherited event count",
    )?;
    if cut > artifact.events.len() as u64 {
        return Err(error("format v2 inherited event count exceeds its events"));
    }
    let seeded = field(&artifact.header, "isSeeded") == true;
    if !seeded && cut != 0 {
        return Err(error("unseeded format v2 Session has inherited events"));
    }
    let mut last = None;
    for (index, event) in artifact.events.iter().enumerate() {
        let Some(kind) = field(event, "type").as_str() else {
            return Err(error(format!(
                "format v2 event {index} type must be a string"
            )));
        };
        let disposition = RELEASED_V2_EVENT_DISPOSITIONS.get(kind);
        if current
            && disposition.is_none()
            && !known.is_some_and(|types| types.contains(kind))
            && field(event, "ignorable") != true
        {
            return Err(refusal(format!(
                "format v2 contains unknown event type {} at seq {index}",
                quoted(event.get("type"))
            )));
        }
        let surface = disposition.is_some() && is_surface(kind);
        let optional: &[&str] = if !current || disposition.is_none() || surface {
            &["ignorable", "sourceEventSeqs", "surfaceOp"]
        } else {
            &["ignorable"]
        };
        assert_released_v2_keys(
            event,
            &["type", "seq", "time", "data"],
            optional,
            &format!("format v2 event {index}"),
        )?;
        if !number_eq(field(event, "seq"), index as u64) {
            return Err(error(format!("format v2 event {index} is not dense")));
        }
        session_format_safe_integer(
            field(event, "time"),
            &format!("format v2 event {index} time"),
        )?;
        if event.contains_key("ignorable") && field(event, "ignorable") != true {
            return Err(error(format!(
                "format v2 event {index} ignorable must be true when present"
            )));
        }
        if kind == "session/end-seed" {
            let data = released_v2_record(
                field(event, "data"),
                &format!("session/end-seed {index} data"),
            )?;
            if field(data, "inherited") == true {
                last = Some(index as u64);
            }
        }
    }
    if seeded && last != Some(cut) {
        return Err(error(
            "format v2 seeded header disagrees with its last inherited end-seed marker",
        ));
    }
    if !seeded && last.is_some() {
        return Err(error(
            "format v2 unseeded Session contains an inherited end-seed marker",
        ));
    }
    if current {
        let extensions = ReleasedRelationshipExtensions {
            step_events: HashSet::from(["assistant/attempt".to_owned()]),
            preserved_source_title_request_text: true,
            ..Default::default()
        };
        if let Some(version) = relationship_version {
            let mut view = artifact.clone();
            view.header.insert("version".into(), version.into());
            assert_released_artifact_relationships(&view, &extensions)?;
        } else {
            assert_released_artifact_relationships(artifact, &extensions)?;
        }
    }
    Ok(())
}
pub(crate) fn is_surface(kind: &str) -> bool {
    matches!(kind, "user/message" | "assistant/message" | "tool/result")
}
