mod common;
use common::*;
use hoh_session::*;
use serde_json::json;
#[test]
fn admits_non_negative_safe_integers_into_distinct_sequence_roles() {
    assert_eq!(u64::from(seq(3)), 3);
    assert_eq!(u64::from(offset(4)), 4);
    assert_ne!(
        std::any::TypeId::of::<SessionSeq>(),
        std::any::TypeId::of::<SessionLogOffset>()
    );
}
#[test]
fn rejects_invalid_session_sequence_positions() {
    for v in [-1.0, -0.0, 0.5, 9_007_199_254_740_992.0, f64::NAN] {
        assert!(SessionSeq::from_number(v).is_err());
        assert!(SessionLogOffset::from_number(v).is_err());
    }
}
#[test]
fn keeps_event_identities_separate_from_log_offsets() {
    let s = session();
    let e = s.append("turn/start", json!({"turn":1}), None).unwrap();
    assert_eq!(e.seq(), seq(0));
    assert_eq!(s.first_live_seq(), offset(0));
    assert_eq!(s.seq(), offset(1));
}
#[test]
fn rejects_a_negative_zero_seq_at_the_restored_event_boundary() {
    let e = event("turn/start", 0, json!({"turn":1}));
    let e = patch(&e, "seq", json!(-0.0));
    error(
        Session::from_restore(
            "test".into(),
            &[e],
            header("test"),
            offset(0),
            SessionSeedEventState::Detached,
            None,
        ),
        "invalid event envelope",
    );
}
#[test]
fn keeps_fork_lineage_outside_the_logical_header_integer_fields() {
    let p = session();
    p.append("turn/start", json!({"turn":1}), None).unwrap();
    p.append(
        "turn/end",
        json!({"turn":1,"reason":{"kind":"completed"}}),
        None,
    )
    .unwrap();
    let mut h = header("child");
    h["isSeeded"] = json!(true);
    let child = Session::create(
        "child".into(),
        Some(&p.snapshot_events(None, None)),
        Some(h),
        Some(p.seq()),
        None,
    )
    .unwrap();
    assert_eq!(child.inherited_event_count(), offset(2));
    assert!(child.header().get("seedLength").is_none());
    assert!(!child.is_own_seq(seq(1)));
    assert!(child.is_own_seq(seq(2)));
    assert!(!child.is_own_seq(seq(3)));
    assert_eq!(child.own_events()[0].event_type(), "session/end-seed");
}
#[test]
fn appends_child_owned_setup_after_the_constructor_owned_inherited_marker() {
    let mut h = header("child");
    h["isSeeded"] = json!(true);
    let s = Session::create("child".into(), Some(&[]), Some(h), Some(offset(0)), None).unwrap();
    s.append("request/context", json!({"provider":"p","model":"m"}), None)
        .unwrap();
    assert_eq!(s.own_events().len(), 2);
    assert!(s.is_own_seq(seq(1)));
}
#[test]
fn accepts_child_closers_after_the_inherited_marker_and_rejects_a_later_inherited_marker() {
    let mut h = header("test");
    h["isSeeded"] = json!(true);
    let seed = vec![
        event("turn/start", 0, json!({"turn":1})),
        event("session/end-seed", 1, json!({"inherited":true})),
        event("turn/end", 2, json!({"turn":1,"reason":{"kind":"forked"}})),
    ];
    let s = Session::create(
        "test".into(),
        Some(&seed),
        Some(h.clone()),
        Some(offset(1)),
        None,
    )
    .unwrap();
    assert_eq!(s.snapshot_events(None, None).as_ref(), seed);
    assert_eq!(s.first_live_seq(), offset(3));
    let mut later = seed;
    later.push(event("session/end-seed", 3, json!({"inherited":true})));
    error(
        Session::create("test".into(), Some(&later), Some(h), Some(offset(1)), None),
        "final inherited marker",
    );
}
#[test]
fn requires_a_separately_supplied_inherited_cut_for_a_seeded_header() {
    let mut h = header("test");
    h["isSeeded"] = json!(true);
    error(
        Session::create("test".into(), Some(&[]), Some(h), None, None),
        "requires an inherited event count",
    );
}
#[test]
fn requires_an_explicit_constructor_seed_for_seeded_lineage() {
    let mut h = header("test");
    h["isSeeded"] = json!(true);
    error(
        Session::create("test".into(), None, Some(h), Some(offset(0)), None),
        "requires an explicit constructor seed",
    );
}
#[test]
fn requires_the_exact_cut_to_agree_with_lineage_and_log_length() {
    error(
        Session::create(
            "test".into(),
            Some(&[]),
            Some(header("test")),
            Some(offset(1)),
            None,
        ),
        "unseeded session inherited event count must be 0",
    );
    let mut h = header("test");
    h["isSeeded"] = json!(true);
    error(
        Session::create(
            "test".into(),
            Some(&[]),
            Some(h.clone()),
            Some(offset(1)),
            None,
        ),
        "exceeds its event log",
    );
    let seed = vec![
        event("turn/start", 0, json!({"turn":1})),
        event(
            "turn/end",
            1,
            json!({"turn":1,"reason":{"kind":"completed"}}),
        ),
    ];
    error(
        Session::create("test".into(), Some(&seed), Some(h), Some(offset(1)), None),
        "constructor seed must equal its inherited prefix",
    );
}
#[test]
fn revalidates_a_supplied_inherited_event_count() {
    for value in [-1.0, 0.5, 9_007_199_254_740_992.0] {
        assert!(SessionLogOffset::from_number(value).is_err());
        assert!(serde_json::from_value::<SessionLogOffset>(json!(value)).is_err());
    }
}
