//! Pure session projections for subagent identity and active-turn duration.

use std::sync::Arc;

use hoh_error::BoxError;
use hoh_session::{SessionEvent, SessionHeader};
use hoh_session_projection::{ProjectionDefinition, ProjectionWire};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    descriptor::{SubagentDescriptorData, fold_subagent_descriptor},
    projection_types::{ActiveInterval, SubagentIdentityProjection, SubagentTimingProjection},
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimingState {
    pub settled_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<ActiveInterval>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_turn_start: Option<i64>,
    pub descriptor_seen: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_turn_completed: Option<bool>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<SubagentIdentityProjection>,
}

fn state_schema<S>(value: Option<&Value>) -> Result<Arc<S>, BoxError>
where
    S: serde::de::DeserializeOwned + Default,
{
    value.map_or_else(
        || Ok(Arc::new(S::default())),
        |value| Ok(Arc::new(serde_json::from_value(value.clone())?)),
    )
}

fn validate_timing(state: &TimingState) -> Result<(), BoxError> {
    const MAX_SAFE: i64 = 9_007_199_254_740_991;
    let active_valid = state.active.as_ref().is_none_or(|active| {
        (0..=MAX_SAFE).contains(&active.since) && (0..=MAX_SAFE).contains(&active.through)
    });
    if !(0..=MAX_SAFE).contains(&state.settled_ms)
        || state
            .pending_turn_start
            .is_some_and(|time| !(0..=MAX_SAFE).contains(&time))
        || !active_valid
    {
        return Err("subagent timing values must be non-negative integers".into());
    }
    Ok(())
}

fn timing_state_schema(value: Option<&Value>) -> Result<Arc<TimingState>, BoxError> {
    let state = state_schema::<TimingState>(value)?;
    validate_timing(&state)?;
    Ok(state)
}

pub fn subagent_timing_projection_definition() -> ProjectionDefinition<TimingState> {
    ProjectionDefinition {
        key: "subagentTiming".into(),
        state_schema: Arc::new(timing_state_schema),
        init: Arc::new(|_: &SessionHeader, _| Ok(Arc::new(TimingState::default()))),
        apply: Arc::new(|state, event| {
            let mut next = (**state).clone();
            match event.event_type() {
                "turn/start" => {
                    next.last_turn_completed = None;
                    if state.descriptor_seen {
                        next.active = Some(ActiveInterval {
                            since: event.time(),
                            through: event.time(),
                        });
                    } else {
                        next.pending_turn_start = Some(event.time());
                    }
                }
                "subagent/descriptor" => {
                    let active_since = state
                        .active
                        .as_ref()
                        .map(|active| active.since)
                        .or(state.pending_turn_start);
                    next = TimingState {
                        descriptor_seen: true,
                        active: active_since.map(|since| ActiveInterval {
                            since,
                            through: event.time(),
                        }),
                        ..TimingState::default()
                    };
                }
                "turn/end" if !state.descriptor_seen => {
                    next.pending_turn_start = None;
                }
                "turn/end" => {
                    let Some(active) = &state.active else {
                        return Ok(state.clone());
                    };
                    next.active = None;
                    next.settled_ms += (event.time() - active.since).max(0);
                    next.last_turn_completed = Some(
                        event.data().pointer("/reason/kind").and_then(Value::as_str)
                            == Some("completed"),
                    );
                }
                _ => {
                    let Some(active) = &mut next.active else {
                        return Ok(state.clone());
                    };
                    active.through = event.time();
                }
            }
            Ok(Arc::new(next))
        }),
        wire: Some(ProjectionWire::new(
            Arc::new(|value| {
                let value = value.ok_or_else(|| -> BoxError {
                    "subagentTiming projection view is required".into()
                })?;
                let view: SubagentTimingProjection = serde_json::from_value(value.clone())?;
                if !(0..=9_007_199_254_740_991).contains(&view.settled_ms)
                    || view.active.as_ref().is_some_and(|active| {
                        !(0..=9_007_199_254_740_991).contains(&active.since)
                            || !(0..=9_007_199_254_740_991).contains(&active.through)
                    })
                {
                    return Err("subagent timing values must be non-negative integers".into());
                }
                Ok(Arc::new(value.clone()))
            }),
            Arc::new(|state| {
                let view = SubagentTimingProjection {
                    settled_ms: state.settled_ms,
                    active: state.active.clone(),
                    last_turn_completed: state.last_turn_completed,
                };
                Ok(Some(Arc::new(serde_json::to_value(view)?)))
            }),
        )),
        state_version: 3,
    }
}

