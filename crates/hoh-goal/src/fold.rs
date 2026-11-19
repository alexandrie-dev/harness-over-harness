use crate::types::*;
use hoh_error::BoxError;
use hoh_session::{MAX_SAFE_INTEGER, SessionEvent};
use serde_json::{Map, Value};
use std::sync::Arc;

fn error(message: impl Into<String>) -> BoxError {
    message.into().into()
}

fn object(value: &Value) -> Result<&Map<String, Value>, BoxError> {
    value
        .as_object()
        .ok_or_else(|| error("goal change goal must be a record"))
}

fn exact_keys(object: &Map<String, Value>, expected: &[&str]) -> bool {
    object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key))
}

fn safe_u64(value: &Value, field: &str, positive: bool) -> Result<u64, BoxError> {
    let number = value.as_u64().or_else(|| {
        let number = value.as_f64()?;
        (number.is_finite()
            && number.fract() == 0.0
            && number >= 0.0
            && number <= MAX_SAFE_INTEGER as f64)
            .then_some(number as u64)
    });
    let Some(number) = number.filter(|number| !positive || *number > 0) else {
        let adjective = if positive { "positive" } else { "non-negative" };
        return Err(error(format!(
            "goal change {field} must be a {adjective} safe integer"
        )));
    };
    if number > MAX_SAFE_INTEGER {
        let adjective = if positive { "positive" } else { "non-negative" };
        return Err(error(format!(
            "goal change {field} must be a {adjective} safe integer"
        )));
    }
    Ok(number)
}

fn safe_i64(value: &Value, field: &str) -> Result<i64, BoxError> {
    let number = value.as_i64().or_else(|| {
        let number = value.as_f64()?;
        (number.is_finite()
            && number.fract() == 0.0
            && number >= 0.0
            && number <= MAX_SAFE_INTEGER as f64)
            .then_some(number as i64)
    });
    match number {
        Some(number) if number >= 0 && number.unsigned_abs() <= MAX_SAFE_INTEGER => Ok(number),
        _ => Err(error(format!(
            "goal change {field} must be a non-negative safe integer"
        ))),
    }
}

