//! Frozen released writer validation, matching `src/testing/validation.ts`.
pub use crate::validation::assert_released_v2_physical_artifact;
use crate::{
    RELEASED_V2_EVENT_DISPOSITIONS, RELEASED_V2_EVENT_TYPES,
    helpers::*,
    validation::{assert_released_v2_keys, is_surface, released_v2_record},
};
use hoh_llm::{BlockAssembler, expand_assistant_stream_value};
use hoh_session_format::{
    SessionFormatArtifact, SessionFormatError, SessionFormatEvent, session_format_count,
    snapshot_session_format_json_labeled,
};
use hoh_session_format_v0_to_v1::{
    assert_released_payload_semantics, assert_released_surface_metadata,
};
use hoh_util_values::deep_equal_json;
use serde_json::{Value, json};
use std::collections::HashSet;

pub fn assert_released_v2_artifact(artifact: &SessionFormatArtifact) -> Result {
    assert_released_v2_physical_artifact(artifact)?;
    for (index, event) in artifact.events.iter().enumerate() {
        let disposition = RELEASED_V2_EVENT_DISPOSITIONS
            .get(kind(event))
            .ok_or_else(|| {
                refusal(format!(
                    "format v2 contains unknown event type {} at seq {index}",
                    quoted(event.get("type"))
                ))
            })?;
        if is_surface(kind(event)) {
            assert_released_surface_metadata(event, index as u64, kind(event), "forbid-assistant")?;
        }
        let data = released_v2_record(field(event, "data"), &format!("{} data", label(event)))?;
        let required: Vec<_> = disposition.required.iter().map(String::as_str).collect();
        let optional: Vec<_> = disposition.optional.iter().map(String::as_str).collect();
        assert_released_v2_keys(
            data,
            &required,
            &optional,
            &format!("{} data", label(event)),
        )?;
        for key in &disposition.opaque {
            if let Some(value) = data.get(key) {
                snapshot_session_format_json_labeled(
                    value,
                    &format!("{} opaque {key}", label(event)),
                )?;
            }
        }
        assert_payload(event, data)?;
    }
    let known: HashSet<_> = RELEASED_V2_EVENT_TYPES
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    crate::validation::validate_current(artifact, &known, None)
}
fn assert_payload(
    event: &SessionFormatEvent,
    data: &hoh_session_format::SessionFormatJsonObject,
) -> Result {
    if matches!(kind(event), "assistant/attempt" | "assistant/message") {
        let turn = session_format_count(field(data, "turn"), &format!("{} turn", label(event)))?;
        let step = session_format_count(field(data, "step"), &format!("{} step", label(event)))?;
        let mut assembler = BlockAssembler::new();
        let timed=(|| {
            let timed=expand_assistant_stream_value(field(data,"stream")).map_err(SessionFormatError::other)?;
            for member in &timed {
                let chunk=object(json!({"type":"assistant/chunk","seq":field(event,"seq"),"time":member["time"],"data":{"turn":turn,"step":step,"chunk":member["chunk"]}}))?;
                assert_released_payload_semantics(&chunk,2)?;
                assembler.push_value(member["chunk"].clone()).map_err(SessionFormatError::other)?;
            } Ok::<_,SessionFormatError>(timed)
        })().map_err(|cause|SessionFormatError::new_with_cause(format!("{} has an invalid embedded stream",label(event)), cause))?;
        if kind(event) == "assistant/attempt" {
            return Ok(());
        }
        assert_released_payload_semantics(event, 2)?;
        if !timed.is_empty() {
            let message = released_v2_record(
                field(data, "message"),
                &format!("assistant/message {} message", text(event.get("seq"))),
            )?;
            let blocks = if field(data, "interrupted") == true {
                assembler.interrupted_blocks()
            } else {
                assembler.blocks().map_err(SessionFormatError::other)?
            };
            let content = serde_json::to_value(blocks).map_err(SessionFormatError::other)?;
            if !deep_equal_json(field(message, "content"), &content) {
                return Err(error(format!(
                    "{} message content disagrees with its embedded stream",
                    label(event)
                )));
            }
            let usage = assembler
                .usage()
                .map(serde_json::to_value)
                .transpose()
                .map_err(SessionFormatError::other)?;
            if !equal_optional(data.get("usage"), usage.as_ref()) {
                return Err(error(format!(
                    "{} usage disagrees with its embedded stream",
                    label(event)
                )));
            }
            let source = released_v2_record(
                field(message, "source"),
                &format!("assistant/message {} source", text(event.get("seq"))),
            )?;
            let replay = assembler
                .replay_state()
                .map_err(SessionFormatError::other)?
                .map(serde_json::to_value)
                .transpose()
                .map_err(SessionFormatError::other)?;
            if !equal_optional(source.get("replayState"), replay.as_ref()) {
                return Err(error(format!(
                    "{} replay state disagrees with its embedded stream",
                    label(event)
                )));
            }
        }
        return Ok(());
    }
    if kind(event) == "session/end-seed" {
        if data.contains_key("inherited") && field(data, "inherited") != true {
            return Err(error(format!(
                "session/end-seed {} inherited must be true when present",
                text(event.get("seq"))
            )));
        }
        return Ok(());
    }
    assert_released_payload_semantics(event, 2)
}
fn equal_optional(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => deep_equal_json(a, b),
        (None, None) => true,
        _ => false,
    }
}
