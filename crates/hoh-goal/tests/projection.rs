use hoh_goal::{
    GoalId, GoalPhase, GoalProjection, GoalProjectionState, GoalSnapshot, apply_goal_projection,
    goal_projection_definition,
};
use hoh_session::SessionEvent;
use serde_json::json;
use std::sync::Arc;

fn event(t: &str, seq: u64, data: serde_json::Value) -> SessionEvent {
    SessionEvent::from_value(json!({"type":t,"seq":seq,"time":seq + 1,"data":data}))
}

fn state() -> Arc<GoalProjectionState> {
    Arc::new(GoalProjectionState {
        current: Some(GoalProjection {
            goal: GoalSnapshot {
                id: GoalId::from("g1"),
                revision: 1,
                objective: "x".into(),
                phase: GoalPhase::Active,
                blocked_reason: None,
                max_goal_rounds: 4,
            },
            rounds_started: 0,
            created_at: 1,
            updated_at: 1,
        }),
        seen_goal_ids: vec![GoalId::from("g1")],
        failure: None,
    })
}

#[test]
fn definition_has_stable_key_and_version() {
    let definition = goal_projection_definition();
    assert_eq!(definition.key, "goal");
    assert_eq!(definition.state_version, 6);
    assert!(definition.wire.is_some());
}

#[test]
fn state_schema_accepts_valid_and_rejects_invalid_invariants() {
    let definition = goal_projection_definition();
    let valid = serde_json::to_value(state().as_ref()).unwrap();
    assert!((definition.state_schema)(Some(&valid)).is_ok());
    for invalid in [
        json!({"current":valid["current"],"seenGoalIds":[],"failure":null}),
        json!({"current":valid["current"],"seenGoalIds":["g1","g1"],"failure":null}),
        json!({"current":{"goal":valid["current"]["goal"],"roundsStarted":5,"createdAt":1,"updatedAt":1},"seenGoalIds":["g1"],"failure":null}),
        json!({"current":{"goal":valid["current"]["goal"],"roundsStarted":0,"createdAt":2,"updatedAt":1},"seenGoalIds":["g1"],"failure":null}),
        json!({"current":null,"seenGoalIds":[],"failure":null,"extra":true}),
        json!({"seenGoalIds":[],"failure":null}),
        json!({"current":null,"failure":null}),
        json!({"current":null,"seenGoalIds":[]}),
        json!({"current":null,"seenGoalIds":[""],"failure":null}),
    ] {
        assert!((definition.state_schema)(Some(&invalid)).is_err());
    }
    for (field, invalid) in [
        ("id", json!("")),
        ("revision", json!(0)),
        ("objective", json!("")),
        ("maxGoalRounds", json!(0)),
    ] {
        let mut invalid_state = valid.clone();
        invalid_state["current"]["goal"][field] = invalid;
        assert!((definition.state_schema)(Some(&invalid_state)).is_err());
    }
}

#[test]
fn init_returns_empty_state() {
    let definition = goal_projection_definition();
    let header = hoh_session::SessionHeader::new("projection-init".into(), 1);
    let initial = (definition.init)(&header, hoh_session::SessionLogOffset::default()).unwrap();
    assert_eq!(initial.as_ref(), &GoalProjectionState::default());
}

#[test]
fn unrelated_events_reuse_the_same_arc() {
    let current = state();
    let next = apply_goal_projection(&current, &event("turn/start", 0, json!({"turn":1})));
    assert!(Arc::ptr_eq(&current, &next));
}

#[test]
fn inbox_splices_do_not_revive_a_cleared_projection() {
    let current = Arc::new(GoalProjectionState {
        current: None,
        seen_goal_ids: vec![GoalId::from("cleared")],
        failure: None,
    });
    let next = apply_goal_projection(
        &current,
        &event(
            "agent/inbox/spliced",
            2,
            json!({"target":"next-step","start":0,"inserted":[{"source":{"kind":"goal"}}]}),
        ),
    );
    assert!(Arc::ptr_eq(&current, &next));
    assert!(next.current.is_none());
}

#[test]
fn admitted_goal_round_advances_projection() {
    let current = state();
    let next = apply_goal_projection(
        &current,
        &event(
            "user/message",
            1,
            json!({
                "source":{"kind":"goal","goalId":"g1","revision":1,"round":1}
            }),
        ),
    );
    assert_eq!(next.current.as_ref().unwrap().rounds_started, 1);
    assert!(next.failure.is_none());
}

#[test]
fn malformed_owned_event_retains_a_sticky_failure() {
    let current = state();
    let malformed = event(
        "goal/change",
        1,
        json!({
            "kind":"goal/change","version":1,"operation":"create"
        }),
    );
    let failed = apply_goal_projection(&current, &malformed);
    assert!(
        failed
            .failure
            .as_ref()
            .unwrap()
            .contains("goal replay failed")
    );
    let again = apply_goal_projection(&failed, &event("turn/start", 2, json!({"turn":1})));
    assert!(Arc::ptr_eq(&failed, &again));
}

#[test]
fn declared_goal_change_with_a_foreign_kind_retains_the_exact_failure() {
    let failed = apply_goal_projection(
        &state(),
        &event("goal/change", 4, json!({"kind":"not-a-goal-change"})),
    );
    assert_eq!(
        failed.failure.as_deref(),
        Some(
            "goal replay failed at session event 4: goal change at session event 4 has an invalid kind"
        )
    );
}

#[test]
fn wire_serves_null_then_the_whole_current_projection() {
    let definition = goal_projection_definition();
    let wire = definition.wire.unwrap();
    assert_eq!(
        (wire.view)(&Arc::new(GoalProjectionState::default()))
            .unwrap()
            .unwrap()
            .as_ref(),
        &json!(null)
    );
    let value = (wire.view)(&state()).unwrap().unwrap();
    assert_eq!(value["goal"]["id"], "g1");
    assert_eq!(value["roundsStarted"], 0);
}
