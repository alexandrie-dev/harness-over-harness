mod common;
use common::*;
use hoh_session::*;
use serde_json::json;
use std::sync::Arc;
fn scratch(s: &Session) -> Vec<Arc<serde_json::Value>> {
    Session::create(
        "scratch".into(),
        Some(&s.snapshot_events(None, None)),
        None,
        None,
        None,
    )
    .unwrap()
    .derive_messages()
    .unwrap()
}
#[test]
fn stays_deep_equal_to_a_from_scratch_replay_derivation_as_the_log_grows() {
    let s = session();
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    user(&s, "one");
    assert_eq!(s.derive_messages().unwrap(), scratch(&s));
    user(&s, "two");
    s.append(
        "assistant/message",
        data("assistant", "reply"),
        Some(SurfaceIntent::append()),
    )
    .unwrap();
    assert_eq!(s.derive_messages().unwrap(), scratch(&s));
    let mut d = data("assistant", "");
    d["usage"] = json!({"inputTokens":1,"outputTokens":0});
    s.append("assistant/message", d, Some(SurfaceIntent::append()))
        .unwrap();
    assert_eq!(s.derive_messages().unwrap(), scratch(&s));
}
#[test]
fn rebuilds_on_a_surface_replace_and_still_matches_scratch() {
    let s = session();
    user(&s, "one");
    user(&s, "two");
    let before = s.derive_messages().unwrap();
    replace(&s, 0, 1, &[0, 1], "summary").unwrap();
    assert_eq!(s.derive_messages().unwrap(), scratch(&s));
    assert_eq!(before.len(), 2);
    assert_eq!(s.derive_messages().unwrap().len(), 1);
}
#[test]
fn returns_a_fresh_array_per_call_later_appends_never_grow_a_held_snapshot() {
    let s = session();
    user(&s, "one");
    let first = s.derive_messages().unwrap();
    user(&s, "two");
    let mut second = s.derive_messages().unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 2);
    assert!(Arc::ptr_eq(&first[0], &second[0]));
    second.reverse();
    assert_eq!(texts(&s), vec!["one", "two"]);
}
#[test]
fn projects_one_appended_event_exactly_as_the_full_derivation_projects_its_node() {
    let s = session();
    let e = user(&s, "hi");
    assert_eq!(
        s.derive_event_message(&e).unwrap(),
        s.derive_messages().unwrap().last().cloned()
    );
}
#[test]
fn reuses_the_logged_events_already_frozen_content() {
    let s = session();
    let e = user(&s, "orig");
    let message = s.derive_event_message(&e).unwrap().unwrap();
    assert_eq!(*message, *e.data());
    let mut detached = (*message).clone();
    detached["content"][0]["text"] = json!("mutated");
    assert_eq!(texts(&s), vec!["orig"]);
}
#[test]
fn projects_null_for_events_that_produce_no_message() {
    let s = session();
    let e = s
        .append("step/start", json!({"turn":1,"step":1}), None)
        .unwrap();
    assert!(s.derive_event_message(&e).unwrap().is_none());
    let e = s
        .append(
            "assistant/message",
            data("assistant", ""),
            Some(SurfaceIntent::append()),
        )
        .unwrap();
    assert!(s.derive_event_message(&e).unwrap().is_none());
}