fn lower_kebab(value: &str) -> bool {
    let mut parts = value.split('-');
    let Some(first) = parts.next() else {
        return false;
    };
    let valid_first = !first.is_empty()
        && first.as_bytes()[0].is_ascii_lowercase()
        && first
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
    valid_first
        && parts.all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

fn decode_block_reason(value: &Value) -> Result<GoalBlockReason, BoxError> {
    let object = value.as_object().ok_or_else(|| {
        error("goal change goal.blockedReason must have exactly code and message fields")
    })?;
    if !exact_keys(object, &["code", "message"]) {
        return Err(error(
            "goal change goal.blockedReason must have exactly code and message fields",
        ));
    }
    let code = object.get("code").and_then(Value::as_str).unwrap_or("");
    if !lower_kebab(code) {
        return Err(error(
            "goal change goal.blockedReason.code must be lower-kebab-case",
        ));
    }
    let message = object.get("message").and_then(Value::as_str).unwrap_or("");
    let normalized = hoh_util_values::js_trim(message);
    if normalized.is_empty() || normalized != message {
        return Err(error(
            "goal change goal.blockedReason.message must be non-empty and normalized",
        ));
    }
    Ok(GoalBlockReason {
        code: code.to_owned(),
        message: message.to_owned(),
    })
}

fn decode_snapshot(value: &Value) -> Result<GoalSnapshot, BoxError> {
    let object = object(value)?;
    let id = object.get("id").and_then(Value::as_str).unwrap_or("");
    if id.is_empty() {
        return Err(error("goal change goal.id must be a non-empty string"));
    }
    let objective = object
        .get("objective")
        .and_then(Value::as_str)
        .unwrap_or("");
    let normalized = hoh_util_values::js_trim(objective);
    if normalized.is_empty() || normalized != objective {
        return Err(error(
            "goal change goal.objective must be non-empty and normalized",
        ));
    }
    let phase = match object.get("phase").and_then(Value::as_str) {
        Some("active") => GoalPhase::Active,
        Some("paused") => GoalPhase::Paused,
        Some("blocked") => GoalPhase::Blocked,
        Some("complete") => GoalPhase::Complete,
        _ => return Err(error("goal change goal.phase is invalid")),
    };
    let keys = if phase == GoalPhase::Blocked {
        &[
            "blockedReason",
            "id",
            "maxGoalRounds",
            "objective",
            "phase",
            "revision",
        ][..]
    } else {
        &["id", "maxGoalRounds", "objective", "phase", "revision"][..]
    };
    if !exact_keys(object, keys) {
        return Err(error(format!(
            "goal change goal for phase {phase} must have exactly {} fields",
            keys.join(",")
        )));
    }
    let revision = safe_u64(&object["revision"], "goal.revision", true)?;
    let max_goal_rounds = safe_u64(&object["maxGoalRounds"], "goal.maxGoalRounds", true)?;
    Ok(GoalSnapshot {
        id: id.to_owned().into(),
        revision,
        objective: objective.to_owned(),
        phase,
        blocked_reason: if phase == GoalPhase::Blocked {
            Some(decode_block_reason(&object["blockedReason"])?)
        } else {
            None
        },
        max_goal_rounds,
    })
}

fn decode_ref(value: &Value) -> Result<GoalRef, BoxError> {
    let Some(object) = value.as_object() else {
        return Err(error(
            "goal clear tombstone must have exactly id and revision fields",
        ));
    };
    if !exact_keys(object, &["id", "revision"]) {
        return Err(error(
            "goal clear tombstone must have exactly id and revision fields",
        ));
    }
    let id = object.get("id").and_then(Value::as_str).unwrap_or("");
    if id.is_empty() {
        return Err(error("goal clear tombstone id must be a non-empty string"));
    }
    Ok(GoalRef {
        id: id.to_owned().into(),
        revision: safe_u64(&object["revision"], "cleared.revision", true)?,
    })
}

pub fn decode_goal_change(value: &Value) -> Result<Option<GoalChangeMeta>, BoxError> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    if object.get("kind").and_then(Value::as_str) != Some("goal/change") {
        return Ok(None);
    }
    let supported_version = object.get("version").is_some_and(|version| {
        version.as_u64() == Some(GOAL_CHANGE_VERSION)
            || version.as_f64() == Some(GOAL_CHANGE_VERSION as f64)
    });
    if !supported_version {
        return Err(error(format!(
            "unsupported goal change version {}",
            object
                .get("version")
                .map_or_else(|| "undefined".to_owned(), hoh_util_values::js_string)
        )));
    }
    let operation = object.get("operation").and_then(Value::as_str);
    if operation == Some("clear") {
        let keys = ["cleared", "clearedAt", "kind", "operation", "version"];
        if !exact_keys(object, &keys) {
            return Err(error(format!(
                "goal clear change must have exactly {} fields",
                keys.join(",")
            )));
        }
        return Ok(Some(GoalChangeMeta::Clear(GoalClearChangeMeta {
            kind: "goal/change".into(),
            version: GOAL_CHANGE_VERSION,
            operation: GoalOperation::Clear,
            cleared: decode_ref(&object["cleared"])?,
            cleared_at: safe_i64(&object["clearedAt"], "clearedAt")?,
        })));
    }
    let operation = match operation {
        Some("create") => GoalOperation::Create,
        Some("edit") => GoalOperation::Edit,
        Some("pause") => GoalOperation::Pause,
        Some("resume") => GoalOperation::Resume,
        Some("complete") => GoalOperation::Complete,
        Some("block") => GoalOperation::Block,
        _ => return Err(error("goal change operation is invalid")),
    };
    let keys = [
        "createdAt",
        "goal",
        "kind",
        "operation",
        "roundsStarted",
        "updatedAt",
        "version",
    ];
    if !exact_keys(object, &keys) {
        return Err(error(format!(
            "goal snapshot change must have exactly {} fields",
            keys.join(",")
        )));
    }
    let created_at = safe_i64(&object["createdAt"], "createdAt")?;
    let updated_at = safe_i64(&object["updatedAt"], "updatedAt")?;
    if updated_at < created_at {
        return Err(error("goal change updatedAt cannot precede createdAt"));
    }
    Ok(Some(GoalChangeMeta::Snapshot(GoalSnapshotChangeMeta {
        kind: "goal/change".into(),
        version: GOAL_CHANGE_VERSION,
        operation,
        goal: decode_snapshot(&object["goal"])?,
        rounds_started: safe_u64(&object["roundsStarted"], "roundsStarted", false)?,
        created_at,
        updated_at,
    })))
}

