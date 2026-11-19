//! Shared pure fold of the Session's turn and step events.

use crate::types::{StepBoundary, StepBoundaryKind, TurnBoundaryProjection};
use hoh_error::BoxError;
use hoh_session::SessionEvent;
use hoh_session_projection::ProjectionDefinition;
use serde::{Serialize, de::DeserializeOwned};
use std::sync::Arc;

/// Build a projection definition from a pure fold. The persisted-state schema is the state's own
/// serde shape; an unchanged state keeps its `Arc` (the registry's reuse contract).
pub fn fold_definition<S>(
    key: &str,
    state_version: u64,
    init: fn() -> S,
    apply: fn(&S, &SessionEvent) -> Result<S, BoxError>,
) -> ProjectionDefinition<S>
where
    S: Serialize + DeserializeOwned + PartialEq + Send + Sync + 'static,
{
    ProjectionDefinition {
        key: key.to_owned(),
        state_schema: Arc::new(|raw| match raw {
            Some(value) => Ok(Arc::new(serde_json::from_value(value.clone())?)),
            None => Err("persisted projection state is missing".into()),
        }),
        init: Arc::new(move |_, _| Ok(Arc::new(init()))),
        apply: Arc::new(move |state, event| {
            let next = apply(state, event)?;
            Ok(if next == **state {
                state.clone()
            } else {
                Arc::new(next)
            })
        }),
        wire: None,
        state_version,
    }
}

/// Host projection of agent turn and step boundaries (`turnBoundary`, state version 2).
pub fn turn_boundary_projection_definition() -> ProjectionDefinition<TurnBoundaryProjection> {
    let mut definition = fold_definition(
        "turnBoundary",
        2,
        || TurnBoundaryProjection {
            open_turn_start_seq: None,
            last_step_start_seq: None,
            last_step_boundary: None,
            last_turn: 0,
        },
        |state, event| {
            let mut next = state.clone();
            match event.event_type() {
                "turn/start" => {
                    next.open_turn_start_seq = Some(event.seq());
                    next.last_turn = event
                        .data()
                        .get("turn")
                        .and_then(|t| t.as_u64())
                        .ok_or("turn/start without a numeric turn")?;
                }
                "turn/end" => next.open_turn_start_seq = None,
                "step/start" => {
                    next.last_step_start_seq = Some(event.seq());
                    next.last_step_boundary = Some(StepBoundary {
                        kind: StepBoundaryKind::Start,
                        seq: event.seq(),
                    });
                }
                "step/end" => {
                    next.last_step_boundary = Some(StepBoundary {
                        kind: StepBoundaryKind::End,
                        seq: event.seq(),
                    });
                }
                _ => {}
            }
            Ok(next)
        },
    );
    // `turnBoundaryProjectionSchema` of the source: all four keys are required (a nullable key
    // may be `null`, never absent); `serde` alone would read an absent `Option` key as `null`.
    definition.state_schema = Arc::new(|raw| {
        let value = raw.ok_or("persisted projection state is missing")?;
        let object = value
            .as_object()
            .ok_or("invalid turnBoundary projection: expected an object")?;
        for key in [
            "openTurnStartSeq",
            "lastStepStartSeq",
            "lastStepBoundary",
            "lastTurn",
        ] {
            if !object.contains_key(key) {
                return Err(format!("invalid turnBoundary projection: {key} is required").into());
            }
        }
        Ok(Arc::new(serde_json::from_value(value.clone())?))
    });
    definition
}
