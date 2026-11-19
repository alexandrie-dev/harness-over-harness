use crate::{apply_goal_event, types::*};
use hoh_error::BoxError;
use hoh_session::{MAX_SAFE_INTEGER, SessionEvent, SessionLogOffset};
use hoh_session_projection::{ProjectionDefinition, ProjectionWire};
use indexmap::IndexSet;
use serde_json::Value;
use std::sync::Arc;

fn error(message: impl Into<String>) -> BoxError {
    message.into().into()
}

fn validate_projection(current: &GoalProjection) -> Result<(), BoxError> {
    let goal = &current.goal;
    if goal.id.as_str().is_empty() {
        return Err(error("goal projection id must be non-empty"));
    }
    if goal.revision == 0 || goal.revision > MAX_SAFE_INTEGER {
        return Err(error(
            "goal projection revision must be a positive safe integer",
        ));
    }
    if goal.objective.is_empty() {
        return Err(error("goal projection objective must be non-empty"));
    }
    if goal.max_goal_rounds == 0 || goal.max_goal_rounds > MAX_SAFE_INTEGER {
        return Err(error(
            "goal projection maxGoalRounds must be a positive safe integer",
        ));
    }
    Ok(())
}

fn validate_state(state: &GoalProjectionState) -> Result<(), BoxError> {
    if state.failure.as_ref().is_some_and(String::is_empty) {
        return Err(error("goal projection failure must be null or non-empty"));
    }
    let seen = state.seen_goal_ids.iter().cloned().collect::<IndexSet<_>>();
    if state.seen_goal_ids.iter().any(|id| id.as_str().is_empty()) {
        return Err(error("seen goal ids must be non-empty"));
    }
    if seen.len() != state.seen_goal_ids.len() {
        return Err(error("seen goal ids must be unique"));
    }
    if let Some(current) = &state.current {
        validate_projection(current)?;
        if !seen.contains(&current.goal.id) {
            return Err(error(
                "current goal id must be retained among seen goal ids",
            ));
        }
        if current.updated_at < current.created_at {
            return Err(error("current goal update cannot precede its creation"));
        }
        if current.rounds_started > current.goal.max_goal_rounds {
            return Err(error(
                "current goal rounds cannot exceed its configured limit",
            ));
        }
    }
    Ok(())
}

fn parse_state(value: Option<&Value>) -> Result<Arc<GoalProjectionState>, BoxError> {
    let value = value.ok_or_else(|| error("goal projection state is required"))?;
    let object = value
        .as_object()
        .ok_or_else(|| error("goal projection state must be a record"))?;
    if object.len() != 3
        || !["current", "seenGoalIds", "failure"]
            .iter()
            .all(|key| object.contains_key(*key))
    {
        return Err(error(
            "goal projection state must have exactly current, seenGoalIds, failure fields",
        ));
    }
    let state: GoalProjectionState = serde_json::from_value(value.clone())?;
    validate_state(&state)?;
    Ok(Arc::new(state))
}

fn fold_state(state: &GoalProjectionState) -> GoalFoldState {
    GoalFoldState {
        goal: state.current.as_ref().map(|current| current.goal.clone()),
        rounds_started: state
            .current
            .as_ref()
            .map_or(0, |current| current.rounds_started),
        created_at: state.current.as_ref().map(|current| current.created_at),
        updated_at: state.current.as_ref().map(|current| current.updated_at),
        last_ref: None,
        seen_goal_ids: state.seen_goal_ids.iter().cloned().collect(),
    }
}

fn projection_state(state: GoalFoldState) -> Result<GoalProjectionState, BoxError> {
    let current = match state.goal {
        Some(goal) => Some(GoalProjection {
            goal,
            rounds_started: state.rounds_started,
            created_at: state
                .created_at
                .ok_or_else(|| error("current goal fold lacks timestamps"))?,
            updated_at: state
                .updated_at
                .ok_or_else(|| error("current goal fold lacks timestamps"))?,
        }),
        None => None,
    };
    Ok(GoalProjectionState {
        current,
        seen_goal_ids: state.seen_goal_ids.into_iter().collect(),
        failure: None,
    })
}

pub fn apply_goal_projection(
    state: &Arc<GoalProjectionState>,
    event: &SessionEvent,
) -> Arc<GoalProjectionState> {
    if state.failure.is_some()
        || (event.event_type() != "goal/change"
            && (event.event_type() != "user/message"
                || event.data().pointer("/source/kind").and_then(Value::as_str) != Some("goal")))
    {
        return state.clone();
    }
    let mut folded = fold_state(state);
    match apply_goal_event(&mut folded, event).and_then(|()| projection_state(folded)) {
        Ok(next) => Arc::new(next),
        Err(cause) => Arc::new(GoalProjectionState {
            current: state.current.clone(),
            seen_goal_ids: state.seen_goal_ids.clone(),
            failure: Some(format!(
                "goal replay failed at session event {}: {cause}",
                event.seq()
            )),
        }),
    }
}

fn view_schema(value: Option<&Value>) -> Result<Arc<Value>, BoxError> {
    let value = value.ok_or_else(|| error("goal projection view is required"))?;
    if !value.is_null() {
        let current: GoalProjection = serde_json::from_value(value.clone())?;
        validate_projection(&current)?;
    }
    Ok(Arc::new(value.clone()))
}

pub fn goal_projection_definition() -> ProjectionDefinition<GoalProjectionState> {
    ProjectionDefinition {
        key: "goal".into(),
        state_schema: Arc::new(parse_state),
        init: Arc::new(|_, _: SessionLogOffset| Ok(Arc::new(GoalProjectionState::default()))),
        apply: Arc::new(|state, event| Ok(apply_goal_projection(state, event))),
        wire: Some(ProjectionWire::new(
            Arc::new(view_schema),
            Arc::new(|state| {
                Ok(Some(Arc::new(match &state.current {
                    Some(current) => serde_json::to_value(current)?,
                    None => Value::Null,
                })))
            }),
        )),
        state_version: 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_goal_fold_without_timestamps_is_rejected() {
        let mut folded = empty_goal_fold_state();
        folded.goal = Some(GoalSnapshot {
            id: "g".into(),
            revision: 1,
            objective: "x".into(),
            phase: GoalPhase::Active,
            blocked_reason: None,
            max_goal_rounds: 1,
        });
        let error = projection_state(folded).unwrap_err();
        assert_eq!(error.to_string(), "current goal fold lacks timestamps");
    }
}
