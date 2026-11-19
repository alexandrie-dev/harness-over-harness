use hoh_agent::fold_consumed_work;
use hoh_llm::{ContentBlock, MessageSource, NewUserMessage, create_user_message};
use hoh_session::{Session, SessionId};
use serde_json::{Value, json};

fn session(id: &str) -> Session {
    Session::create(SessionId::from(id), None, None, None, None).unwrap()
}

fn message(text: &str) -> Value {
    serde_json::to_value(create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(text)],
        source: MessageSource::new("user"),
    }))
    .unwrap()
}

/// Log an accepted message the way `Inbox.append()` does.
fn accept(session: &Session, text: &str) {
    session
        .append(
            "agent/inbox/spliced",
            json!({ "target": "next-turn", "start": 0, "inserted": [message(text)] }),
            None,
        )
        .unwrap();
}

/// Log the step-boundary read of one pending message, as `Inbox.claim()` does.
fn claim(session: &Session) {
    session
        .append(
            "agent/inbox/spliced",
            json!({ "target": "next-turn", "start": 0, "removedCount": 1, "inserted": [] }),
            None,
        )
        .unwrap();
}

/// Log a cancellation of one pending message, as `Inbox.clear()` does.
fn cancel_pending(session: &Session) {
    session
        .append(
            "agent/inbox/spliced",
            json!({
                "target": "next-turn", "start": 0, "removedCount": 1, "inserted": [],
                "outcome": "canceled"
            }),
            None,
        )
        .unwrap();
}

/// Run one whole turn that reached a model step.
fn stepped_turn(session: &Session, turn: u64, reason: Value) {
    session
        .append("turn/start", json!({ "turn": turn }), None)
        .unwrap();
    claim(session);
    session
        .append("step/start", json!({ "turn": turn, "step": 1 }), None)
        .unwrap();
    session
        .append("step/end", json!({ "turn": turn, "step": 1 }), None)
        .unwrap();
    session
        .append("turn/end", json!({ "turn": turn, "reason": reason }), None)
        .unwrap();
}

fn end_turn(work: &hoh_agent::ConsumedWork) -> Option<u64> {
    work.end
        .as_ref()
        .and_then(|e| e.data().get("turn"))
        .and_then(Value::as_u64)
}

#[test]
fn reports_nothing_for_a_log_that_consumed_no_work() {
    let s = session("empty");
    accept(&s, "queued");
    let work = fold_consumed_work(&s.snapshot_events(None, None));
    assert!(work.end.is_none());
    assert!(!work.dropped_unrun);
}

#[test]
fn reports_the_latest_turn_that_entered_a_model_step() {
    let s = session("stepped");
    stepped_turn(&s, 1, json!({ "kind": "completed" }));
    stepped_turn(&s, 2, json!({ "kind": "max-tokens" }));
    let work = fold_consumed_work(&s.snapshot_events(None, None));
    assert_eq!(
        work.end.unwrap().data(),
        &json!({ "turn": 2, "reason": { "kind": "max-tokens" } })
    );
}

#[test]
fn reports_a_turn_that_claimed_its_input_and_then_failed_before_any_step() {
    let s = session("failed-claim");
    stepped_turn(&s, 1, json!({ "kind": "completed" }));
    s.append("turn/start", json!({ "turn": 2 }), None).unwrap();
    claim(&s);
    s.append(
        "turn/end",
        json!({ "turn": 2, "reason": { "kind": "error", "error": { "message": "ENOSPC", "code": "UNKNOWN" } } }),
        None,
    )
    .unwrap();
    assert_eq!(
        end_turn(&fold_consumed_work(&s.snapshot_events(None, None))),
        Some(2)
    );
}

#[test]
fn reports_a_turn_that_claimed_its_input_and_was_then_stopped_before_any_step() {
    let s = session("stopped-claim");
    stepped_turn(&s, 1, json!({ "kind": "completed" }));
    s.append("turn/start", json!({ "turn": 2 }), None).unwrap();
    claim(&s);
    s.append(
        "turn/end",
        json!({ "turn": 2, "reason": { "kind": "aborted", "reason": { "kind": "user" } } }),
        None,
    )
    .unwrap();
    assert_eq!(
        end_turn(&fold_consumed_work(&s.snapshot_events(None, None))),
        Some(2)
    );
}

