use hoh_goal::{
    GoalOperation, apply_goal_change, apply_goal_event, decode_goal_change, empty_goal_fold_state,
    fold_goal,
};
use hoh_session::SessionEvent;
use serde_json::{Value, json};
use std::sync::Arc;

fn snapshot(id: &str, revision: u64, phase: &str) -> Value {
    let mut goal = json!({
        "id":id, "revision":revision, "objective":"ship", "phase":phase,
        "maxGoalRounds":3
    });
    if phase == "blocked" {
        goal["blockedReason"] = json!({"code":"needs-input","message":"ask owner"});
    }
    goal
}

fn change(operation: &str, goal: Value, rounds: u64, created: i64, updated: i64) -> Value {
    json!({
        "kind":"goal/change", "version":1, "operation":operation, "goal":goal,
        "roundsStarted":rounds, "createdAt":created, "updatedAt":updated
    })
}

fn clear(id: &str, revision: u64, at: i64) -> Value {
    json!({
        "kind":"goal/change", "version":1, "operation":"clear",
        "cleared":{"id":id,"revision":revision}, "clearedAt":at
    })
}

fn event(t: &str, seq: u64, data: Value) -> SessionEvent {
    SessionEvent::from_value(json!({"type":t,"seq":seq,"time":seq + 1,"data":data}))
}

fn apply(state: &mut hoh_goal::GoalFoldState, value: Value) -> Result<(), hoh_error::BoxError> {
    apply_goal_event(state, &event("goal/change", 0, value))
}

#[test]
fn decoder_ignores_unrelated_metadata() {
    assert!(
        decode_goal_change(&json!({"kind":"other"}))
            .unwrap()
            .is_none()
    );
    assert!(decode_goal_change(&json!(null)).unwrap().is_none());
}

#[test]
fn fold_ignores_unrelated_events_and_non_goal_sources() {
    let mut state = empty_goal_fold_state();
    apply_goal_event(&mut state, &event("turn/start", 0, json!({"turn":1}))).unwrap();
    apply_goal_event(
        &mut state,
        &event("user/message", 1, json!({"source":{"kind":"user"}})),
    )
    .unwrap();
    assert!(state.goal.is_none());
}

#[test]
fn decoder_rejects_unsupported_version() {
    let error = decode_goal_change(&json!({"kind":"goal/change","version":2})).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unsupported goal change version")
    );
}

#[test]
fn decoder_rejects_invalid_operation() {
    let mut value = change("create", snapshot("g", 1, "active"), 0, 1, 1);
    value["operation"] = json!("launch");
    assert!(
        decode_goal_change(&value)
            .unwrap_err()
            .to_string()
            .contains("operation is invalid")
    );
}

#[test]
fn decoder_rejects_extra_top_level_fields() {
    let mut value = change("create", snapshot("g", 1, "active"), 0, 1, 1);
    value["extra"] = json!(true);
    assert!(
        decode_goal_change(&value)
            .unwrap_err()
            .to_string()
            .contains("exactly")
    );
}

#[test]
fn valid_create_initializes_fold() {
    let mut state = empty_goal_fold_state();
    apply(
        &mut state,
        change("create", snapshot("g", 1, "active"), 0, 1, 1),
    )
    .unwrap();
    assert_eq!(state.goal.as_ref().unwrap().id.as_str(), "g");
    assert_eq!(state.last_ref.as_ref().unwrap().revision, 1);
}

#[test]
fn create_requires_fresh_active_revision_one_and_zero_rounds() {
    for value in [
        change("create", snapshot("g", 2, "active"), 0, 1, 1),
        change("create", snapshot("g", 1, "paused"), 0, 1, 1),
        change("create", snapshot("g", 1, "active"), 1, 1, 1),
    ] {
        assert!(apply(&mut empty_goal_fold_state(), value).is_err());
    }
}

#[test]
fn mutations_require_a_current_goal() {
    for operation in ["edit", "pause", "resume", "complete", "block"] {
        assert!(
            apply(
                &mut empty_goal_fold_state(),
                change(operation, snapshot("g", 2, "active"), 0, 1, 2)
            )
            .is_err()
        );
    }
}

#[test]
fn mutations_reject_stale_identity_or_revision() {
    let mut state = empty_goal_fold_state();
    apply(
        &mut state,
        change("create", snapshot("g", 1, "active"), 0, 1, 1),
    )
    .unwrap();
    assert!(
        apply(
            &mut state.clone(),
            change("edit", snapshot("other", 2, "active"), 0, 1, 2)
        )
        .is_err()
    );
    assert!(
        apply(
            &mut state,
            change("edit", snapshot("g", 3, "active"), 0, 1, 2)
        )
        .is_err()
    );
}