pub fn goal_change_ref(change: &GoalChangeMeta) -> GoalRef {
    match change {
        GoalChangeMeta::Snapshot(change) => GoalRef {
            id: change.goal.id.clone(),
            revision: change.goal.revision,
        },
        GoalChangeMeta::Clear(change) => change.cleared.clone(),
    }
}

fn require_next_revision(
    current: &GoalSnapshot,
    next: &GoalRef,
    operation: GoalOperation,
) -> Result<(), BoxError> {
    if next.id != current.id || next.revision != current.revision + 1 {
        return Err(error(format!(
            "goal {operation} must advance the current goal by one revision"
        )));
    }
    Ok(())
}

fn require_same_definition(
    current: &GoalSnapshot,
    next: &GoalSnapshot,
    operation: GoalOperation,
) -> Result<(), BoxError> {
    if next.objective != current.objective || next.max_goal_rounds != current.max_goal_rounds {
        return Err(error(format!(
            "goal {operation} cannot change objective or maxGoalRounds"
        )));
    }
    Ok(())
}

fn validate_transition(
    state: &GoalFoldState,
    change: &GoalSnapshotChangeMeta,
    current: &GoalSnapshot,
) -> Result<(), BoxError> {
    let next = &change.goal;
    require_next_revision(
        current,
        &GoalRef {
            id: next.id.clone(),
            revision: next.revision,
        },
        change.operation,
    )?;
    let updated_at = state
        .updated_at
        .ok_or_else(|| error("current goal fold lacks updatedAt"))?;
    if Some(change.created_at) != state.created_at
        || change.updated_at < updated_at
        || change.rounds_started != state.rounds_started
    {
        return Err(error(format!(
            "goal {} does not preserve the current counters and timestamps",
            change.operation
        )));
    }
    match change.operation {
        GoalOperation::Edit => {
            if next.phase != current.phase || next.blocked_reason != current.blocked_reason {
                return Err(error("goal edit cannot change phase or blocked reason"));
            }
        }
        GoalOperation::Pause => {
            require_same_definition(current, next, change.operation)?;
            if current.phase != GoalPhase::Active || next.phase != GoalPhase::Paused {
                return Err(error("goal pause has an invalid phase transition"));
            }
        }
        GoalOperation::Resume => {
            require_same_definition(current, next, change.operation)?;
            if !matches!(
                current.phase,
                GoalPhase::Active | GoalPhase::Paused | GoalPhase::Blocked
            ) || next.phase != GoalPhase::Active
                || state.rounds_started >= next.max_goal_rounds
            {
                return Err(error(
                    "goal resume has an invalid phase transition or exhausted round budget",
                ));
            }
        }
        GoalOperation::Complete => {
            require_same_definition(current, next, change.operation)?;
            if current.phase == GoalPhase::Complete || next.phase != GoalPhase::Complete {
                return Err(error("goal complete has an invalid phase transition"));
            }
        }
        GoalOperation::Block => {
            require_same_definition(current, next, change.operation)?;
            if current.phase != GoalPhase::Active || next.phase != GoalPhase::Blocked {
                return Err(error("goal block has an invalid phase transition"));
            }
        }
        GoalOperation::Create | GoalOperation::Clear => {
            return Err(error(
                "goal create cannot be validated as a current-goal transition",
            ));
        }
    }
    Ok(())
}

