use crate::dispositions::released_v0_event_dispositions;
use crate::payload_validation::{SUBAGENT_DESCRIPTOR_VERSIONS, assert_released_payload_semantics};
use crate::validation_helpers::{
    assert_released_v0_keys, event_seq, event_type, field, js_stringify, released_v0_record,
};
use hoh_session_format::{
    SessionFormatArtifact, SessionFormatError, SessionFormatEvent, SessionFormatHeader,
    session_format_count, session_format_count_u64, session_format_safe_integer,
};
use serde_json::Value;
use std::collections::HashSet;

pub fn assert_released_session_format_header(
    header: &SessionFormatHeader,
    version: u64,
) -> Result<(), SessionFormatError> {
    let label = format!("format v{version} header");
    assert_released_v0_keys(
        header,
        &["version", "id", "createdAt", "isSeeded", "delegationDepth"],
        &["cwd", "parentSession", "origin", "agentPreset"],
        &label,
    )?;
    if !number_eq(field(header, "version"), version) {
        return Err(SessionFormatError::new(format!(
            "expected format v{version} header"
        )));
    }
    if !field(header, "id").is_string() {
        return Err(SessionFormatError::new(format!(
            "{label} id must be a string"
        )));
    }
    session_format_count(field(header, "createdAt"), &format!("{label} createdAt"))?;
    if !field(header, "isSeeded").is_boolean() {
        return Err(SessionFormatError::new(format!(
            "{label} isSeeded must be a boolean"
        )));
    }
    session_format_count(
        field(header, "delegationDepth"),
        &format!("{label} delegationDepth"),
    )?;
    for key in ["cwd", "parentSession", "agentPreset"] {
        if header.contains_key(key) && !field(header, key).is_string() {
            return Err(SessionFormatError::new(format!(
                "{label} {key} must be a string"
            )));
        }
    }
    if let Some(cwd) = field(header, "cwd").as_str()
        && !released_path_is_absolute(cwd)
    {
        return Err(SessionFormatError::new(format!(
            "{label} cwd must be absolute"
        )));
    }
    if header.contains_key("origin") && field(header, "origin") != "subagent" {
        return Err(SessionFormatError::new(format!(
            "{label} origin must be \"subagent\""
        )));
    }
    Ok(())
}

fn released_path_is_absolute(path: &str) -> bool {
    #[cfg(windows)]
    {
        let b = path.as_bytes();
        matches!(b.first(), Some(b'/' | b'\\'))
            || (b.len() >= 3
                && b[0].is_ascii_alphabetic()
                && b[1] == b':'
                && matches!(b[2], b'/' | b'\\'))
    }
    #[cfg(not(windows))]
    {
        std::path::Path::new(path).is_absolute()
    }
}

pub(crate) fn number_eq(value: &Value, expected: u64) -> bool {
    value.as_f64() == Some(expected as f64)
}

pub fn assert_released_v1_header(header: &SessionFormatHeader) -> Result<(), SessionFormatError> {
    assert_released_session_format_header(header, 1)
}

pub fn restore_released_v1_artifact(
    artifact: SessionFormatArtifact,
    known_event_types: &HashSet<String>,
) -> Result<SessionFormatArtifact, SessionFormatError> {
    assert_released_v1_header(&artifact.header)?;
    assert_released_artifact_coordinates(&artifact, false, Some(known_event_types), false, false)?;
    Ok(artifact)
}

pub fn assert_released_artifact_coordinates(
    artifact: &SessionFormatArtifact,
    allow_legacy_steering: bool,
    known_event_types: Option<&HashSet<String>>,
    vocabulary_neutral: bool,
    frozen_envelope: bool,
) -> Result<(), SessionFormatError> {
    let inherited = session_format_count_u64(
        artifact.inherited_event_count,
        "Session inheritedEventCount",
    )?;
    if inherited > artifact.events.len() as u64 {
        return Err(SessionFormatError::new(
            "Session inheritedEventCount exceeds its event count",
        ));
    }
    if field(&artifact.header, "isSeeded") != true && inherited != 0 {
        return Err(SessionFormatError::new(
            "unseeded Session inheritedEventCount must be 0",
        ));
    }
    let inventory = released_v0_event_dispositions();
    for (index, event) in artifact.events.iter().enumerate() {
        let Some(kind) = field(event, "type").as_str() else {
            return Err(SessionFormatError::new(format!(
                "Session event {index} type must be a string"
            )));
        };
        let legacy = allow_legacy_steering
            && matches!(
                kind,
                "steering/message"
                    | "request/header-delta"
                    | "mode/set"
                    | "compact/start"
                    | "compact/summary"
                    | "compact/end"
                    | "compact/prune"
            );
        let current_known = known_event_types.is_some_and(|types| types.contains(kind));
        let ignorable_current =
            !allow_legacy_steering && !current_known && field(event, "ignorable") == true;
        if !current_known && !legacy && !ignorable_current && !vocabulary_neutral {
            return Err(if allow_legacy_steering {
                unknown_historical(kind, index as u64)
            } else {
                SessionFormatError::unsupported_migration(format!(
                    "format v1 contains unknown required event type {} at seq {index}",
                    js_stringify(&Value::from(kind))
                ))
            });
        }
        let surface = if inventory.contains_key(kind) {
            is_surface(kind)
        } else {
            kind == "steering/message"
        };
        let optional: &[&str] = if !frozen_envelope || surface {
            &["ignorable", "sourceEventSeqs", "surfaceOp"]
        } else {
            &["ignorable"]
        };
        assert_released_v0_keys(
            event,
            &["type", "seq", "time", "data"],
            optional,
            &format!("Session event {index}"),
        )?;
        if !number_eq(field(event, "seq"), index as u64) {
            return Err(SessionFormatError::new(format!(
                "Session event {index} has non-dense seq {}",
                event
                    .get("seq")
                    .map(js_stringify)
                    .unwrap_or_else(|| "undefined".into())
            )));
        }
        session_format_safe_integer(field(event, "time"), &format!("Session event {index} time"))?;
        if event.contains_key("ignorable") && field(event, "ignorable") != true {
            return Err(SessionFormatError::new(format!(
                "Session event {index} ignorable must be true when present"
            )));
        }
        if frozen_envelope && surface {
            assert_released_surface_metadata(event, index as u64, kind, "allow-empty-assistant")?;
        }
    }
    Ok(())
}