#[test]
fn mutations_preserve_counters_and_monotonic_timestamps() {
    let mut state = empty_goal_fold_state();
    apply(
        &mut state,
        change("create", snapshot("g", 1, "active"), 0, 10, 10),
    )
    .unwrap();
    assert!(
        apply(
            &mut state.clone(),
            change("edit", snapshot("g", 2, "active"), 1, 10, 11)
        )
        .is_err()
    );
    assert!(
        apply(
            &mut state.clone(),
            change("edit", snapshot("g", 2, "active"), 0, 9, 11)
        )
        .is_err()
    );
    assert!(
        apply(
            &mut state,
            change("edit", snapshot("g", 2, "active"), 0, 10, 9)
        )
        .is_err()
    );
}

#[test]
fn lifecycle_mutations_cannot_change_definition() {
    let mut state = empty_goal_fold_state();
    apply(
        &mut state,
        change("create", snapshot("g", 1, "active"), 0, 1, 1),
    )
    .unwrap();
    let mut next = snapshot("g", 2, "paused");
    next["objective"] = json!("different");
    assert!(
        apply(&mut state, change("pause", next, 0, 1, 2))
            .unwrap_err()
            .to_string()
            .contains("cannot change")
    );
}

#[test]
fn replay_rejects_invalid_lifecycle_phase_transitions() {
    let mut active = empty_goal_fold_state();
    apply(
        &mut active,
        change("create", snapshot("g", 1, "active"), 0, 1, 1),
    )
    .unwrap();
    assert!(
        apply(
            &mut active.clone(),
            change("pause", snapshot("g", 2, "complete"), 0, 1, 2)
        )
        .is_err()
    );
    assert!(
        apply(
            &mut active.clone(),
            change("block", snapshot("g", 2, "paused"), 0, 1, 2)
        )
        .is_err()
    );
    assert!(
        apply(
            &mut active,
            change("complete", snapshot("g", 2, "active"), 0, 1, 2)
        )
        .is_err()
    );
}

#[test]
fn clear_requires_current_contiguous_ref_and_timestamp() {
    assert!(apply(&mut empty_goal_fold_state(), clear("g", 2, 2)).is_err());
    let mut state = empty_goal_fold_state();
    apply(
        &mut state,
        change("create", snapshot("g", 1, "active"), 0, 10, 10),
    )
    .unwrap();
    assert!(apply(&mut state.clone(), clear("g", 3, 11)).is_err());
    assert!(apply(&mut state.clone(), clear("other", 2, 11)).is_err());
    assert!(apply(&mut state, clear("g", 2, 9)).is_err());
}

#[test]
fn cleared_goal_id_cannot_be_reused() {
    let mut state = empty_goal_fold_state();
    apply(
        &mut state,
        change("create", snapshot("g", 1, "active"), 0, 1, 1),
    )
    .unwrap();
    apply(&mut state, clear("g", 2, 2)).unwrap();
    assert!(
        apply(
            &mut state,
            change("create", snapshot("g", 1, "active"), 0, 3, 3)
        )
        .is_err()
    );
}

#[test]
fn round_must_belong_to_current_goal_and_revision() {
    let mut state = empty_goal_fold_state();
    apply(
        &mut state,
        change("create", snapshot("g", 1, "active"), 0, 1, 1),
    )
    .unwrap();
    for source in [
        json!({"kind":"goal","goalId":"other","revision":1,"round":1}),
        json!({"kind":"goal","goalId":"g","revision":2,"round":1}),
    ] {
        assert!(
            apply_goal_event(
                &mut state.clone(),
                &event("user/message", 1, json!({"source":source}))
            )
            .is_err()
        );
    }
}

#[test]
fn round_sources_are_positive_sequential_and_bounded() {
    let mut state = empty_goal_fold_state();
    apply(
        &mut state,
        change("create", snapshot("g", 1, "active"), 0, 1, 1),
    )
    .unwrap();
    for round in [0, 2, 4] {
        let message = json!({"source":{"kind":"goal","goalId":"g","revision":1,"round":round}});
        assert!(apply_goal_event(&mut state.clone(), &event("user/message", 1, message)).is_err());
    }
    apply_goal_event(
        &mut state,
        &event(
            "user/message",
            1,
            json!({
                "source":{"kind":"goal","goalId":"g","revision":1,"round":1}
            }),
        ),
    )
    .unwrap();
    assert_eq!(state.rounds_started, 1);
}