pub fn apply_goal_change(
    state: &mut GoalFoldState,
    change: &GoalChangeMeta,
) -> Result<(), BoxError> {
    let reference = goal_change_ref(change);
    match change {
        GoalChangeMeta::Clear(change) => {
            let current = state
                .goal
                .as_ref()
                .ok_or_else(|| error("goal clear requires a current goal"))?;
            require_next_revision(current, &change.cleared, GoalOperation::Clear)?;
            let updated_at = state
                .updated_at
                .ok_or_else(|| error("current goal fold lacks updatedAt"))?;
            if change.cleared_at < updated_at {
                return Err(error(
                    "goal clear timestamp cannot precede the current goal update",
                ));
            }
            state.goal = None;
            state.rounds_started = 0;
            state.created_at = None;
            state.updated_at = None;
            state.last_ref = Some(reference);
        }
        GoalChangeMeta::Snapshot(change) => {
            if change.operation == GoalOperation::Create {
                if change.goal.revision != 1
                    || change.goal.phase != GoalPhase::Active
                    || change.rounds_started != 0
                    || state
                        .goal
                        .as_ref()
                        .is_some_and(|goal| goal.phase != GoalPhase::Complete)
                    || state.seen_goal_ids.contains(&change.goal.id)
                {
                    return Err(error(
                        "goal create requires a fresh active revision-one goal with zero rounds",
                    ));
                }
                state.seen_goal_ids.insert(change.goal.id.clone());
            } else {
                let current = state.goal.as_ref().ok_or_else(|| {
                    error(format!("goal {} requires a current goal", change.operation))
                })?;
                validate_transition(state, change, current)?;
            }
            state.goal = Some(change.goal.clone());
            state.rounds_started = change.rounds_started;
            state.created_at = Some(change.created_at);
            state.updated_at = Some(change.updated_at);
            state.last_ref = Some(reference);
        }
    }
    Ok(())
}

fn goal_source(value: &Value) -> Result<Option<GoalMessageSource>, BoxError> {
    let Some(source) = value.as_object() else {
        return Ok(None);
    };
    if source.get("kind").and_then(Value::as_str) != Some("goal") {
        return Ok(None);
    }
    let id = source.get("goalId").and_then(Value::as_str).unwrap_or("");
    let revision = source
        .get("revision")
        .map(|value| safe_u64(value, "source.revision", true));
    let round = source
        .get("round")
        .map(|value| safe_u64(value, "source.round", true));
    match (id.is_empty(), revision, round) {
        (false, Some(Ok(revision)), Some(Ok(round))) => Ok(Some(GoalMessageSource {
            goal_id: id.to_owned().into(),
            revision,
            round,
        })),
        _ => Err(error("goal message source is invalid")),
    }
}

pub fn apply_goal_event(state: &mut GoalFoldState, event: &SessionEvent) -> Result<(), BoxError> {
    if event.event_type() == "goal/change" {
        let change = decode_goal_change(event.data())?.ok_or_else(|| {
            error(format!(
                "goal change at session event {} has an invalid kind",
                event.seq()
            ))
        })?;
        return apply_goal_change(state, &change);
    }
    if event.event_type() != "user/message" {
        return Ok(());
    }
    let Some(source) = goal_source(&event.data()["source"])? else {
        return Ok(());
    };
    let current = state.goal.as_ref();
    if current.is_none_or(|goal| {
        goal.phase != GoalPhase::Active
            || source.goal_id != goal.id
            || source.revision != goal.revision
            || source.round != state.rounds_started + 1
            || source.round > goal.max_goal_rounds
    }) {
        return Err(error(format!(
            "goal round at session event {} is not the next admitted round of the active goal",
            event.seq()
        )));
    }
    state.rounds_started = source.round;
    Ok(())
}

pub fn fold_goal(events: &[Arc<SessionEvent>]) -> Result<FoldedGoal, BoxError> {
    let mut state = empty_goal_fold_state();
    for event in events {
        apply_goal_event(&mut state, event)?;
    }
    Ok(FoldedGoal {
        goal: state.goal,
        rounds_started: state.rounds_started,
        created_at: state.created_at,
        updated_at: state.updated_at,
        last_ref: state.last_ref,
    })
}