#[test]
fn ignores_a_turn_stopped_failed_or_rejected_without_taking_any_input() {
    let s = session("no-claim");
    stepped_turn(&s, 1, json!({ "kind": "completed" }));
    s.append("turn/start", json!({ "turn": 2 }), None).unwrap();
    s.append(
        "turn/end",
        json!({ "turn": 2, "reason": { "kind": "aborted", "reason": { "kind": "parent" } } }),
        None,
    )
    .unwrap();
    s.append("turn/start", json!({ "turn": 3 }), None).unwrap();
    s.append(
        "turn/end",
        json!({ "turn": 3, "reason": { "kind": "error", "error": { "message": "x", "code": "UNKNOWN" } } }),
        None,
    )
    .unwrap();
    s.append("turn/start", json!({ "turn": 4 }), None).unwrap();
    s.append(
        "turn/end",
        json!({ "turn": 4, "reason": { "kind": "blocked" } }),
        None,
    )
    .unwrap();
    assert_eq!(
        end_turn(&fold_consumed_work(&s.snapshot_events(None, None))),
        Some(1)
    );
}

#[test]
fn reports_a_turn_whose_claimed_input_a_pre_step_rejection_discarded() {
    let s = session("rejected-claim");
    stepped_turn(&s, 1, json!({ "kind": "completed" }));
    s.append("turn/start", json!({ "turn": 2 }), None).unwrap();
    claim(&s);
    s.append(
        "turn/end",
        json!({ "turn": 2, "reason": { "kind": "blocked" } }),
        None,
    )
    .unwrap();
    assert_eq!(
        end_turn(&fold_consumed_work(&s.snapshot_events(None, None))),
        Some(2)
    );
}

#[test]
fn ignores_a_claim_its_own_turn_emptied() {
    let s = session("emptied-claim");
    stepped_turn(&s, 1, json!({ "kind": "completed" }));
    s.append("turn/start", json!({ "turn": 2 }), None).unwrap();
    claim(&s);
    s.append(
        "turn/end",
        json!({ "turn": 2, "reason": { "kind": "completed" } }),
        None,
    )
    .unwrap();
    assert_eq!(
        end_turn(&fold_consumed_work(&s.snapshot_events(None, None))),
        Some(1)
    );
}

#[test]
fn credits_a_claim_with_no_open_turn_to_no_turn_at_all() {
    let s = session("mid-turn-suffix");
    stepped_turn(&s, 1, json!({ "kind": "completed" }));
    // An owned suffix can begin inside a turn whose start it does not contain.
    claim(&s);
    s.append(
        "turn/end",
        json!({ "turn": 2, "reason": { "kind": "aborted", "reason": { "kind": "user" } } }),
        None,
    )
    .unwrap();
    assert_eq!(
        end_turn(&fold_consumed_work(&s.snapshot_events(None, None))),
        Some(1)
    );
}

#[test]
fn reports_work_cancelled_out_of_the_inbox_after_the_last_accounting_turn() {
    let s = session("dropped");
    stepped_turn(&s, 1, json!({ "kind": "completed" }));
    accept(&s, "never runs");
    cancel_pending(&s);
    let events = s.snapshot_events(None, None);
    let work = fold_consumed_work(&events);
    let expected = events
        .iter()
        .find(|e| e.event_type() == "turn/end")
        .unwrap();
    assert!(std::sync::Arc::ptr_eq(work.end.as_ref().unwrap(), expected));
    assert!(work.dropped_unrun);
}

#[test]
fn keeps_a_replacement_pending_rather_than_counting_it_as_dropped() {
    let s = session("replaced");
    stepped_turn(&s, 1, json!({ "kind": "completed" }));
    s.append(
        "agent/inbox/spliced",
        json!({
            "target": "next-turn", "start": 0, "removedCount": 1,
            "inserted": [message("rewritten")], "outcome": "canceled"
        }),
        None,
    )
    .unwrap();
    assert!(!fold_consumed_work(&s.snapshot_events(None, None)).dropped_unrun);
}

#[test]
fn lets_a_later_accounting_turn_absorb_an_earlier_drop() {
    let s = session("absorbed");
    stepped_turn(&s, 1, json!({ "kind": "completed" }));
    cancel_pending(&s);
    stepped_turn(&s, 2, json!({ "kind": "completed" }));
    let events = s.snapshot_events(None, None);
    let work = fold_consumed_work(&events);
    let expected = events
        .iter()
        .rev()
        .find(|e| e.event_type() == "turn/end")
        .unwrap();
    assert!(std::sync::Arc::ptr_eq(work.end.as_ref().unwrap(), expected));
    assert!(!work.dropped_unrun);
}
