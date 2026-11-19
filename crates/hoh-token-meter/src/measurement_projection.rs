use crate::{ROLE_OVERHEAD, error, estimate_content, estimate_message};
use hoh_error::BoxError;
use hoh_llm::{AssistantStreamRecord, BlockAssembler, assemble_assistant_stream};
use hoh_session::{
    MAX_SAFE_INTEGER, SessionEvent, SessionSeq, canonical_header, derive_event_message,
};
use hoh_session_projection::ProjectionDefinition;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StepStart {
    #[serde(serialize_with = "serialize_js_number")]
    pub turn: f64,
    #[serde(serialize_with = "serialize_js_number")]
    pub step: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeasurementAnchor {
    pub header_seq: Option<SessionSeq>,
    #[serde(serialize_with = "serialize_js_number")]
    pub assistant_tokens: f64,
    pub usage: Option<Value>,
}

/// Fixed-size recovery facts; positional prices are deliberately excluded.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeasurementState {
    pub header_seq: Option<SessionSeq>,
    pub step_start: Option<StepStart>,
    pub anchor: Option<MeasurementAnchor>,
    pub invalid_seq: Option<SessionSeq>,
}

pub(crate) fn serialize_js_number<S: Serializer>(
    value: &f64,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if value.is_finite() && value.fract() == 0.0 && value.abs() <= MAX_SAFE_INTEGER as f64 {
        if *value >= 0.0 && !value.is_sign_negative() {
            return serializer.serialize_u64(*value as u64);
        }
        return serializer.serialize_i64(*value as i64);
    }
    serializer.serialize_f64(*value)
}

fn coordinates(event: &SessionEvent) -> Result<(f64, f64), BoxError> {
    let turn = event
        .data()
        .get("turn")
        .and_then(Value::as_f64)
        .ok_or_else(|| error("token meter: event has an invalid turn"))?;
    let step = event
        .data()
        .get("step")
        .and_then(Value::as_f64)
        .ok_or_else(|| error("token meter: event has an invalid step"))?;
    Ok((turn, step))
}

/// Advance one measurement prefix without mutating its previous state.
pub fn advance_measurement(
    state: &MeasurementState,
    event: &SessionEvent,
) -> Result<MeasurementState, BoxError> {
    match event.event_type() {
        "request/header" => {
            let header = event
                .data()
                .get("header")
                .ok_or_else(|| error("token meter: request/header is missing header"))?;
            let _ = canonical_header(header);
            Ok(MeasurementState {
                header_seq: Some(event.seq()),
                ..state.clone()
            })
        }
        "step/start" => {
            if let Some(open) = &state.step_start {
                return Err(error(format!(
                    "token meter: step/start at seq {} arrived before turn {}/step {} ended",
                    event.seq().get(),
                    open.turn,
                    open.step
                )));
            }
            let (turn, step) = coordinates(event)?;
            Ok(MeasurementState {
                step_start: Some(StepStart { turn, step }),
                ..state.clone()
            })
        }
        "step/end" => {
            let (turn, step) = coordinates(event)?;
            if state
                .step_start
                .as_ref()
                .is_none_or(|open| open.turn != turn || open.step != step)
            {
                return Err(error(format!(
                    "token meter: step/end at seq {} has no matching step/start event",
                    event.seq().get()
                )));
            }
            Ok(MeasurementState {
                step_start: None,
                ..state.clone()
            })
        }
        "assistant/message" => {
            let (turn, step) = coordinates(event)?;
            if state
                .step_start
                .as_ref()
                .is_none_or(|open| open.turn != turn || open.step != step)
            {
                return Err(error(format!(
                    "token meter: assistant/message at seq {} has no matching step/start event",
                    event.seq().get()
                )));
            }
            let recorded_usage = event.data().get("usage").cloned();
            let (assistant_tokens, usage) =
                if recorded_usage.is_some() && state.header_seq.is_some() {
                    let stream: Vec<AssistantStreamRecord> =
                        serde_json::from_value(event.data().get("stream").cloned().ok_or_else(
                            || error("token meter: assistant/message is missing stream"),
                        )?)
                        .map_err(|_| error("unreachable variant in assembleAssistantStream"))?;
                    let mut assembler = BlockAssembler::new();
                    assemble_assistant_stream(&stream, &mut assembler)?;
                    let blocks = assembler.blocks()?;
                    let tokens = if blocks.is_empty() {
                        0.0
                    } else {
                        (estimate_content(&blocks) + ROLE_OVERHEAD) as f64
                    };
                    (tokens, recorded_usage)
                } else {
                    let tokens = match derive_event_message(event, None) {
                        Some(message) => {
                            estimate_message(&serde_json::from_value((*message).clone())?) as f64
                        }
                        None => 0.0,
                    };
                    (tokens, None)
                };
            Ok(MeasurementState {
                anchor: Some(MeasurementAnchor {
                    header_seq: state.header_seq,
                    assistant_tokens,
                    usage,
                }),
                ..state.clone()
            })
        }
        _ => Ok(state.clone()),
    }
}

