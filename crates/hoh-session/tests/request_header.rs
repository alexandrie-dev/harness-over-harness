mod common;
use common::*;
use hoh_session::*;
use serde_json::json;
use std::sync::Arc;
#[test]
fn normalizes_empty_optional_fields_to_absence_and_preserves_populated_fields() {
    assert_eq!(
        canonical_header(&json!({"config":{},"adapterDefaults":{},"tools":[]})),
        json!({"config":{}})
    );
    let h = json!({"config":{"maxTokens":256000},"adapterDefaults":{"maxTokens":true},"tools":[tool("a")]});
    assert_eq!(canonical_header(&h), h);
}
#[test]
fn compares_every_canonical_field_and_preserves_tool_order() {
    let a = json!({"config":{"provider":"mock","model":"m"},"tools":[tool("a")]});
    assert!(header_equals(&a, &a));
    for (key, value) in [
        ("provider", json!("other")),
        ("model", json!("other")),
        ("reasoningEffort", json!("high")),
        ("temperature", json!(0.5)),
        ("maxTokens", json!(256000)),
        ("stop", json!(["STOP"])),
    ] {
        let mut b = a.clone();
        b["config"][key] = value;
        assert!(!header_equals(&a, &b));
    }
    for key in ["reasoningEffort", "maxTokens"] {
        let mut b = a.clone();
        b["adapterDefaults"] = json!({key:true});
        assert!(!header_equals(&a, &b));
    }
    let mut b = a.clone();
    b["tools"][0]["description"] = json!("changed");
    assert!(!header_equals(&a, &b));
    assert!(!header_equals(
        &json!({"config":{},"tools":[tool("a"),tool("b")]}),
        &json!({"config":{},"tools":[tool("b"),tool("a")]})
    ));
}
#[test]
fn treats_absent_and_empty_tool_arrays_as_equivalent_canonical_absence() {
    assert!(header_equals(
        &json!({"config":{}}),
        &json!({"config":{},"tools":[]})
    ));
}
#[test]
fn returns_the_supplied_baseline_when_no_snapshot_follows() {
    let h = Arc::new(json!({"config":{}}));
    assert!(fold_request_header(&[], None).is_none());
    assert!(Arc::ptr_eq(
        &fold_request_header(
            &[event("turn/start", 0, json!({"turn":1}))],
            Some(h.clone())
        )
        .unwrap(),
        &h
    ));
}
#[test]
fn takes_the_latest_full_snapshot_and_skips_unrelated_events() {
    let s = session();
    request(&s, vec![tool("first")], "initial");
    user(&s, "hi");
    request(&s, vec![], "change");
    assert_eq!(
        *fold_request_header(&s.snapshot_events(None, None), None).unwrap(),
        json!({"config":{"provider":"mock","model":"mock"}})
    );
}
#[test]
fn reads_undefined_before_any_record_exists() {
    assert!(session().request_context().is_none());
}
#[test]
fn folds_a_seeded_log_on_first_read_taking_the_last_record() {
    let events = vec![
        event(
            "request/context",
            0,
            json!({"provider":"p","model":"first","contextWindow":128000}),
        ),
        event(
            "request/context",
            1,
            json!({"provider":"p","model":"later","contextWindow":256000}),
        ),
    ];
    let s = Session::create("test".into(), Some(&events), None, None, None).unwrap();
    assert_eq!(s.request_context().unwrap()["model"], "later");
}
#[test]
fn advances_incrementally_across_appends_and_skips_unrelated_events() {
    let s = session();
    s.append(
        "request/context",
        json!({"provider":"p","model":"m","contextWindow":128000}),
        None,
    )
    .unwrap();
    let first = s.request_context().unwrap();
    user(&s, "noise");
    assert!(Arc::ptr_eq(&first, &s.request_context().unwrap()));
    s.append(
        "request/context",
        json!({"provider":"p","model":"next"}),
        None,
    )
    .unwrap();
    assert!(s.request_context().unwrap().get("contextWindow").is_none());
    assert_eq!(first["contextWindow"], 128000);
}
#[test]
fn folds_a_batch_appended_between_two_reads() {
    let s = session();
    s.append("request/context", json!({"contextWindow":1}), None)
        .unwrap();
    s.request_context();
    s.append_batch(&[
        SessionEventInput::new("request/context", json!({"contextWindow":2})),
        SessionEventInput::new("turn/start", json!({"turn":1})),
        SessionEventInput::new("request/context", json!({"contextWindow":3})),
    ])
    .unwrap();
    assert_eq!(s.request_context().unwrap()["contextWindow"], 3);
}
#[test]
fn exposes_a_frozen_record_so_a_reader_cannot_desync_later_comparisons() {
    let s = session();
    s.append("request/context", json!({"contextWindow":128000}), None)
        .unwrap();
    let h = s.request_context().unwrap();
    let mut copy = (*h).clone();
    copy["contextWindow"] = json!(1);
    assert_eq!(s.request_context().unwrap()["contextWindow"], 128000);
}