fn descriptor_identity(event: &SessionEvent) -> Option<SubagentIdentityProjection> {
    let event = Arc::new(event.clone());
    match fold_subagent_descriptor(std::slice::from_ref(&event))
        .ok()
        .flatten()?
    {
        SubagentDescriptorData::OneShot(descriptor) => Some(SubagentIdentityProjection::OneShot {
            label: descriptor.label,
            seq: event.seq(),
        }),
        SubagentDescriptorData::Continuable(descriptor) => {
            Some(SubagentIdentityProjection::Continuable {
                label: descriptor.label,
                seq: event.seq(),
            })
        }
    }
}

pub fn subagent_identity_projection_definition() -> ProjectionDefinition<IdentityState> {
    ProjectionDefinition {
        key: "subagent".into(),
        state_schema: Arc::new(state_schema::<IdentityState>),
        init: Arc::new(|_, _| Ok(Arc::new(IdentityState::default()))),
        apply: Arc::new(|state, event| {
            if event.event_type() != "subagent/descriptor" {
                return Ok(state.clone());
            }
            Ok(Arc::new(IdentityState {
                identity: descriptor_identity(event),
            }))
        }),
        wire: Some(ProjectionWire::new(
            Arc::new(|value| {
                let value = value.ok_or_else(|| -> BoxError {
                    "subagent identity projection view is required".into()
                })?;
                if !value.is_null() {
                    let _: SubagentIdentityProjection = serde_json::from_value(value.clone())?;
                }
                Ok(Arc::new(value.clone()))
            }),
            Arc::new(|state| {
                Ok(Some(Arc::new(match &state.identity {
                    Some(identity) => serde_json::to_value(identity)?,
                    None => Value::Null,
                })))
            }),
        )),
        state_version: 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoh_session::SessionLogOffset;
    use serde_json::json;

    fn header() -> SessionHeader {
        SessionHeader::new("s".into(), 0)
    }
    fn event(seq: u64, time: i64, kind: &str, data: Value) -> SessionEvent {
        SessionEvent::from_value(json!({ "seq": seq, "time": time, "type": kind, "data": data }))
    }

    #[test]
    fn timing_counts_descriptor_owned_active_interval() {
        let definition = subagent_timing_projection_definition();
        let mut state = (definition.init)(&header(), SessionLogOffset::default()).unwrap();
        for event in [
            event(0, 10, "turn/start", json!({})),
            event(
                1,
                12,
                "subagent/descriptor",
                json!({ "version": 3, "mode": "one-shot", "provider": "p" }),
            ),
            event(2, 15, "message", json!({})),
            event(
                3,
                20,
                "turn/end",
                json!({ "reason": { "kind": "completed" } }),
            ),
        ] {
            state = (definition.apply)(&state, &event).unwrap();
        }
        assert_eq!(state.settled_ms, 10);
        assert_eq!(state.last_turn_completed, Some(true));
        assert!(state.active.is_none());
    }

    #[test]
    fn identity_uses_descriptor_event_sequence() {
        let definition = subagent_identity_projection_definition();
        let state = (definition.init)(&header(), SessionLogOffset::default()).unwrap();
        let state = (definition.apply)(
            &state,
            &event(
                7,
                1,
                "subagent/descriptor",
                json!({
                    "version": 3, "mode": "continuable", "provider": "p", "label": "worker"
                }),
            ),
        )
        .unwrap();
        let Some(SubagentIdentityProjection::Continuable { label, seq }) = &state.identity else {
            panic!("continuable")
        };
        assert_eq!(label, "worker");
        assert_eq!(u64::from(*seq), 7);
    }

    #[test]
    fn unknown_descriptor_version_has_no_identity() {
        let definition = subagent_identity_projection_definition();
        let state = (definition.init)(&header(), SessionLogOffset::default()).unwrap();
        let state = (definition.apply)(
            &state,
            &event(0, 1, "subagent/descriptor", json!({ "version": 99 })),
        )
        .unwrap();
        assert!(state.identity.is_none());
    }

    #[test]
    fn timing_checkpoint_rejects_values_outside_javascript_safe_integer_range() {
        let definition = subagent_timing_projection_definition();
        assert!(
            (definition.state_schema)(Some(&json!({
                "settledMs": 9_007_199_254_740_992_u64,
                "descriptorSeen": true
            })))
            .is_err()
        );
    }
}