#[test]
fn decoder_rejects_malformed_snapshots_refs_counters_and_timestamps() {
    let mut malformed = vec![];
    let mut empty_id = snapshot("g", 1, "active");
    empty_id["id"] = json!("");
    malformed.push(change("create", empty_id, 0, 1, 1));
    let mut bad_objective = snapshot("g", 1, "active");
    bad_objective["objective"] = json!(" x ");
    malformed.push(change("create", bad_objective, 0, 1, 1));
    malformed.push(change("create", snapshot("g", 1, "active"), 0, 2, 1));
    malformed.push(json!({"kind":"goal/change","version":1,"operation":"clear","cleared":{"id":"g","revision":0},"clearedAt":1}));
    for value in malformed {
        assert!(decode_goal_change(&value).is_err());
    }
}

#[test]
fn decoder_rejects_every_noncanonical_snapshot_and_tombstone_shape() {
    let mut malformed = Vec::new();

    let mut extra = snapshot("g", 1, "active");
    extra["extra"] = json!(true);
    malformed.push(change("create", extra, 0, 1, 1));

    let mut phase = snapshot("g", 1, "active");
    phase["phase"] = json!("unknown");
    malformed.push(change("create", phase, 0, 1, 1));

    let mut non_blocked_reason = snapshot("g", 1, "active");
    non_blocked_reason["blockedReason"] = json!({"code":"x","message":"x"});
    malformed.push(change("create", non_blocked_reason, 0, 1, 1));

    let mut blocked_without_reason = snapshot("g", 1, "blocked");
    blocked_without_reason
        .as_object_mut()
        .unwrap()
        .remove("blockedReason");
    malformed.push(change("create", blocked_without_reason, 0, 1, 1));

    let mut blocked_null = snapshot("g", 1, "blocked");
    blocked_null["blockedReason"] = Value::Null;
    malformed.push(change("create", blocked_null, 0, 1, 1));

    let mut reason_extra = snapshot("g", 1, "blocked");
    reason_extra["blockedReason"]["extra"] = json!(true);
    malformed.push(change("create", reason_extra, 0, 1, 1));

    let mut reason_code = snapshot("g", 1, "blocked");
    reason_code["blockedReason"]["code"] = json!("NOT_CANONICAL");
    malformed.push(change("create", reason_code, 0, 1, 1));

    let mut reason_message = snapshot("g", 1, "blocked");
    reason_message["blockedReason"]["message"] = json!(" spaced ");
    malformed.push(change("create", reason_message, 0, 1, 1));

    let mut zero_revision = snapshot("g", 1, "active");
    zero_revision["revision"] = json!(0);
    malformed.push(change("create", zero_revision, 0, 1, 1));

    let mut negative_cap = snapshot("g", 1, "active");
    negative_cap["maxGoalRounds"] = json!(-1);
    malformed.push(change("create", negative_cap, 0, 1, 1));

    malformed.push(json!({
        "kind":"goal/change","version":1,"operation":"create",
        "goal":snapshot("g", 1, "active"),"roundsStarted":-1,"createdAt":1,"updatedAt":1
    }));
    malformed.push(json!({
        "kind":"goal/change","version":1,"operation":"create",
        "goal":snapshot("g", 1, "active"),"roundsStarted":0,"createdAt":-1,"updatedAt":1
    }));
    malformed.push(json!({
        "kind":"goal/change","version":1,"operation":"clear","cleared":null,"clearedAt":1
    }));
    malformed.push(clear("", 2, 1));

    for value in malformed {
        assert!(decode_goal_change(&value).is_err(), "accepted {value}");
    }
}

#[test]
fn decoder_uses_exact_typescript_error_text_and_validation_order() {
    let too_large = hoh_session::MAX_SAFE_INTEGER + 1;
    let mut revision = snapshot("g", 1, "active");
    revision["revision"] = json!(too_large);
    assert_eq!(
        decode_goal_change(&change("create", revision, 0, 1, 1))
            .unwrap_err()
            .to_string(),
        "goal change goal.revision must be a positive safe integer"
    );
    let mut rounds = change("create", snapshot("g", 1, "active"), 0, 1, 1);
    rounds["roundsStarted"] = json!(too_large);
    assert_eq!(
        decode_goal_change(&rounds).unwrap_err().to_string(),
        "goal change roundsStarted must be a non-negative safe integer"
    );
    assert_eq!(
        decode_goal_change(&json!({"kind":"goal/change","version":"2"}))
            .unwrap_err()
            .to_string(),
        "unsupported goal change version 2"
    );
    let mut blocked = snapshot("g", 1, "blocked");
    blocked["maxGoalRounds"] = json!(0);
    blocked["blockedReason"] = Value::Null;
    assert_eq!(
        decode_goal_change(&change("create", blocked, 0, 1, 1))
            .unwrap_err()
            .to_string(),
        "goal change goal.maxGoalRounds must be a positive safe integer"
    );
}

