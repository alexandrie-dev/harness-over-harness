//! Physical framing adapters backed by frozen V4 validation.
use crate::{Result, error::error};
use hoh_session::{SessionEvent, SessionHeader, SessionLogOffset, SessionSeq};
use hoh_session_format::{session_format_count, session_format_safe_integer};
use hoh_session_persistence::SessionFormatUnsupportedError;
use serde_json::{Value, json};
use std::collections::HashSet;

fn count(v: &Value, label: &str) -> Result<u64> {
    Ok(session_format_count(v, label)?)
}
fn problem(message: impl Into<String>) -> hoh_error::BoxError {
    Box::new(hoh_session_format::SessionFormatError::new(message))
}
pub(crate) fn encode_header(header: &SessionHeader, _cut: SessionLogOffset) -> Result<Value> {
    let mut header = header.clone();
    header.delegation_depth = Some(header.delegation_depth.unwrap_or(0));
    let value = serde_json::to_value(&header)?;
    let mut out = serde_json::Map::new();
    out.insert("type".into(), "session".into());
    for (key, value) in value
        .as_object()
        .ok_or_else(|| error("expected format v4 header"))?
    {
        out.insert(key.clone(), value.clone());
    }
    let out = Value::Object(out);
    decode_header(&out)?;
    Ok(out)
}
pub(crate) fn decode_header(value: &Value) -> Result<SessionHeader> {
    let record = value
        .as_object()
        .ok_or_else(|| error("expected format v4 physical header"))?;
    let required = [
        "type",
        "version",
        "id",
        "createdAt",
        "isSeeded",
        "delegationDepth",
    ];
    if record.get("type") != Some(&json!("session"))
        || record.get("version").and_then(Value::as_f64) != Some(4.0)
    {
        return Err(error("expected format v4 physical header"));
    }
    for key in required {
        if !record.contains_key(key) {
            return Err(error(format!(
                "format v4 header lacks required field {key}"
            )));
        }
    }
    for key in record.keys() {
        if !required.contains(&key.as_str())
            && !["cwd", "parentSession", "origin", "agentPreset"].contains(&key.as_str())
        {
            return Err(error(format!(
                "format v4 header has unexpected field {key}"
            )));
        }
    }
    if !value["id"].is_string() {
        return Err(error("format v4 header id must be a string"));
    }
    count(&value["createdAt"], "format v4 header createdAt")?;
    count(
        &value["delegationDepth"],
        "format v4 header delegationDepth",
    )?;
    if !value["isSeeded"].is_boolean() {
        return Err(error("format v4 header isSeeded must be boolean"));
    }
    if let Some(cwd) = record.get("cwd")
        && !cwd
            .as_str()
            .is_some_and(hoh_util_workspace_path::node_path::is_absolute)
    {
        return Err(error("format v4 header cwd must be absolute"));
    }
    for key in ["parentSession", "agentPreset"] {
        if record.get(key).is_some_and(|v| !v.is_string()) {
            return Err(error(format!("format v4 header {key} must be a string")));
        }
    }
    if record.get("origin").is_some_and(|v| v != "subagent") {
        return Err(error("format v4 header origin must be \"subagent\""));
    }
    let mut header = record.clone();
    header.shift_remove("type");
    Ok(serde_json::from_value(Value::Object(header))?)
}
pub(crate) fn encode_event(event: &SessionEvent) -> Result<Value> {
    assert_row_admission(event.as_value())?;
    let mut value = event.as_value().clone();
    if let Some(sources) = value.get("sourceEventSeqs") {
        let array = sources
            .as_array()
            .ok_or_else(|| error("sourceEventSeqs must be an array"))?;
        let seqs = array
            .iter()
            .map(|v| SessionSeq::new(count(v, "sourceEventSeqs member")?))
            .collect::<Result<Vec<_>>>()?;
        value["sourceEventSeqs"] = serde_json::to_value(hoh_session::encode_seq_ranges(&seqs))?;
    }
    Ok(value)
}
pub(crate) fn decode_event(mut value: Value, expected: u64, index: u64) -> Result<SessionEvent> {
    let record = value
        .as_object()
        .ok_or_else(|| problem(format!("released v2 row {index} must be an object")))?;
    for key in ["type", "seq", "time", "data"] {
        if !record.contains_key(key) {
            return Err(problem(format!(
                "released v2 row {index} lacks required field {key}"
            )));
        }
    }
    for key in record.keys() {
        if ![
            "type",
            "seq",
            "time",
            "data",
            "ignorable",
            "sourceEventSeqs",
            "surfaceOp",
        ]
        .contains(&key.as_str())
        {
            return Err(problem(format!(
                "released v2 row {index} has unexpected field {key}"
            )));
        }
    }
    if !value["type"].is_string() {
        return Err(problem(format!(
            "released v2 row {index} type must be a string"
        )));
    }
    session_format_safe_integer(&value["time"], &format!("released v2 row {index} time"))?;
    if record.get("ignorable").is_some_and(|v| v != true) {
        return Err(problem(format!(
            "released v2 row {index} ignorable must be true when present"
        )));
    }
    if value["seq"].as_f64() != Some(expected as f64) {
        return Err(problem(format!(
            "released v2 row {index} has seq gap (expected {expected}, got {})",
            record
                .get("seq")
                .map_or_else(|| "undefined".into(), hoh_util_values::js_string)
        )));
    }
    if let Some(sources) = record.get("sourceEventSeqs") {
        let seqs = hoh_session::decode_seq_ranges(sources, Some(expected))?;
        let mut seen = HashSet::new();
        if seqs
            .iter()
            .any(|s| s.get() >= expected || !seen.insert(s.get()))
        {
            return Err(problem(
                "sourceEventSeqs ranges must contain unique earlier seqs",
            ));
        }
        value["sourceEventSeqs"] = serde_json::to_value(seqs)?;
    }
    Ok(SessionEvent::from_value(value))
}
pub(crate) fn inherited_cut(
    header: &SessionHeader,
    events: &[SessionEvent],
) -> Result<SessionLogOffset> {
    let last = events
        .iter()
        .enumerate()
        .rev()
        .find(|(_, e)| e.event_type() == "session/end-seed" && e.data()["inherited"] == true)
        .map(|(i, _)| i as u64);
    if header.is_seeded && last.is_none() {
        return Err(problem(
            "released v2 seeded Session lacks an inherited end-seed marker",
        ));
    }
    if !header.is_seeded && last.is_some() {
        return Err(problem(
            "released v2 unseeded Session contains an inherited end-seed marker",
        ));
    }
    SessionLogOffset::new(last.unwrap_or(0))
}
pub(crate) fn assert_relationships(
    header: &SessionHeader,
    events: &[SessionEvent],
    cut: SessionLogOffset,
) -> Result<()> {
    let artifact = hoh_session_format::SessionFormatArtifact {
        header: serde_json::to_value(header)?
            .as_object()
            .ok_or_else(|| error("expected header"))?
            .clone(),
        inherited_event_count: cut.get(),
        events: events
            .iter()
            .map(|e| {
                e.as_value()
                    .as_object()
                    .cloned()
                    .ok_or_else(|| problem("expected decoded event object"))
            })
            .collect::<Result<Vec<_>>>()?,
    };
    let known = hoh_session::KNOWN_SESSION_EVENT_TYPES
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    hoh_session_format_v3_to_v4::assert_released_v4_relationships(&artifact, &known)?;
    Ok(())
}
pub(crate) fn assert_row_admission(row: &Value) -> Result<()> {
    let known = hoh_session::KNOWN_SESSION_EVENT_TYPES
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    hoh_session_format_v3_to_v4::assert_v4_row_admission(row, Some(&known)).map_err(|e| {
        if e.is_unsupported_migration() {
            Box::new(SessionFormatUnsupportedError::new(e.to_string(), None)) as hoh_error::BoxError
        } else {
            Box::new(e)
        }
    })
}
