mod common;
use common::*;
use hoh_session::*;
use serde_json::{Value, json};
use std::sync::Arc;
fn addition() -> Value {
    json!([{"type":"tool-addition","toolName":"search"}])
}
#[test]
fn resolves_a_tool_addition_against_an_earlier_header_in_the_same_batch() {
    let s = session();
    let mut d = data("developer", "");
    d["headerSeq"] = json!(0);
    d["message"]["content"] = addition();
    s.append_batch(&[SessionEventInput::new("request/header",json!({"header":{"config":{"provider":"mock","model":"mock"},"tools":[tool("search")]},"reason":"initial"})),SessionEventInput::new("developer/message",d.clone()).with_surface(SurfaceIntent::append())]).unwrap();
    assert_eq!(s.seq(), offset(2));
    assert_eq!(*s.derive_messages().unwrap()[0], d["message"]);
    assert_eq!(nodes(&s), vec![1]);
}
#[test]
fn rejects_a_non_earlier_batch_header_reference_without_committing_a_prefix() {
    for reference in [1, 2] {
        let s = session();
        let mut d = data("developer", "");
        d["headerSeq"] = json!(reference);
        d["message"]["content"] = addition();
        error(s.append_batch(&[SessionEventInput::new("request/header",json!({"header":{"config":{"provider":"mock","model":"mock"},"tools":[tool("search")]},"reason":"initial"})),SessionEventInput::new("developer/message",d).with_surface(SurfaceIntent::append())]),"earlier request/header");
        assert_eq!(s.seq(), offset(0));
    }
}
#[test]
fn keeps_an_older_header_binding_through_compound_changes_replacement_restore_and_fork() {
    let s = session();
    let first = tool("search");
    let h = request(&s, vec![first.clone(), tool("other")], "initial");
    let mut second = first.clone();
    second["description"] = json!("Second");
    request(&s, vec![second], "change");
    let a=developer(&s,json!([{"type":"tool-removal","toolName":"search"},{"type":"tool-addition","toolName":"search"},{"type":"tool-addition","toolName":"other"}]),Some(h.seq())).unwrap();
    let mut d = data("developer", "");
    d["headerSeq"] = json!(h.seq());
    d["message"]["content"] = addition();
    let replacement = s
        .append(
            "developer/message",
            d,
            Some(SurfaceIntent::replace(a.seq(), a.seq(), vec![a.seq()])),
        )
        .unwrap();
    let events = s.snapshot_events(None, None);
    assert_eq!(events[0].data()["header"]["tools"][0], first);
    assert_eq!(
        fold_surface(&events, &[]).unwrap().nodes,
        vec![replacement.seq()]
    );
    let restored = Session::from_restore(
        s.id().clone(),
        &events,
        (*s.header()).clone(),
        offset(0),
        SessionSeedEventState::SharedFrozen,
        None,
    )
    .unwrap();
    assert_eq!(
        restored.derive_messages().unwrap(),
        s.derive_messages().unwrap()
    );
    let seed = build_fork_seed(&events, replacement.seq()).unwrap();
    let mut child_header = header("child");
    child_header["isSeeded"] = json!(true);
    let child = Session::create(
        "child".into(),
        Some(&seed),
        Some(child_header),
        Some(offset(u64::from(replacement.seq()) + 1)),
        None,
    )
    .unwrap();
    assert_eq!(child.event_at(h.seq()), Some(h));
}
#[test]
fn refuses_a_non_header_or_non_earlier_reference_before_append() {
    let s = session();
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    s.append("step/start", json!({"turn":1,"step":1}), None)
        .unwrap();
    request(&s, vec![tool("search")], "initial");
    let before = s.snapshot_events(None, None);
    for reference in [0, 3, 100] {
        error(
            developer(&s, addition(), Some(seq(reference))),
            "earlier request/header",
        );
        assert!(Arc::ptr_eq(&before, &s.snapshot_events(None, None)));
    }
}
#[test]
fn refuses_missing_or_ambiguous_historical_tool_names() {
    for tools in [
        vec![tool("different")],
        vec![tool("search"), tool("search")],
    ] {
        let s = session();
        let h = request(&s, tools, "initial");
        error(developer(&s, addition(), Some(h.seq())), "exactly one tool");
    }
}
#[test]
fn refuses_an_addition_bound_to_a_tool_less_header() {
    let s = session();
    let h = request(&s, vec![], "initial");
    error(developer(&s, addition(), Some(h.seq())), "exactly one tool");
}
#[test]
fn requires_a_header_exactly_for_additions_and_rejects_inline_definitions() {
    let s = session();
    let h = request(&s, vec![tool("search")], "initial");
    error(developer(&s, addition(), None), "requires headerSeq");
    error(
        developer(
            &s,
            json!([{"type":"tool-removal","toolName":"search"}]),
            Some(h.seq()),
        ),
        "requires headerSeq",
    );
    error(
        developer(
            &s,
            json!([{"type":"tool-addition","toolName":"search","tool":tool("search")}]),
            Some(h.seq()),
        ),
        "omit inline",
    );
    error(
        developer(
            &s,
            json!([{"type":"tool-addition","toolName":""}]),
            Some(h.seq()),
        ),
        "nonempty toolName",
    );
}
#[test]
fn refuses_an_incomplete_referenced_definition_in_imported_history() {
    for t in [
        json!({"name":"search"}),
        json!({"name":"search","description":null,"parameters":{}}),
        json!({"name":"search","description":"","parameters":[]}),
        json!({"name":"search","description":"","parameters":{},"deferLoading":false}),
    ] {
        let s = session();
        let h = request(&s, vec![t], "initial");
        assert!(developer(&s, addition(), Some(h.seq())).is_err());
    }
}
#[test]
fn applies_the_same_binding_check_to_imported_events_and_full_log_surface_folds() {
    let s = session();
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    let mut d = data("developer", "");
    d["headerSeq"] = json!(0);
    d["message"]["content"] = addition();
    let e = patch(&surface_event("developer", 1, ""), "data", d);
    let mut events = s.snapshot_events(None, None).to_vec();
    events.push(e);
    error(fold_surface(&events, &[]), "earlier request/header");
    error(
        Session::create("test".into(), Some(&events), None, None, None),
        "earlier request/header",
    );
}
