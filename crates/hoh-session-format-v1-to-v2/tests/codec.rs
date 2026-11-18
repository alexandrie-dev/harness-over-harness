mod common;
use common::*;
use hoh_session_format::*;
use hoh_session_format_v1_to_v2::*;
use hoh_util_values::js_json_stringify;
use serde_json::{Value, json};
fn strict(rows: Vec<Value>) -> Result<SessionFormatArtifact, SessionFormatError> {
    decode(&physical(), rows, SessionFormatRecovery::Strict)
}
fn recover(rows: Vec<Value>) -> Result<SessionFormatArtifact, SessionFormatError> {
    decode(&physical(), rows, SessionFormatRecovery::Recoverable)
}

#[test]
fn round_trips_the_minimal_and_complete_optional_header_images() {
    let codec = RELEASED_V2_SESSION_FORMAT_CODEC;
    assert_eq!(codec.version(), 2);
    assert_eq!(
        codec.decode_header(&physical()).unwrap(),
        header(2, "codec")
    );
    let mut full = physical();
    for (k, v) in [
        ("cwd", json!("/work")),
        ("parentSession", json!("parent")),
        ("origin", json!("subagent")),
        ("agentPreset", json!("default")),
    ] {
        full[k] = v;
    }
    let h = codec.decode_header(&full).unwrap();
    assert_eq!(
        js_json_stringify(&Value::Object(h.clone())),
        r#"{"version":2,"id":"codec","createdAt":1,"cwd":"/work","parentSession":"parent","isSeeded":false,"origin":"subagent","delegationDepth":0,"agentPreset":"default"}"#
    );
    assert_eq!(codec.encode_header(&h, 0).unwrap(), obj(full));
    assert_eq!(
        codec.encode_header(&header(2, "codec"), 0).unwrap(),
        obj(physical())
    );
    err_pattern(
        codec.encode_header(&header(2, "codec"), 1),
        "unseeded.*inherited events",
    );
}
#[test]
fn refuses_a_malformed_physical_header() {
    let mut missing = physical();
    missing.as_object_mut().unwrap().shift_remove("id");
    let mut cases = vec![
        (Value::Null, "must be an object"),
        (json!([]), "must be an object"),
        (json!("session"), "must be an object"),
        (missing, "lacks id"),
    ];
    for (key, value, message) in [
        ("seedLength", json!(0), "unexpected field seedLength"),
        ("type", json!("other"), "expected released v2"),
        ("version", json!(1), "expected released v2"),
        ("id", json!(1), "id must be a string"),
        ("createdAt", json!(-1), "createdAt"),
        ("delegationDepth", json!(-1), "delegationDepth"),
        ("isSeeded", json!(0), "isSeeded must be boolean"),
        ("cwd", json!(1), "cwd must be a string"),
        ("cwd", json!("relative"), "cwd must be absolute"),
        ("parentSession", json!(1), "parentSession must be a string"),
        ("agentPreset", json!(1), "agentPreset must be a string"),
        ("origin", json!("user"), "origin must be \"subagent\""),
    ] {
        let mut h = physical();
        h[key] = value;
        cases.push((h, message));
    }
    for (value, message) in cases {
        err(
            RELEASED_V2_SESSION_FORMAT_CODEC.decode_header(&value),
            message,
        );
    }
}
#[test]
fn stores_one_event_per_row_and_compacts_scalar_pair_and_long_run_source_event_references_exactly()
{
    let events = vec![
        feedback(0),
        user(1, Some(json!([0]))),
        user(2, Some(json!([0, 1]))),
        user(3, Some(json!([0, 1, 2]))),
    ];
    let rows: Vec<_> = events
        .iter()
        .map(|e| Value::Object(RELEASED_V2_SESSION_FORMAT_CODEC.encode_event(e).unwrap()))
        .collect();
    assert!(rows[0].get("sourceEventSeqs").is_none());
    assert_eq!(rows[1]["sourceEventSeqs"], json!([0]));
    assert_eq!(rows[2]["sourceEventSeqs"], json!([0, 1]));
    assert_eq!(rows[3]["sourceEventSeqs"], json!([[0, 2]]));
    assert_eq!(strict(rows).unwrap().events, events);
}
#[test]
fn keeps_non_monotonic_source_event_references_scalar_only_for_the_current_backend_reader() {
    let mut events: Vec<SessionFormatEvent> = (0..6).map(feedback).collect();
    events.push(user(6, Some(json!([4, 5, 1, 2, 3]))));
    let rows: Vec<_> = events
        .iter()
        .map(|e| Value::Object(RELEASED_V2_SESSION_FORMAT_CODEC.encode_event(e).unwrap()))
        .collect();
    assert_eq!(rows[6]["sourceEventSeqs"], json!([4, 5, 1, 2, 3]));
    assert_eq!(strict(rows).unwrap().events, events);
}
#[test]
fn keeps_the_v2_physical_codec_vocabulary_neutral_for_current_growth_and_a_future_source_freeze() {
    let mut ignorable = ev("external/ignorable", 1, json!({"retained":true}));
    ignorable.insert("ignorable".into(), true.into());
    let events = vec![
        ev("external/required", 0, json!({"retained":true})),
        ignorable,
        ev("turn/start", 2, json!({"turn":1,"postReleaseMember":true})),
    ];
    let rows = events
        .iter()
        .map(|e| Value::Object(RELEASED_V2_SESSION_FORMAT_CODEC.encode_event(e).unwrap()))
        .collect();
    assert_eq!(strict(rows).unwrap().events, events);
}
#[test]
fn expands_mixed_stored_source_event_ranges_and_preserves_unrelated_rows() {
    let mut rows = values((0..5).map(feedback).collect());
    rows.push(Value::Object(user(5, Some(json!([[0, 2], 4])))));
    let decoded = strict(rows).unwrap();
    assert_eq!(decoded.events[0], feedback(0));
    assert_eq!(decoded.events[5]["sourceEventSeqs"], json!([0, 1, 2, 4]));
    assert_eq!(
        strict(values(vec![
            feedback(0),
            feedback(1),
            user(2, Some(json!([1, 0])))
        ]))
        .unwrap()
        .events[2]["sourceEventSeqs"],
        json!([1, 0])
    );
}
#[test]
fn refuses_malformed_stored_source_event_ranges() {
    for (sources, message) in [
        (json!("bad"), "must be an array"),
        (json!([-1]), "sourceEventSeqs member"),
        (json!([[0]]), "must be a [start, end] pair"),
        (json!([[-1, 0]]), "range start"),
        (json!([[0, -1]]), "range end"),
        (json!([[2, 1]]), "range exceeds"),
        (json!([[0, 4]]), "range exceeds"),
        (json!([0, 1, 2, [0, 1]]), "range exceeds"),
        (json!([4]), "unique earlier"),
        (json!([0, 0]), "unique earlier"),
        (json!([[0, 1], 1]), "unique earlier"),
        (json!([3, [0, 2]]), "strictly increasing"),
    ] {
        let mut rows = values((0..4).map(feedback).collect());
        rows.push(Value::Object(user(4, Some(sources))));
        err(strict(rows), message);
    }
}
#[test]
fn contains_ordinary_and_non_session_format_error_row_failures_in_a_recoverable_tail() {
    // Proxy/throwing ownKeys do not exist at the Rust JSON boundary (PORT_NOTES).
    assert_eq!(
        recover(vec![Value::Object(feedback(0)), Value::Null])
            .unwrap()
            .events,
        vec![feedback(0)]
    );
    assert!(
        recover(vec![Value::Null, json!([]), Value::Object(feedback(0))])
            .unwrap()
            .events
            .is_empty()
    );
}
#[test]
fn refuses_malformed_strict_rows_strict_gaps_and_terminal_recoverable_tails() {
    err(strict(vec![Value::Null]), "row 0");
    err(strict(values(vec![feedback(1)])), "seq gap");
    err(
        recover(values(vec![ev(
            "turn/end",
            1,
            json!({"turn":1,"reason":{"kind":"completed"}}),
        )])),
        "seq gap",
    );
    err(
        recover(vec![
            Value::Null,
            Value::Object(ev(
                "turn/end",
                0,
                json!({"turn":1,"reason":{"kind":"completed"}}),
            )),
        ]),
        "row 0",
    );
    for (key, v, msg) in [
        ("extra", json!(true), "unexpected field extra"),
        ("type", json!(1), "type must be a string"),
        ("ignorable", json!(false), "ignorable must be true"),
    ] {
        let mut row = feedback(0);
        row.insert(key.into(), v);
        err(strict(values(vec![row])), msg);
    }
    err(
        strict(vec![json!({"type":"feedback/record","seq":0,"time":1})]),
        "lacks required field data",
    );
}
#[test]
fn keeps_the_prefix_before_a_recoverable_non_terminal_seq_gap() {
    assert_eq!(
        recover(values(vec![feedback(0), feedback(3), feedback(1)]))
            .unwrap()
            .events,
        vec![feedback(0)]
    );
}
#[test]
fn derives_the_last_tagged_seed_marker_and_rejects_lineage_disagreements() {
    let mut h = physical();
    h["id"] = json!("seeded");
    h["isSeeded"] = true.into();
    let markers = values(vec![
        ev("session/end-seed", 0, json!({"inherited":true})),
        ev("session/end-seed", 1, json!({})),
        ev("session/end-seed", 2, json!({"inherited":true})),
    ]);
    assert_eq!(
        decode(&h, markers.clone(), SessionFormatRecovery::Strict)
            .unwrap()
            .inherited_event_count,
        2
    );
    assert_eq!(
        decode(&h, vec![markers[0].clone()], SessionFormatRecovery::Strict)
            .unwrap()
            .inherited_event_count,
        0
    );
    err(
        decode(&h, vec![], SessionFormatRecovery::Strict),
        "lacks an inherited end-seed marker",
    );
    err(
        strict(vec![markers[0].clone()]),
        "unseeded Session contains an inherited end-seed marker",
    );
    err(
        decode(
            &h,
            values(vec![ev("session/end-seed", 0, Value::Null)]),
            SessionFormatRecovery::Strict,
        ),
        "must be an object",
    );
}
