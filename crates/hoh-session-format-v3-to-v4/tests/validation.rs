mod common;
use common::*;
use hoh_session_format_v3_to_v4::validation::assert_released_v4_header_value;
fn conversation() -> Vec<SessionFormatEvent> {
    let mut rows = opening();
    rows.push(assistant(json!([{"type":"text","text":"calling"},{"type":"tool-call","id":"call-1","name":"bash","arguments":"{}"}])));
    rows[2]["data"]["stream"] = json!([{"type":"chunk","time":3,"chunk":{"type":"block-end","index":1,"block":{"type":"tool-call","id":"call-1","name":"bash","arguments":"{}"}}}]);
    rows.push(ev(
        "tool/call",
        json!({"turn":1,"step":1,"callId":"call-1","name":"bash","arguments":"{}"}),
    ));
    rows.push(result(native_message("call-1", "ok", Some(false.into()))));
    rows.extend(closing());
    numbered(rows, 1)
}
fn vocabulary() -> HashSet<String> {
    [
        "turn/start",
        "step/start",
        "assistant/message",
        "tool/call",
        "tool/result",
        "step/end",
        "turn/end",
        "user/message",
        "session/end-seed",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
#[test]
fn accepts_the_minimal_and_complete_header_images() {
    let mut header = h(4, "v4-session");
    assert_released_v4_header(&header).unwrap();
    for (k, v) in obj(
        json!({"cwd":"/work","parentSession":"parent","origin":"subagent","agentPreset":"headless","isSeeded":true}),
    ) {
        header.insert(k, v);
    }
    assert_released_v4_header(&header).unwrap();
}
#[test]
fn refuses_malformed_headers() {
    for (key, value, text) in [
        ("version", json!(2), "expected format v4"),
        ("id", json!(3), "id must be a string"),
        ("createdAt", json!("x"), "createdAt"),
        ("delegationDepth", json!(-1), "delegationDepth"),
        ("isSeeded", json!(1), "isSeeded must be boolean"),
        ("cwd", json!("work"), "cwd must be absolute"),
        ("parentSession", json!(3), "parentSession must be a string"),
        ("agentPreset", json!(3), "agentPreset must be a string"),
        ("origin", json!("other"), "origin must be \"subagent\""),
    ] {
        let mut header = h(4, "v4-session");
        header.insert(key.into(), value);
        err(assert_released_v4_header(&header), text);
    }
}
#[test]
fn validates_dense_envelopes_cuts_and_event_vocabulary() {
    let a = artifact(conversation(), h(4, "v4-session"), 0);
    assert_eq!(
        restore_released_v4_artifact(a.clone(), &vocabulary()).unwrap(),
        a
    );
    let mut header = h(4, "v4-session");
    header.insert("isSeeded".into(), true.into());
    err(
        restore_released_v4_artifact(artifact(conversation(), header, 20), &vocabulary()),
        "inherited event count exceeds its events",
    );
    err(
        restore_released_v4_artifact(
            artifact(
                vec![ev("session/end-seed", json!({"inherited":true}))],
                h(4, "v4-session"),
                0,
            ),
            &vocabulary(),
        ),
        "format v4 unseeded Session contains an inherited end-seed marker",
    );
    err(
        restore_released_v4_artifact(
            artifact(conversation(), h(4, "v4-session"), 1),
            &vocabulary(),
        ),
        "unseeded format v4 Session has inherited events",
    );
}
#[test]
fn retains_unknown_child_membership_and_rejects_invalid_identities_or_duplicate_child_ids() {
    let data = json!({"version":1,"childId":"unreadable","childCreatedAt":2,"mode":"unknown"});
    let event = ev("subagent/catalog", data.clone());
    assert!(native(vec![event.clone()]).is_ok());
    let mut labeled = event.clone();
    labeled["data"]["label"] = "retained".into();
    assert!(native(vec![labeled]).is_ok());
    let mut bad = vec![Value::Null];
    for (key, value) in [
        ("version", json!(0)),
        ("version", json!(2)),
        ("childId", json!(1)),
        ("childCreatedAt", json!(-1)),
        ("mode", json!("invalid")),
        ("label", Value::Null),
    ] {
        let mut d = data.clone();
        d[key] = value;
        bad.push(d);
    }
    for data in bad {
        assert!(native(vec![ev("subagent/catalog", data)]).is_err());
    }
    err(
        native(vec![event.clone(), event.clone()]),
        "duplicate catalog child",
    );
    let mut other = event.clone();
    other["data"]["mode"] = "one-shot".into();
    err(native(vec![event, other]), "duplicate catalog child");
}
#[test]
fn validates_only_the_envelope_in_physical_mode() {
    let mut rows = conversation();
    rows.insert(2, ev("campaign/unknown", json!({"x":1})));
    rows = dense(rows);
    let a = artifact(rows, h(4, "v4-session"), 0);
    hoh_session_format_v3_to_v4::testing::assert_released_v4_artifact(&a).unwrap();
    err(
        restore_released_v4_artifact(a, &vocabulary()),
        "unknown event type",
    );
}
#[test]
fn refuses_unknown_non_ignorable_types_and_admits_ignorable_ones() {
    let mut rows = conversation();
    rows.insert(2, e("campaign/unknown", 99, json!({"x":1})));
    err(
        restore_released_v4_artifact(artifact(rows, h(4, "v4-session"), 0), &vocabulary()),
        "unknown event type",
    );
    let mut event = ev("campaign/unknown", json!({"x":1}));
    event.insert("ignorable".into(), true.into());
    let mut rows = conversation();
    rows.insert(0, event);
    let rows = dense(rows);
    let mut known = vocabulary();
    known.insert("campaign/unknown".into());
    assert!(restore_released_v4_artifact(artifact(rows, h(4, "v4-session"), 0), &known).is_ok());
}
#[test]
fn enforces_v4_owned_event_density() {
    let mut missing = ev("turn/start", json!({}));
    missing.shift_remove("seq");
    err(
        restore_released_v4_artifact(
            artifact(vec![missing], h(4, "v4-session"), 0),
            &vocabulary(),
        ),
        "is not dense",
    );
    let mut rows = conversation()[..2].to_vec();
    rows[1].insert("seq".into(), 3.into());
    err(
        restore_released_v4_artifact(artifact(rows, h(4, "v4-session"), 0), &vocabulary()),
        "is not dense",
    );
}
#[test]
fn enforces_end_seed_marker_agreement() {
    let marker = ev("session/end-seed", json!({"inherited":true}));
    let mut seeded = h(4, "v4-session");
    seeded.insert("isSeeded".into(), true.into());
    assert!(
        restore_released_v4_artifact(
            artifact(vec![marker.clone()], seeded.clone(), 0),
            &vocabulary()
        )
        .is_ok()
    );
    err(
        restore_released_v4_artifact(
            artifact(
                vec![
                    ev("session/end-seed", json!({})),
                    e("session/end-seed", 1, json!({"inherited":true})),
                ],
                seeded,
                0,
            ),
            &vocabulary(),
        ),
        "seeded header disagrees",
    );
    err(
        restore_released_v4_artifact(artifact(vec![marker], h(4, "v4-session"), 0), &vocabulary()),
        "unseeded Session contains an inherited end-seed marker",
    );
}
#[test]
fn refuses_non_object_records_and_non_string_event_types() {
    let event = obj(json!({"type":3,"seq":0,"time":1,"data":{}}));
    err(
        restore_released_v4_artifact(artifact(vec![event], h(4, "v4-session"), 0), &vocabulary()),
        "unknown event type",
    );
    assert!(assert_released_v4_header_value(&Value::Null).is_err());
}
