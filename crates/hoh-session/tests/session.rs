mod common;
use common::*;
use hoh_session::*;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
#[test]
fn exposes_one_stable_readonly_surface_view() {
    let s = session();
    let view = s.surface().unwrap();
    assert_eq!(view, s.surface().unwrap());
    user(&s, "new");
    assert_eq!(view.nodes().unwrap(), vec![seq(0)]);
    replace(&s, 0, 0, &[0], "replacement").unwrap();
    assert_eq!(view.nodes().unwrap(), vec![seq(1)]);
    assert_eq!(view.replace_generation().unwrap(), 1);
}
#[test]
fn derives_message_history_from_the_event_log() {
    let s = session();
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    user(&s, "hello");
    s.append("assistant/attempt",json!({"turn":1,"step":1,"stream":[{"type":"text-chunks","time0":1,"index":0,"dt":[],"texts":["hi"]}]}),None).unwrap();
    let mut d = data("assistant", "let me check");
    d["message"]["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"tool-call","id":"call","name":"echo","arguments":"{}"}));
    s.append("assistant/message", d, Some(SurfaceIntent::append()))
        .unwrap();
    s.append(
        "tool/result",
        data("tool", "ok"),
        Some(SurfaceIntent::append()),
    )
    .unwrap();
    s.append(
        "turn/end",
        json!({"turn":1,"reason":{"kind":"completed"}}),
        None,
    )
    .unwrap();
    let m = s.derive_messages().unwrap();
    assert_eq!(
        m.iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["user", "assistant", "tool"]
    );
    assert_eq!(m[1]["content"].as_array().unwrap().len(), 2);
    assert_eq!(m[2]["content"][0]["text"], "ok");
}
#[test]
fn accepts_and_round_trips_a_max_tokens_turn_end_reason() {
    let s = session();
    let e = s
        .append(
            "turn/end",
            json!({"turn":1,"reason":{"kind":"max-tokens"}}),
            None,
        )
        .unwrap();
    assert_eq!(e.data()["reason"], json!({"kind":"max-tokens"}));
    assert_eq!(e.as_value().clone()["data"]["reason"], e.data()["reason"]);
}
#[test]
fn round_trips_an_aborted_turn_with_its_cancellation_cause() {
    let s = session();
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    s.append(
        "turn/end",
        json!({"turn":1,"reason":{"kind":"aborted","reason":{"kind":"user"}}}),
        None,
    )
    .unwrap();
    let r = Session::create(
        "replay".into(),
        Some(&s.snapshot_events(None, None)),
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        r.event_at(seq(1)).unwrap().data()["reason"],
        json!({"kind":"aborted","reason":{"kind":"user"}})
    );
}
#[test]
fn renders_injected_context_and_user_messages_as_plain_user_content() {
    let s = session();
    let mut d = data("user", "file changed: a.ts");
    d["source"] = json!({"kind":"watcher"});
    s.append("user/message", d, Some(SurfaceIntent::append()))
        .unwrap();
    user(&s, "focus on tests");
    assert_eq!(texts(&s), vec!["file changed: a.ts", "focus on tests"]);
}
#[test]
fn keeps_the_exact_identified_context_message_in_durable_history_and_projection() {
    let s = session();
    let mut d = data(
        "user",
        "<system-reminder>Additional instructions from: pkg/AGENTS.md</system-reminder>",
    );
    d["source"] = json!({"kind":"agent-instructions","form":"instructions","changes":[]});
    let e = s
        .append("user/message", d.clone(), Some(SurfaceIntent::append()))
        .unwrap();
    assert_eq!(e.data(), &d);
    assert_eq!(*s.derive_messages().unwrap()[0], d);
}
#[test]
fn replays_identically_from_a_seeded_event_log() {
    let s = session();
    user(&s, "q");
    s.append(
        "assistant/message",
        data("assistant", "a"),
        Some(SurfaceIntent::append()),
    )
    .unwrap();
    let r = Session::create(
        "replay".into(),
        Some(&s.snapshot_events(None, None)),
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(s.derive_messages().unwrap(), r.derive_messages().unwrap());
    assert_eq!(r.first_live_seq(), s.seq());
    assert_eq!(u64::from(r.seq()), u64::from(s.seq()) + 1);
}
#[test]
fn marks_an_explicitly_empty_seed_without_marking_a_fresh_session() {
    assert_eq!(session().seq(), offset(0));
    let resumed = Session::create("resumed".into(), Some(&[]), None, None, None).unwrap();
    assert_eq!(resumed.first_live_seq(), offset(0));
    assert_eq!(resumed.event_at(seq(0)).unwrap().data(), &json!({}));
    let reopened = Session::create(
        "reopened".into(),
        Some(&resumed.snapshot_events(None, None)),
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        resumed.snapshot_events(None, None),
        reopened.snapshot_events(None, None)
    );
}
#[test]
fn rejects_pre_provider_request_headers_and_assistant_messages_on_seed_load() {
    error(
        Session::create(
            "test".into(),
            Some(&[event(
                "request/header",
                0,
                json!({"header":{"config":{"model":"old-model"}},"reason":"initial"}),
            )]),
            None,
            None,
            None,
        ),
        "lacks provider/model",
    );
    let old = event(
        "assistant/message",
        0,
        json!({"turn":1,"step":1,"content":[{"type":"text","text":"old"}]}),
    );
    error(
        Session::create(
            "test".into(),
            Some(&[patch(&old, "surfaceOp", json!("append"))]),
            None,
            None,
            None,
        ),
        "lacks an identified message",
    );
    Session::create(
        "test".into(),
        Some(&[event("plugin/event", 0, Value::Null)]),
        None,
        None,
        None,
    )
    .unwrap();
}
#[test]
fn validates_assistant_settlement_fields_without_replaying_embedded_streams() {
    for (key, value) in [
        ("turn", json!("1")),
        ("turn", json!(-1)),
        ("turn", json!(-0.0)),
        ("turn", json!(1.5)),
        ("step", json!("1")),
        ("step", json!(-1)),
        ("step", json!(-0.0)),
        ("step", json!(1.5)),
        ("stream", Value::Null),
    ] {
        let mut d = json!({"turn":1,"step":1,"stream":[]});
        d[key] = value;
        error(
            Session::from_restore(
                "test".into(),
                &[event("assistant/attempt", 0, d)],
                header("test"),
                offset(0),
                SessionSeedEventState::Detached,
                None,
            ),
            "invalid settlement fields",
        );
    }
    let mut d = data("assistant", "different");
    d["stream"] = json!([{"type":"text-chunks","time0":1,"index":0,"dt":[],"texts":["streamed"]}]);
    let e = patch(&surface_event("assistant", 0, "different"), "data", d);
    let r = Session::from_restore(
        "test".into(),
        std::slice::from_ref(&e),
        header("test"),
        offset(0),
        SessionSeedEventState::Detached,
        None,
    )
    .unwrap();
    assert!(Arc::ptr_eq(&e, &r.event_at(seq(0)).unwrap()));
}
#[test]
fn rejects_historical_or_malformed_request_header_lifecycle_markers_on_seed_load() {
    for reason in [json!("fallback"), json!("unknown"), Value::Null] {
        error(
            Session::create(
                "test".into(),
                Some(&[event(
                    "request/header",
                    0,
                    json!({"header":{"config":{"provider":"mock","model":"model"}},"reason":reason}),
                )]),
                None,
                None,
                None,
            ),
            "invalid reason",
        );
    }
    for marker in [json!(false), json!(1), json!("true")] {
        error(
            Session::create(
                "test".into(),
                Some(&[event(
                    "request/header",
                    0,
                    json!({"header":{"config":{"provider":"mock","model":"model"}},"reason":"initial","startsSeries":marker}),
                )]),
                None,
                None,
                None,
            ),
            "invalid startsSeries marker",
        );
    }
}
#[test]
fn rejects_event_specific_malformed_message_shapes_on_seed_load() {
    for (role, key, value, expected) in [
        ("user", "id", json!(""), "identified message"),
        ("user", "role", json!("assistant"), "role \"user\""),
        ("user", "source", Value::Null, "invalid source"),
        ("user", "content", json!("not-an-array"), "invalid content"),
        (
            "assistant",
            "source",
            json!({"kind":"user"}),
            "model source",
        ),
        ("tool", "source", json!({"kind":"user"}), "tool source"),
        ("system", "role", json!("user"), "role \"system\""),
        (
            "system",
            "source",
            json!({"kind":"user"}),
            "system-prompt source",
        ),
        ("tool", "role", json!("user"), "role \"tool\""),
        (
            "tool",
            "toolCallId",
            json!("other"),
            "mismatched tool call ids",
        ),
    ] {
        let e = surface_event(role, 0, "content");
        let mut d = e.data().clone();
        if role == "user" {
            d[key] = value;
        } else {
            d["message"][key] = value;
        }
        error(
            Session::create(
                "test".into(),
                Some(&[patch(&e, "data", d)]),
                None,
                None,
                None,
            ),
            expected,
        );
    }
}
#[test]
fn snapshots_message_events_without_validating_plugin_owned_block_details() {
    let mut e = surface_event("user", 0, "extended").as_value().clone();
    e["data"]["content"] = json!([{"type":"plugin-block","value":1}]);
    e["data"]["source"] = json!({"kind":"plugin-source","value":1});
    let e = SessionEvent::from_value(e);
    assert_eq!(snapshot_session_event(&e).unwrap(), e);
}
#[test]
fn adopts_exclusively_owned_messages_in_place_and_keeps_snapshots_detached() {
    let e = (*surface_event("user", 0, "owned")).clone();
    let accepted = adopt_session_event(e.clone()).unwrap();
    assert_eq!(accepted, e);
    assert_eq!(snapshot_session_event(&accepted).unwrap(), e);
}
#[test]
fn adopts_developer_messages_in_place_and_freezes_their_nested_tool_changes() {
    let mut d = data("developer", "");
    d["headerSeq"] = json!(0);
    d["message"]["content"] = json!([{"type":"tool-addition","toolName":"search"}]);
    let e = patch(&surface_event("developer", 1, ""), "data", d);
    assert_eq!(adopt_session_event((*e).clone()).unwrap(), *e);
}
#[test]
fn adopts_a_system_message_by_freezing_its_message() {
    let e = surface_event("system", 0, "You are terse.");
    assert_eq!(adopt_session_event((*e).clone()).unwrap(), *e);
}
#[test]
fn validates_message_shape_before_adopting_ownership() {
    let e = surface_event("user", 0, "");
    let mut d = e.data().clone();
    d["role"] = json!("assistant");
    error(
        adopt_session_event((*patch(&e, "data", d)).clone()),
        "role \"user\"",
    );
}
#[test]
fn round_trips_a_non_empty_reasoning_effort_and_rejects_invalid_durable_values() {
    for (value, valid) in [
        (json!("adapter-owned"), true),
        (json!(""), false),
        (json!(1), false),
    ] {
        let e = event(
            "request/header",
            0,
            json!({"header":{"config":{"provider":"mock","model":"model","reasoningEffort":value}},"reason":"initial"}),
        );
        let r = Session::create(
            "test".into(),
            Some(std::slice::from_ref(&e)),
            None,
            None,
            None,
        );
        if valid {
            assert_eq!(r.unwrap().event_at(seq(0)), Some(e));
        } else {
            error(r, "invalid reasoningEffort");
        }
    }
}
#[test]
fn round_trips_adapter_default_markers_and_rejects_invalid_durable_values() {
    for (defaults, valid) in [
        (json!({"maxTokens":true}), true),
        (Value::Null, false),
        (json!([]), false),
        (json!({"unknown":true}), false),
        (json!({"maxTokens":false}), false),
        (json!({"reasoningEffort":true}), false),
    ] {
        let e = event(
            "request/header",
            0,
            json!({"header":{"config":{"provider":"mock","model":"model","maxTokens":256000},"adapterDefaults":defaults},"reason":"initial"}),
        );
        let r = Session::create(
            "test".into(),
            Some(std::slice::from_ref(&e)),
            None,
            None,
            None,
        );
        if valid {
            assert_eq!(r.unwrap().event_at(seq(0)), Some(e));
        } else {
            error(r, "invalid adapterDefaults");
        }
    }
}
#[test]
fn isolates_the_log_from_mutation_through_a_derived_message() {
    let s = session();
    user(&s, "original");
    s.append(
        "tool/result",
        data("tool", "tool out"),
        Some(SurfaceIntent::append()),
    )
    .unwrap();
    let before = s.snapshot_events(None, None);
    let mut messages = s.derive_messages().unwrap();
    let mut d = (*messages[0]).clone();
    d["content"][0]["text"] = json!("HACKED");
    messages.reverse();
    assert_eq!(s.snapshot_events(None, None), before);
    assert_eq!(texts(&s), vec!["original", "tool out"]);
}
#[test]
fn rejects_non_json_serializable_event_data_at_the_source() {
    let s = session();
    let mut d = data("user", "x");
    d["extra"] = json!(-0.0);
    error(
        s.append("user/message", d, Some(SurfaceIntent::append())),
        "non-JSON-serializable",
    );
    assert_eq!(s.seq(), offset(0));
}
#[test]
fn rejects_a_surface_eligible_append_with_no_surface_op_marker() {
    let s = session();
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    error(
        s.append("user/message", data("user", "hi"), None),
        "requires a surfaceOp marker",
    );
    assert_eq!(s.seq(), offset(1));
}
#[test]
fn accepts_dense_arrays_and_nested_plain_objects() {
    let s = session();
    let mut d = data("user", "x");
    d["extra"] = json!([1,2,[3,{"a":null,"b":true}]]);
    s.append("user/message", d.clone(), Some(SurfaceIntent::append()))
        .unwrap();
    assert_eq!(s.event_at(seq(0)).unwrap().data(), &d);
}
#[test]
fn validates_seed_events_rejects_a_non_json_serializable_seed() {
    let e = event("turn/start", 0, json!({"bad":-0.0}));
    error(
        Session::create("test".into(), Some(&[e]), None, None, None),
        "losslessly JSON-serializable",
    );
}
#[test]
fn validates_seed_events_rejects_a_non_contiguous_seq() {
    error(
        Session::create(
            "test".into(),
            Some(&[
                event("turn/start", 0, json!({"turn":1})),
                event(
                    "turn/end",
                    5,
                    json!({"turn":1,"reason":{"kind":"completed"}}),
                ),
            ]),
            None,
            None,
            None,
        ),
        "seed must be contiguous from 0",
    );
}
#[test]
fn validates_seed_events_rejects_a_surface_eligible_event_missing_its_marker() {
    error(
        Session::create(
            "test".into(),
            Some(&[event("user/message", 0, data("user", "hi"))]),
            None,
            None,
            None,
        ),
        "requires a surfaceOp marker",
    );
}
#[test]
fn accepts_a_well_formed_contiguous_serializable_seed() {
    let seed = vec![
        event("turn/start", 0, json!({"turn":1})),
        surface_event("user", 1, "hi"),
        event(
            "turn/end",
            2,
            json!({"turn":1,"reason":{"kind":"completed"}}),
        ),
    ];
    let s = Session::create("test".into(), Some(&seed), None, None, None).unwrap();
    assert_eq!(s.snapshot_events(None, Some(offset(3))).as_ref(), seed);
    assert_eq!(s.first_live_seq(), offset(3));
}
#[test]
fn rejects_non_json_surface_metadata_in_a_seed_event() {
    let e = patch(
        &surface_event("user", 0, "hello"),
        "surfaceOp",
        json!({"op":"replace","startSeq":-0.0,"endSeq":2}),
    );
    error(
        Session::create("test".into(), Some(&[e]), None, None, None),
        "losslessly JSON-serializable",
    );
}
#[test]
fn snapshots_the_seed_mutating_the_original_after_construction_does_not_affect_the_log() {
    let mut v = surface_event("user", 0, "original").as_value().clone();
    let seed = Arc::new(SessionEvent::from_value(v.clone()));
    let s = Session::create(
        "test".into(),
        Some(std::slice::from_ref(&seed)),
        None,
        None,
        None,
    )
    .unwrap();
    v["data"]["content"][0]["text"] = json!("HACKED");
    assert_eq!(texts(&s), vec!["original"]);
    assert!(!Arc::ptr_eq(&seed, &s.event_at(seq(0)).unwrap()));
}
#[test]
fn snapshots_append_data_mutating_the_passed_object_after_append_does_not_affect_the_log() {
    let s = session();
    let mut d = data("user", "original");
    let e = s
        .append("user/message", d.clone(), Some(SurfaceIntent::append()))
        .unwrap();
    d["content"][0]["text"] = json!("HACKED");
    assert_eq!(texts(&s), vec!["original"]);
    assert_eq!(e.data()["content"][0]["text"], "original");
}
#[test]
fn rejects_non_json_surface_metadata_before_appending_the_event() {
    let s = session();
    error(
        s.append_batch(&[
            SessionEventInput::new("user/message", data("user", "hello"))
                .with_options(json!({"surfaceOp":{"op":"replace","startSeq":-0.0,"endSeq":2}})),
        ]),
        "non-JSON-serializable surface metadata",
    );
    assert_eq!(s.seq(), offset(0));
}
#[test]
fn rejects_invalid_plain_surface_metadata_shapes_at_append() {
    for (opts, expected) in [
        (json!({"surfaceOp":"invalid"}), "invalid surfaceOp"),
        (
            json!({"surfaceOp":{"op":"replace","startSeq":-1,"endSeq":0}}),
            "invalid replace surfaceOp",
        ),
        (
            json!({"surfaceOp":"append","sourceEventSeqs":[0,-1]}),
            "safe integers",
        ),
    ] {
        let s = session();
        error(
            s.append_batch(&[
                SessionEventInput::new("user/message", data("user", "hello")).with_options(opts),
            ]),
            expected,
        );
        assert_eq!(s.seq(), offset(0));
    }
}
#[test]
fn rejects_surface_metadata_on_non_surface_append_and_seed_events() {
    let s = session();
    error(
        s.append_batch(&[SessionEventInput::new("turn/start", json!({"turn":1}))
            .with_surface(SurfaceIntent::append())]),
        "cannot carry surfaceOp",
    );
    error(
        Session::create(
            "test".into(),
            Some(&[patch(
                &event("turn/start", 0, json!({"turn":1})),
                "surfaceOp",
                json!("append"),
            )]),
            None,
            None,
            None,
        ),
        "not surface-eligible",
    );
}
#[test]
fn deep_freezes_seeded_and_appended_event_snapshots() {
    let seed = event("turn/start", 0, json!({"turn":1}));
    let s = Session::create("test".into(), Some(&[seed]), None, None, None).unwrap();
    let e = user(&s, "first");
    let mut v = e.as_value().clone();
    v["data"]["content"][0]["text"] = json!("mutated");
    assert_eq!(texts(&s), vec!["first"]);
    assert_eq!(s.event_at(seq(0)).unwrap().data()["turn"], 1);
}
#[test]
fn returns_cached_frozen_event_array_snapshots_that_do_not_grow_after_append() {
    let s = session();
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    let before = s.snapshot_events(None, None);
    assert!(Arc::ptr_eq(&before, &s.snapshot_events(None, None)));
    s.append(
        "turn/end",
        json!({"turn":1,"reason":{"kind":"completed"}}),
        None,
    )
    .unwrap();
    let after = s.snapshot_events(None, None);
    assert_eq!(before.len(), 1);
    assert_eq!(after.len(), 2);
    assert!(!Arc::ptr_eq(&before, &after));
    assert!(Arc::ptr_eq(&after, &s.snapshot_events(None, None)));
}
#[test]
fn reads_one_event_without_materializing_an_array_and_snapshots_half_open_ranges() {
    let s = session();
    let start = s.append("turn/start", json!({"turn":1}), None).unwrap();
    let end = s
        .append(
            "turn/end",
            json!({"turn":1,"reason":{"kind":"completed"}}),
            None,
        )
        .unwrap();
    assert!(Arc::ptr_eq(&s.event_at(seq(0)).unwrap(), &start));
    assert!(Arc::ptr_eq(&s.event_at(seq(1)).unwrap(), &end));
    assert!(s.event_at(seq(2)).is_none());
    let range = s.snapshot_events(Some(offset(1)), Some(offset(2)));
    assert_eq!(range.as_ref(), &[end]);
    s.append("turn/start", json!({"turn":2}), None).unwrap();
    assert_eq!(range.len(), 1);
}
#[test]
fn detaches_and_freezes_an_explicitly_supplied_session_header() {
    let mut h = header("test");
    h["cwd"] = json!("/accepted");
    h["parentSession"] = json!("parent");
    h["isSeeded"] = json!(true);
    let s = Session::create(
        "test".into(),
        Some(&[]),
        Some(h.clone()),
        Some(offset(0)),
        None,
    )
    .unwrap();
    h["cwd"] = json!("/caller-mutated");
    assert_eq!(s.header()["cwd"], "/accepted");
    assert_eq!(s.id().as_str(), "test");
}
#[test]
fn rejects_an_exotic_non_json_or_mismatched_supplied_header() {
    for h in [Value::Null, json!(1), json!([])] {
        error(
            Session::from_restore(
                "test".into(),
                &[],
                h,
                offset(0),
                SessionSeedEventState::Detached,
                None,
            ),
            "plain JSON record",
        );
    }
    let mut h = header("other");
    error(
        Session::create("test".into(), None, Some(h.clone()), None, None),
        "does not match session id",
    );
    h["extra"] = json!(-0.0);
    error(
        Session::create("test".into(), None, Some(h), None, None),
        "losslessly JSON-serializable",
    );
}
#[test]
fn rejects_invalid_scalar_fields_in_an_explicitly_supplied_header() {
    for (key, value, expected) in [
        ("version", json!(5), "header version"),
        ("createdAt", json!("123"), "createdAt must be"),
        ("cwd", json!(1), "cwd must be a string"),
        ("cwd", json!("relative"), "absolute path"),
        ("parentSession", json!(1), "parentSession must be a string"),
        ("isSeeded", json!("yes"), "isSeeded must be a boolean"),
        ("seedLength", json!(1), "invalid field \"seedLength\""),
    ] {
        let mut h = header("test");
        h[key] = value;
        error(
            Session::create("test".into(), None, Some(h), None, None),
            expected,
        );
    }
}
#[test]
fn retains_opaque_logical_header_metadata_except_the_physical_v0_seed_field() {
    let mut h = header("test");
    h["extension"] = json!({"value":"kept"});
    let s = Session::create("test".into(), None, Some(h), None, None).unwrap();
    assert_eq!(s.header()["extension"], json!({"value":"kept"}));
}
#[test]
fn rejects_seed_records_with_invalid_fixed_envelope_fields() {
    let base = event("turn/start", 0, json!({"turn":1}));
    for (key, value) in [
        ("extra", json!(true)),
        ("type", json!(1)),
        ("seq", json!("0")),
        ("seq", json!(0.5)),
        ("seq", json!(-1)),
        ("time", json!("1")),
        ("time", json!(0.5)),
        ("ignorable", json!(false)),
        ("ignorable", json!("yes")),
    ] {
        error(
            Session::create(
                "test".into(),
                Some(&[patch(&base, key, value)]),
                None,
                None,
                None,
            ),
            "invalid event envelope",
        );
    }
    let mut missing = base.as_value().clone();
    missing.as_object_mut().unwrap().shift_remove("data");
    error(
        Session::create(
            "test".into(),
            Some(&[Arc::new(SessionEvent::from_value(missing))]),
            None,
            None,
            None,
        ),
        "invalid event envelope",
    );
    let s = Session::create(
        "test".into(),
        Some(&[patch(&base, "ignorable", json!(true))]),
        None,
        None,
        None,
    )
    .unwrap();
    assert!(s.event_at(seq(0)).unwrap().ignorable());
}

#[tokio::test(start_paused = true)]
async fn creates_sessions_emits_session_created_and_session_event() {
    let (k, ctx, store) = mounted().await;
    let created = Arc::new(Mutex::new(vec![]));
    let out = created.clone();
    SessionStore::on_created(&ctx, move |s| {
        lock(&out).push(s.clone());
        Ok(())
    })
    .unwrap();
    let events = Arc::new(Mutex::new(vec![]));
    let out = events.clone();
    ctx.on_contained::<SessionEventEmitted>(move |e| {
        lock(&out).push((e.session.clone(), e.event.clone()));
        Ok(())
    })
    .unwrap();
    let s = store.create(&ctx, None, Default::default()).unwrap();
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    user(&s, "x");
    assert_eq!(*lock(&created), vec![s.clone()]);
    assert_eq!(lock(&events).len(), 2);
    assert_eq!(lock(&events)[1].0, s);
    assert_eq!(store.list(), vec![s.clone()]);
    assert_eq!(store.get(s.id()), Some(s));
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn rejects_duplicate_ids_and_supports_seeding() {
    let (k, ctx, store) = mounted().await;
    let a = store
        .create(&ctx, Some("fixed".into()), Default::default())
        .unwrap();
    error(
        store.create(&ctx, Some("fixed".into()), Default::default()),
        "already exists",
    );
    user(&a, "q");
    let b = store
        .create(
            &ctx,
            Some("fork".into()),
            CreateSessionOptions {
                seed: Some(a.snapshot_events(None, None).to_vec()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(a.derive_messages().unwrap(), b.derive_messages().unwrap());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn enter_rejects_a_stale_prepared_session_whose_id_is_already_live() {
    let (k, ctx, store) = mounted().await;
    let stale = store
        .prepare(Some("racy".into()), Default::default())
        .unwrap();
    let live = store
        .create(&ctx, Some("racy".into()), Default::default())
        .unwrap();
    error(store.enter(&ctx, stale), "already exists");
    assert_eq!(store.get(live.id()), Some(live));
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn prepare_enter_announce_register_a_session_and_emit_created() {
    let (k, ctx, store) = mounted().await;
    let n = Arc::new(Mutex::new(0));
    let out = n.clone();
    SessionStore::on_created(&ctx, move |_| {
        *lock(&out) += 1;
        Ok(())
    })
    .unwrap();
    let s = store
        .prepare(Some("lifecycle".into()), Default::default())
        .unwrap();
    assert!(store.get(s.id()).is_none());
    let d = store.enter(&ctx, s.clone()).unwrap();
    assert_eq!(*lock(&n), 0);
    store.announce(&s).unwrap();
    assert_eq!(*lock(&n), 1);
    d.detach();
    d.detach();
    assert!(store.get(s.id()).is_none());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn prevents_simultaneous_attachment_of_one_session_object_to_two_stores() {
    let (k, ctx, first) = mounted().await;
    let second = SessionStore::new();
    let s = session();
    let a = first.enter(&ctx, s.clone()).unwrap();
    error(second.enter(&ctx, s.clone()), "already attached to a store");
    a.detach();
    let b = second.enter(&ctx, s.clone()).unwrap();
    assert_eq!(second.get(s.id()), Some(s));
    b.detach();
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn rejects_direct_and_reentrant_repeat_announcements_to_preserve_one_lifecycle_pair() {
    let (k, ctx, store) = mounted().await;
    let st = store.clone();
    SessionStore::on_created(&ctx, move |s| {
        error(st.announce(s), "already announced");
        Ok(())
    })
    .unwrap();
    let s = store.prepare(None, Default::default()).unwrap();
    let d = store.enter(&ctx, s.clone()).unwrap();
    store.announce(&s).unwrap();
    error(store.announce(&s), "already announced");
    d.detach();
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn defers_a_reentrant_detach_until_the_creation_dispatch_unwinds() {
    let (k, ctx, store) = mounted().await;
    let order = Arc::new(Mutex::new(vec![]));
    let s = store.prepare(None, Default::default()).unwrap();
    let detach = store.enter(&ctx, s.clone()).unwrap();
    let st = store.clone();
    let out = order.clone();
    SessionStore::on_created(&ctx, move |s| {
        lock(&out).push("created:first");
        detach.detach();
        assert_eq!(st.get(s.id()), Some(s.clone()));
        Ok(())
    })
    .unwrap();
    let out = order.clone();
    SessionStore::on_created(&ctx, move |_| {
        lock(&out).push("created:second");
        Ok(())
    })
    .unwrap();
    let out = order.clone();
    ctx.on_contained::<SessionDisposed>(move |_| {
        lock(&out).push("disposed");
        Ok(())
    })
    .unwrap();
    store.announce(&s).unwrap();
    assert_eq!(
        *lock(&order),
        vec!["created:first", "created:second", "disposed"]
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn rolls_back_create_when_its_owner_unloads_from_session_created() {
    let (k, ctx, store) = mounted().await;
    let owner = ctx.child("owner").unwrap();
    let fiber = owner.fiber();
    let disposable = fiber.clone();
    SessionStore::on_created(&ctx, move |_| {
        drop(disposable.dispose());
        Ok(())
    })
    .unwrap();
    let s = store.create(&owner, None, Default::default()).unwrap();
    fiber.dispose().await;
    assert!(store.get(s.id()).is_none());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn synthesizes_a_minimal_current_version_header_for_a_bare_created_session() {
    let (k, ctx, store) = mounted().await;
    let s = store
        .create(&ctx, Some("plain".into()), Default::default())
        .unwrap();
    assert_eq!(s.header()["version"], 4);
    assert_eq!(s.header()["id"], "plain");
    assert_eq!(s.header()["isSeeded"], false);
    assert!(s.header()["createdAt"].as_i64().is_some());
    assert!(s.header().get("cwd").is_none());
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn attaches_cwd_and_parent_session_from_meta_to_the_header() {
    let (k, ctx, store) = mounted().await;
    let s = store
        .create(
            &ctx,
            Some("child".into()),
            CreateSessionOptions {
                meta: Some(json!({"cwd":"/work/project","parentSession":"parent"})),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(s.header()["cwd"], "/work/project");
    assert_eq!(s.header()["parentSession"], "parent");
    assert_eq!(s.header()["isSeeded"], false);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn attaches_subagent_origin_and_delegation_depth_from_meta_to_the_header() {
    let (k, ctx, store) = mounted().await;
    let s = store
        .create(
            &ctx,
            None,
            CreateSessionOptions {
                meta: Some(
                    json!({"parentSession":"parent","origin":"subagent","delegationDepth":2}),
                ),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(s.header()["origin"], "subagent");
    assert_eq!(s.header()["delegationDepth"], 2);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn rejects_non_json_and_invalid_scalar_session_metadata() {
    let (k, _ctx, store) = mounted().await;
    for meta in [
        json!({"parentSession":-0.0}),
        json!({"cwd":1}),
        json!({"parentSession":1}),
        json!({"createdAt":"123"}),
        json!({"createdAt":1.5}),
        json!({"createdAt":-1}),
        json!({"createdAt":9_007_199_254_740_992_u64}),
        json!({"isSeeded":"yes"}),
        json!({"origin":"fork"}),
        json!({"delegationDepth":"1"}),
        json!({"delegationDepth":0.5}),
        json!({"delegationDepth":-1}),
        json!({"agentPreset":1}),
    ] {
        assert!(
            store
                .prepare(
                    None,
                    PrepareSessionOptions::Create(CreateSessionOptions {
                        meta: Some(meta),
                        ..Default::default()
                    })
                )
                .is_err()
        );
    }
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn rejects_a_non_absolute_meta_cwd() {
    let (k, ctx, store) = mounted().await;
    error(
        store.create(
            &ctx,
            Some("rel".into()),
            CreateSessionOptions {
                meta: Some(json!({"cwd":"relative/path"})),
                ..Default::default()
            },
        ),
        "cwd must be an absolute path",
    );
    assert!(store.get(&"rel".into()).is_none());
    k.shutdown().await;
}
#[test]
fn a_bare_session_constructed_without_the_store_still_exposes_a_current_version_header() {
    assert_eq!(session().header()["version"], 4);
    assert_eq!(session().header()["isSeeded"], false);
}
#[tokio::test(start_paused = true)]
async fn detaches_sessions_when_the_creating_fiber_is_disposed() {
    let (k, ctx, store) = mounted().await;
    let owner = ctx.child("owner").unwrap();
    let s = store.create(&owner, None, Default::default()).unwrap();
    owner.fiber().dispose().await;
    assert!(store.get(s.id()).is_none());
    error(
        s.append(
            "user/message",
            data("user", "late"),
            Some(SurfaceIntent::append()),
        ),
        "outside a SessionStore",
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn pairs_a_partial_created_announcement_with_disposal_during_rollback() {
    let (k, ctx, store) = mounted().await;
    let once = Arc::new(AtomicBool::new(false));
    let flag = once.clone();
    SessionStore::on_created(&ctx, move |_| {
        if !flag.swap(true, Ordering::SeqCst) {
            Err("boom created listener".into())
        } else {
            Ok(())
        }
    })
    .unwrap();
    let disposed = Arc::new(Mutex::new(vec![]));
    let out = disposed.clone();
    ctx.on_contained::<SessionDisposed>(move |e| {
        lock(&out).push(e.session.id().clone());
        Ok(())
    })
    .unwrap();
    error(
        store.create(&ctx, Some("fixed".into()), Default::default()),
        "boom created listener",
    );
    assert!(store.get(&"fixed".into()).is_none());
    assert_eq!(*lock(&disposed), vec![SessionId::from("fixed")]);
    let s = store
        .create(&ctx, Some("fixed".into()), Default::default())
        .unwrap();
    user(&s, "hi");
    assert_eq!(texts(&s), vec!["hi"]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn contains_session_event_observer_failures_after_the_append_commit_point() {
    let (k, ctx, store) = mounted().await;
    let s = store
        .create(&ctx, Some("contained-event".into()), Default::default())
        .unwrap();
    ctx.on_contained::<SessionEventEmitted>(|e| {
        assert_eq!(e.session.event_at(e.event.seq()), Some(e.event.clone()));
        Err("sync event observer".into())
    })
    .unwrap();
    ctx.on_contained_async::<SessionEventEmitted, _>(|_| async {
        Err("async event observer".into())
    })
    .unwrap();
    let calls = Arc::new(Mutex::new(0));
    let out = calls.clone();
    ctx.on_contained::<SessionEventEmitted>(move |_| {
        *lock(&out) += 1;
        Ok(())
    })
    .unwrap();
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    assert_eq!(*lock(&calls), 1);
    let warnings = ctx
        .logger()
        .buffer()
        .into_iter()
        .filter(|r| r.message.starts_with("session \"contained-event\":"))
        .map(|r| r.message)
        .collect::<Vec<_>>();
    assert_eq!(
        warnings,
        vec![
            "session \"contained-event\": session/event listener threw: Error: sync event observer",
            "session \"contained-event\": session/event listener rejected: Error: async event observer"
        ]
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn runs_internal_dispatch_validation_on_one_frozen_candidate_before_commit_and_resets_after_a_veto()
 {
    let (k, ctx, store) = mounted().await;
    let s = store.create(&ctx, None, Default::default()).unwrap();
    let reject = Arc::new(AtomicBool::new(true));
    let flag = reject.clone();
    let validations = Arc::new(Mutex::new(vec![]));
    let out = validations.clone();
    ctx.on_dispatch(move |_, name, e| {
        if name == "session/event" {
            let e = e.downcast_ref::<SessionEventEmitted>().unwrap();
            lock(&out).push((e.event.seq(), e.session.seq()));
            if flag.swap(false, Ordering::SeqCst) {
                e.validation.reject("reject first candidate".into());
            }
        }
    })
    .unwrap();
    error(
        s.append("turn/start", json!({"turn":1}), None),
        "reject first candidate",
    );
    assert_eq!(s.seq(), offset(0));
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    assert_eq!(
        *lock(&validations),
        vec![(seq(0), offset(0)), (seq(0), offset(0))]
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn does_not_publish_a_surface_transition_rejected_by_internal_dispatch() {
    let (k, ctx, store) = mounted().await;
    let s = store.create(&ctx, None, Default::default()).unwrap();
    user(&s, "source");
    let stop = ctx
        .on_dispatch(|_, name, e| {
            if name == "session/event" {
                e.downcast_ref::<SessionEventEmitted>()
                    .unwrap()
                    .validation
                    .reject("reject surface candidate".into());
            }
        })
        .unwrap();
    error(
        replace(&s, 0, 0, &[0], "replacement"),
        "reject surface candidate",
    );
    assert_eq!(nodes(&s), vec![0]);
    assert_eq!(s.surface().unwrap().replace_generation().unwrap(), 0);
    stop.dispose().await;
    user(&s, "next");
    assert_eq!(nodes(&s), vec![0, 1]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn resolves_session_event_dispatch_before_commit_so_instrumentation_failure_cannot_hide_a_logged_event()
 {
    let (k, ctx, store) = mounted().await;
    let s = store.create(&ctx, None, Default::default()).unwrap();
    ctx.on_dispatch(|_, name, e| {
        if name == "session/event" {
            e.downcast_ref::<SessionEventEmitted>()
                .unwrap()
                .validation
                .reject("dispatch instrumentation rejected the carrier".into());
        }
    })
    .unwrap();
    error(
        s.append("turn/start", json!({"turn":1}), None),
        "dispatch instrumentation rejected the carrier",
    );
    assert_eq!(s.seq(), offset(0));
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn contains_a_reentrant_observer_append_without_reordering_later_observers() {
    let (k, ctx, store) = mounted().await;
    let s = store.create(&ctx, None, Default::default()).unwrap();
    ctx.on_contained::<SessionEventEmitted>(|e| {
        e.session.append(
            "request/context",
            json!({"provider":"mock","model":"mock"}),
            None,
        )?;
        Ok(())
    })
    .unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let out = seen.clone();
    ctx.on_contained::<SessionEventEmitted>(move |e| {
        lock(&out).push(e.event.clone());
        Ok(())
    })
    .unwrap();
    let e = s.append("turn/start", json!({"turn":1}), None).unwrap();
    assert_eq!(*lock(&seen), vec![e]);
    assert_eq!(s.seq(), offset(1));
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn defers_detach_through_dispatch_resolution_commit_and_observer_publication() {
    let (k, ctx, store) = mounted().await;
    let s = store.prepare(None, Default::default()).unwrap();
    let detach = store.enter(&ctx, s.clone()).unwrap();
    store.announce(&s).unwrap();
    let order = Arc::new(Mutex::new(vec![]));
    let out = order.clone();
    let st = store.clone();
    ctx.on_dispatch(move |_, name, e| {
        if name == "session/event" {
            let e = e.downcast_ref::<SessionEventEmitted>().unwrap();
            assert_eq!(st.get(e.session.id()), Some(e.session.clone()));
            lock(&out).push("resolve:live");
            detach.detach();
        }
    })
    .unwrap();
    let out = order.clone();
    let st = store.clone();
    ctx.on_contained::<SessionEventEmitted>(move |e| {
        assert_eq!(st.get(e.session.id()), Some(e.session.clone()));
        lock(&out).push("observe:live");
        Ok(())
    })
    .unwrap();
    let out = order.clone();
    let st = store.clone();
    ctx.on_contained::<SessionDisposed>(move |e| {
        assert!(st.get(e.session.id()).is_none());
        lock(&out).push("dispose:detached");
        Ok(())
    })
    .unwrap();
    s.append("turn/start", json!({"turn":1}), None).unwrap();
    assert_eq!(
        *lock(&order),
        vec!["resolve:live", "observe:live", "dispose:detached"]
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn observes_async_created_rejection_without_rolling_back_or_starving_peers() {
    let (k, ctx, store) = mounted().await;
    ctx.on_contained_async::<SessionCreated, _>(|_| async { Err("late creation failure".into()) })
        .unwrap();
    let heard = Arc::new(Mutex::new(0));
    let out = heard.clone();
    SessionStore::on_created(&ctx, move |_| {
        *lock(&out) += 1;
        Ok(())
    })
    .unwrap();
    let s = store.create(&ctx, None, Default::default()).unwrap();
    assert_eq!(*lock(&heard), 1);
    assert_eq!(store.get(s.id()), Some(s));
    assert!(ctx.logger().buffer().iter().any(|r| {
        r.message
            .contains("session/created listener rejected: Error: late creation failure")
    }));
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn contains_synchronous_and_async_disposed_listener_failures_per_observer() {
    let (k, ctx, store) = mounted().await;
    ctx.on_contained::<SessionDisposed>(|_| Err("sync disposed".into()))
        .unwrap();
    ctx.on_contained_async::<SessionDisposed, _>(|_| async { Err("async disposed".into()) })
        .unwrap();
    let count = Arc::new(Mutex::new(0));
    let out = count.clone();
    ctx.on_contained::<SessionDisposed>(move |_| {
        *lock(&out) += 1;
        Ok(())
    })
    .unwrap();
    let unannounced = store.prepare(None, Default::default()).unwrap();
    store.enter(&ctx, unannounced).unwrap().detach();
    assert_eq!(*lock(&count), 0);
    let s = store.prepare(None, Default::default()).unwrap();
    let detach = store.enter(&ctx, s.clone()).unwrap();
    store.announce(&s).unwrap();
    detach.detach();
    assert_eq!(*lock(&count), 1);
    assert_eq!(
        ctx.logger()
            .buffer()
            .iter()
            .filter(|r| r.message.contains("session/disposed listener"))
            .count(),
        2
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn does_not_let_internal_dispatch_replace_the_disposed_callback_tuple() {
    let (k, ctx, store) = mounted().await;
    let seen = Arc::new(Mutex::new(vec![]));
    let out = seen.clone();
    ctx.on_dispatch(|_, _, _| {}).unwrap();
    ctx.on_contained::<SessionDisposed>(move |e| {
        lock(&out).push(e.session.clone());
        Ok(())
    })
    .unwrap();
    let s = store.prepare(None, Default::default()).unwrap();
    let d = store.enter(&ctx, s.clone()).unwrap();
    store.announce(&s).unwrap();
    d.detach();
    assert_eq!(*lock(&seen), vec![s]);
    k.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn contains_internal_dispatch_failure_after_session_detachment() {
    let (k, ctx, store) = mounted().await;
    let heard = Arc::new(Mutex::new(0));
    let out = heard.clone();
    ctx.on_contained::<SessionDisposed>(move |_| {
        *lock(&out) += 1;
        Ok(())
    })
    .unwrap();
    ctx.on_dispatch(|_, name, e| {
        if name == "session/disposed" {
            e.downcast_ref::<SessionDisposed>()
                .unwrap()
                .validation
                .reject("disposed dispatch instrumentation".into());
        }
    })
    .unwrap();
    let s = store
        .prepare(Some("disposed-dispatch".into()), Default::default())
        .unwrap();
    let detach = store.enter(&ctx, s.clone()).unwrap();
    store.announce(&s).unwrap();
    detach.detach();
    assert!(store.get(s.id()).is_none());
    assert_eq!(*lock(&heard), 0);
    assert!(ctx.logger().buffer().iter().any(|r|r.message=="session \"disposed-dispatch\": session/disposed dispatch threw: Error: disposed dispatch instrumentation"));
    k.shutdown().await;
}