#[test]
fn decoder_uses_javascript_trim_semantics() {
    let mut next_line = snapshot("g", 1, "active");
    next_line["objective"] = json!("\u{85}x");
    assert!(decode_goal_change(&change("create", next_line, 0, 1, 1)).is_ok());

    let mut bom = snapshot("g", 1, "active");
    bom["objective"] = json!("\u{feff}x");
    assert_eq!(
        decode_goal_change(&change("create", bom, 0, 1, 1))
            .unwrap_err()
            .to_string(),
        "goal change goal.objective must be non-empty and normalized"
    );
}

#[test]
fn replay_rejects_exhausted_resume_edit_phase_changes_and_bad_sources() {
    let mut exhausted = empty_goal_fold_state();
    let mut bounded = snapshot("g", 1, "active");
    bounded["maxGoalRounds"] = json!(1);
    apply(&mut exhausted, change("create", bounded, 0, 1, 1)).unwrap();
    apply_goal_event(
        &mut exhausted,
        &event(
            "user/message",
            1,
            json!({"source":{"kind":"goal","goalId":"g","revision":1,"round":1}}),
        ),
    )
    .unwrap();
    assert!(
        apply(&mut exhausted, {
            let mut next = snapshot("g", 2, "active");
            next["maxGoalRounds"] = json!(1);
            change("resume", next, 1, 1, 2)
        })
        .unwrap_err()
        .to_string()
        .contains("exhausted round budget")
    );

    let mut edited = empty_goal_fold_state();
    apply(
        &mut edited,
        change("create", snapshot("g", 1, "active"), 0, 1, 1),
    )
    .unwrap();
    assert_eq!(
        apply(
            &mut edited,
            change("edit", snapshot("g", 2, "paused"), 0, 1, 2)
        )
        .unwrap_err()
        .to_string(),
        "goal edit cannot change phase or blocked reason"
    );

    assert_eq!(
        apply_goal_event(
            &mut edited,
            &event(
                "user/message",
                2,
                json!({"source":{"kind":"goal","goalId":"","revision":1,"round":1}}),
            ),
        )
        .unwrap_err()
        .to_string(),
        "goal message source is invalid"
    );
}

#[test]
fn completed_and_nonadjacent_goal_ids_can_never_be_reused() {
    let mut state = empty_goal_fold_state();
    apply(
        &mut state,
        change("create", snapshot("g", 1, "active"), 0, 1, 1),
    )
    .unwrap();
    apply(
        &mut state,
        change("complete", snapshot("g", 2, "complete"), 0, 1, 2),
    )
    .unwrap();
    apply(
        &mut state,
        change("create", snapshot("h", 1, "active"), 0, 3, 3),
    )
    .unwrap();
    apply(
        &mut state,
        change("complete", snapshot("h", 2, "complete"), 0, 3, 4),
    )
    .unwrap();
    assert!(
        apply(
            &mut state,
            change("create", snapshot("g", 1, "active"), 0, 5, 5)
        )
        .is_err()
    );
}

#[test]
fn clear_requires_its_exact_canonical_field_set() {
    let mut value = clear("g", 2, 2);
    value["extra"] = json!(true);
    assert_eq!(
        decode_goal_change(&value).unwrap_err().to_string(),
        "goal clear change must have exactly cleared,clearedAt,kind,operation,version fields"
    );
}

#[test]
fn fold_applies_clear_tombstone_after_snapshot() {
    let events = vec![
        Arc::new(event(
            "goal/change",
            0,
            change("create", snapshot("g", 1, "active"), 0, 1, 1),
        )),
        Arc::new(event("goal/change", 1, clear("g", 2, 2))),
    ];
    let folded = fold_goal(&events).unwrap();
    assert!(folded.goal.is_none());
    assert_eq!(folded.last_ref.unwrap().revision, 2);
}

#[test]
fn decoded_operation_is_preserved() {
    let decoded = decode_goal_change(&change("create", snapshot("g", 1, "active"), 0, 1, 1))
        .unwrap()
        .unwrap();
    assert_eq!(decoded.operation(), GoalOperation::Create);
    let mut state = empty_goal_fold_state();
    apply_goal_change(&mut state, &decoded).unwrap();
    assert!(state.goal.is_some());
}