pub(crate) fn is_surface(kind: &str) -> bool {
    matches!(kind, "user/message" | "assistant/message" | "tool/result")
}
pub(crate) fn unknown_historical(kind: &str, seq: u64) -> SessionFormatError {
    SessionFormatError::unsupported_migration(format!(
        "format v0 contains unknown historical event type {} at seq {seq}; migration refuses unknown historical events even when ignorable",
        js_stringify(&Value::from(kind))
    ))
}

pub fn assert_released_surface_metadata(
    record: &SessionFormatEvent,
    seq: u64,
    kind: &str,
    assistant_sources: &str,
) -> Result<(), SessionFormatError> {
    let label = format!("{kind} {seq}");
    if kind == "assistant/message"
        && record.contains_key("sourceEventSeqs")
        && assistant_sources == "forbid-assistant"
    {
        return Err(SessionFormatError::new(format!(
            "assistant/message {seq} retains obsolete chunk references"
        )));
    }
    if let Some(sources) = record.get("sourceEventSeqs") {
        let Some(sources) = sources.as_array() else {
            return Err(SessionFormatError::new(format!(
                "{label} sourceEventSeqs must be an array"
            )));
        };
        let mut seen = HashSet::new();
        for source in sources {
            let current = session_format_count(source, &format!("{label} sourceEventSeqs member"))?;
            if current >= seq || !seen.insert(current) {
                return Err(SessionFormatError::new(format!(
                    "{label} sourceEventSeqs must be unique earlier seqs"
                )));
            }
        }
        if sources.is_empty() && kind != "assistant/message" {
            return Err(SessionFormatError::new(format!(
                "{label} sourceEventSeqs must be non-empty"
            )));
        }
    }
    let Some(operation) = record.get("surfaceOp") else {
        return Ok(());
    };
    if operation == "append" {
        return Ok(());
    }
    let replacement = released_v0_record(operation, &format!("{label} surfaceOp"))?;
    assert_released_v0_keys(
        replacement,
        &["op", "start", "end"],
        &[],
        &format!("{label} surfaceOp"),
    )?;
    if field(replacement, "op") != "replace" {
        return Err(SessionFormatError::new(format!(
            "{label} surfaceOp must replace"
        )));
    }
    let start = session_format_count(
        field(replacement, "start"),
        &format!("{label} surface start"),
    )?;
    let end = session_format_count(field(replacement, "end"), &format!("{label} surface end"))?;
    if start >= seq || end >= seq {
        return Err(SessionFormatError::new(format!(
            "{label} has an invalid surface replacement"
        )));
    }
    Ok(())
}

pub fn assert_released_event_payload(
    event: &SessionFormatEvent,
    version: u64,
) -> Result<(), SessionFormatError> {
    let kind = event_type(event);
    let seq = event_seq(event);
    let inventory = released_v0_event_dispositions();
    let Some(disposition) = inventory.get(kind) else {
        return Err(SessionFormatError::unsupported_migration(format!(
            "format v0 contains unknown historical event type {} at seq {seq}; migration refuses unknown historical events even when ignorable",
            event
                .get("type")
                .map(js_stringify)
                .unwrap_or_else(|| "undefined".into())
        )));
    };
    let label = format!("{kind} {seq}");
    let data = released_v0_record(field(event, "data"), &format!("{label} data"))?;
    if kind == "subagent/descriptor"
        && !SUBAGENT_DESCRIPTOR_VERSIONS
            .iter()
            .any(|v| number_eq(field(data, "version"), *v))
    {
        let descriptor_version =
            session_format_count(field(data, "version"), &format!("{label} version"))?;
        if version == 0 {
            return Err(SessionFormatError::unsupported_migration(format!(
                "{label} uses unsupported descriptor version {descriptor_version}"
            )));
        }
        return Ok(());
    }
    let required: Vec<_> = disposition.required.iter().map(String::as_str).collect();
    let mut optional: Vec<_> = disposition.optional.iter().map(String::as_str).collect();
    if version == 1 && kind == "session-log-deepseek/delivery-accepted" {
        optional.push("sessionFormatVersion");
    }
    assert_released_v0_keys(data, &required, &optional, &format!("{label} data"))?;
    for key in &disposition.opaque {
        if data.contains_key(key) && !hoh_util_values::is_json_value(field(data, key)) {
            return Err(SessionFormatError::new(format!(
                "{label} opaque {key} is not lossless JSON"
            )));
        }
    }
    assert_released_payload_semantics(event, version)
}