fn normalize_seq_fields(value: &mut Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    for key in ["headerSeq", "invalidSeq"] {
        if let Some(value) = object.get_mut(key)
            && value.is_f64()
            && let Some(number) = value.as_f64()
            && number.fract() == 0.0
            && number >= 0.0
            && number <= MAX_SAFE_INTEGER as f64
        {
            *value = Value::from(number as u64);
        }
    }
    if let Some(anchor) = object.get_mut("anchor") {
        normalize_seq_fields(anchor);
    }
}

fn validate_persisted_shape(value: &Value) -> Result<(), BoxError> {
    let object = value
        .as_object()
        .ok_or_else(|| error("tokenMeasurement state must be an object"))?;
    for key in ["headerSeq", "stepStart", "anchor", "invalidSeq"] {
        if !object.contains_key(key) {
            return Err(error(format!("tokenMeasurement state is missing {key}")));
        }
    }
    if let Some(step) = object.get("stepStart").filter(|value| !value.is_null()) {
        let step = step
            .as_object()
            .ok_or_else(|| error("tokenMeasurement stepStart must be an object or null"))?;
        for key in ["turn", "step"] {
            if !step.get(key).is_some_and(Value::is_number) {
                return Err(error(format!(
                    "tokenMeasurement stepStart is missing numeric {key}"
                )));
            }
        }
    }
    if let Some(anchor) = object.get("anchor").filter(|value| !value.is_null()) {
        let anchor = anchor
            .as_object()
            .ok_or_else(|| error("tokenMeasurement anchor must be an object or null"))?;
        for key in ["headerSeq", "assistantTokens", "usage"] {
            if !anchor.contains_key(key) {
                return Err(error(format!("tokenMeasurement anchor is missing {key}")));
            }
        }
        if !anchor["assistantTokens"].is_number() {
            return Err(error("tokenMeasurement assistantTokens must be a number"));
        }
        if let Some(usage) = anchor.get("usage").filter(|value| !value.is_null()) {
            let usage = usage
                .as_object()
                .ok_or_else(|| error("tokenMeasurement usage must be an object or null"))?;
            for key in ["inputTokens", "outputTokens"] {
                if !usage.get(key).is_some_and(Value::is_number) {
                    return Err(error(format!(
                        "tokenMeasurement usage is missing numeric {key}"
                    )));
                }
            }
            for key in [
                "totalTokens",
                "cacheReadTokens",
                "cacheWriteTokens",
                "reasoningTokens",
            ] {
                if usage.get(key).is_some_and(|value| !value.is_number()) {
                    return Err(error(format!(
                        "tokenMeasurement usage has non-numeric {key}"
                    )));
                }
            }
        }
    }
    Ok(())
}

pub fn measurement_projection_definition() -> ProjectionDefinition<MeasurementState> {
    ProjectionDefinition {
        key: "tokenMeasurement".into(),
        state_version: 2,
        state_schema: Arc::new(|value| {
            let mut value = value
                .cloned()
                .ok_or_else(|| error("tokenMeasurement state is missing"))?;
            validate_persisted_shape(&value)?;
            normalize_seq_fields(&mut value);
            Ok(Arc::new(serde_json::from_value(value)?))
        }),
        init: Arc::new(|_, _| Ok(Arc::new(MeasurementState::default()))),
        apply: Arc::new(|state, event| {
            if state.invalid_seq.is_some() {
                return Ok(state.clone());
            }
            Ok(Arc::new(match advance_measurement(state, event) {
                Ok(next) => next,
                Err(_) => MeasurementState {
                    invalid_seq: Some(event.seq()),
                    ..state.as_ref().clone()
                },
            }))
        }),
        wire: None,
    }
}
