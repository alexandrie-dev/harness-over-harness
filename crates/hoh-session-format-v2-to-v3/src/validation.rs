use crate::{
    helpers::*,
    payload::{SURFACE_TYPES, assert_v3_event, is_repair_identity, record},
};
use hoh_session_format::{SessionFormatArtifact, SessionFormatEvent, SessionFormatHeader};
use hoh_session_format_v1_to_v2::{assert_released_v2_header, restore_released_v2_artifact};
use serde_json::{Value, json};
use std::collections::HashSet;
pub fn assert_released_v3_header(header: &SessionFormatHeader) -> Result {
    if !number_eq(field(header, "version"), 3) {
        return Err(error("expected format v3 header"));
    }
    let mut view = header.clone();
    view.insert("version".into(), 2.into());
    assert_released_v2_header(&view)
}
pub fn restore_released_v3_artifact(
    artifact: SessionFormatArtifact,
    known_event_types: &HashSet<String>,
) -> Result<SessionFormatArtifact> {
    assert_released_v3_header(&artifact.header)?;
    let mut step: Option<(Value, Value)> = None;
    let mut head: Option<u64> = None;
    let mut has_surface = false;
    let mut events = Vec::with_capacity(artifact.events.len());
    for event in &artifact.events {
        assert_v3_event_admission(event)?;
        assert_v3_event(event, Some(known_event_types))?;
        let system = kind(event) == "system/message";
        if kind(event) == "step/start" {
            let data = record(field(event, "data"), kind(event))?;
            step = Some((field(data, "turn").clone(), field(data, "step").clone()));
        } else if matches!(kind(event), "step/end" | "turn/end") {
            step = None;
        }
        if system {
            let data = record(field(event, "data"), "system/message")?;
            if !step.as_ref().is_some_and(|(turn, step)| {
                hoh_util_values::deep_equal_json(turn, field(data, "turn"))
                    && hoh_util_values::deep_equal_json(step, field(data, "step"))
            }) {
                return Err(error("system/message does not match an open step"));
            }
            if has_surface && head.is_none() {
                return Err(error(
                    "system/message requires a protected first surface head",
                ));
            }
            if field(event, "surfaceOp") == "append" {
                if !has_surface {
                    head = Some(hoh_session_format::session_format_count(
                        field(event, "seq"),
                        "event seq",
                    )?);
                }
            } else {
                let replace = record(field(event, "surfaceOp"), "system replacement")?;
                let start = head.is_some_and(|n| number_eq(field(replace, "startSeq"), n));
                let end = head.is_some_and(|n| number_eq(field(replace, "endSeq"), n));
                if start || end {
                    if !start || !end {
                        return Err(error(
                            "system/message must replace exactly the current system head",
                        ));
                    }
                    head = Some(hoh_session_format::session_format_count(
                        field(event, "seq"),
                        "event seq",
                    )?);
                }
            }
        } else if SURFACE_TYPES.contains(&kind(event)) && field(event, "surfaceOp") != "append" {
            let replace = record(field(event, "surfaceOp"), "surface replacement")?;
            if head.is_some_and(|n| {
                number_eq(field(replace, "startSeq"), n) || number_eq(field(replace, "endSeq"), n)
            }) {
                return Err(error(
                    "surface replacement cannot shadow the protected system head",
                ));
            }
        }
        if matches!(kind(event), "compaction/prune" | "compaction/summary") {
            let data = record(field(event, "data"), kind(event))?;
            if field(data, "shadowedSeqs")
                .as_array()
                .is_some_and(|seqs| seqs.iter().any(|s| head.is_some_and(|n| number_eq(s, n))))
            {
                return Err(error("compaction cannot shadow the protected system head"));
            }
        }
        if SURFACE_TYPES.contains(&kind(event)) {
            has_surface = true;
        }
        let mut projected = relationship_event(event)?;
        if SURFACE_TYPES.contains(&kind(event)) && field(event, "surfaceOp") != "append" {
            let replacement = record(field(event, "surfaceOp"), "surface replacement")?;
            projected.insert("surfaceOp".into(),json!({"op":"replace","start":field(replacement,"startSeq"),"end":field(replacement,"endSeq")}));
        }
        events.push(projected);
    }
    let mut header = artifact.header.clone();
    header.insert("version".into(), 2.into());
    restore_released_v2_artifact(
        SessionFormatArtifact {
            header,
            inherited_event_count: artifact.inherited_event_count,
            events,
        },
        known_event_types,
        Some(3),
    )?;
    Ok(artifact)
}
pub fn assert_v3_event_admission(event: &SessionFormatEvent) -> Result {
    if matches!(
        kind(event),
        "tool/code-dispatch-start" | "tool/code-dispatch"
    ) && field(event, "ignorable") != true
    {
        return Err(refusal(format!(
            "format v3 contains unknown event type {} at seq {}",
            quoted(event.get("type")),
            text(event.get("seq"))
        )));
    }
    Ok(())
}
fn relationship_event(event: &SessionFormatEvent) -> Result<SessionFormatEvent> {
    let mut output = event.clone();
    match kind(event) {
        "tool/ptc-dispatch-start" => {
            output.insert("type".into(), "tool/code-dispatch-start".into());
            return Ok(output);
        }
        "tool/ptc-dispatch" => {
            output.insert("type".into(), "tool/code-dispatch".into());
            return Ok(output);
        }
        "tool/code-dispatch-start" | "tool/code-dispatch" => {
            assert_v3_event_admission(event)?;
            output.insert("type".into(), "v3/opaque-released-event".into());
            return Ok(output);
        }
        _ => {}
    }
    if kind(event) == "system/message" {
        let mut message = record(
            field(record(field(event, "data"), "system data")?, "message"),
            "system message",
        )?
        .clone();
        message.insert("role".into(), "user".into());
        output.insert("type".into(), "user/message".into());
        output.insert("data".into(), Value::Object(message));
        return Ok(output);
    }
    if kind(event) != "tool/result" {
        return Ok(output);
    }
    let mut data = record(field(event, "data"), "tool result")?.clone();
    let Some(err) = data.get("error") else {
        return Ok(output);
    };
    if field(record(err, "tool error")?, "code") != "TOOL_NOT_STARTED" {
        return Ok(output);
    }
    let mut message = record(field(&data, "message"), "tool message")?.clone();
    let source = record(field(&message, "source"), "tool source")?;
    if !is_repair_identity(message.get("id"), source.get("callId")) {
        return Ok(output);
    }
    let prefix = format!("interrupted-tool-result-{}-", text(source.get("callId")));
    message.insert(
        "id".into(),
        format!("{prefix}{}", text(event.get("seq"))).into(),
    );
    data.insert("message".into(), Value::Object(message));
    output.insert("data".into(), Value::Object(data));
    Ok(output)
}
