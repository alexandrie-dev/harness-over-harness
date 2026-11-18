mod common;
use common::*;
use hoh_session_format::*;
use hoh_session_format_v1_to_v2::{
    testing::assert_released_v2_artifact, validation::assert_released_v2_header_value, *,
};
use serde_json::{Value, json};
use std::collections::HashSet;
fn known() -> HashSet<String> {
    RELEASED_V2_EVENT_TYPES
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}
fn restore(a: SessionFormatArtifact) -> Result<SessionFormatArtifact, SessionFormatError> {
    restore_released_v2_artifact(a, &known(), None)
}
fn check(a: SessionFormatArtifact) -> Result<(), SessionFormatError> {
    assert_released_v2_artifact(&a)
}
fn assistant_data() -> Value {
    json!({"turn":1,"step":1,"message":{"id":"assistant","role":"assistant","content":[{"type":"text","text":"hello"}],"source":{"kind":"model","provider":"mock","model":"mock","replayState":{"response":{"id":"response"},"blocks":["text-meta"]}}},"stream":[{"type":"text-chunks","time0":3,"index":0,"dt":[],"texts":["hello"]},{"type":"chunk","time":4,"chunk":{"type":"usage","usage":{"inputTokens":3,"outputTokens":2}}},{"type":"chunk","time":5,"chunk":{"type":"finish","reason":{"kind":"stop"},"replayState":{"response":{"id":"response"},"blocks":["text-meta"]}}}],"usage":{"inputTokens":3,"outputTokens":2}})
}
fn empty_assistant() -> Value {
    let mut d = assistant_data();
    d["message"]["content"] = json!([]);
    d["message"]["source"]
        .as_object_mut()
        .unwrap()
        .shift_remove("replayState");
    d["stream"] = json!([]);
    d.as_object_mut().unwrap().shift_remove("usage");
    d
}
fn lifecycle(kind: &str, data: Value) -> SessionFormatArtifact {
    let mut e = ev(kind, 2, data);
    if kind == "assistant/message" {
        e.insert("surfaceOp".into(), "append".into());
    }
    artifact(vec![
        ev("turn/start", 0, json!({"turn":1})),
        ev("step/start", 1, json!({"turn":1,"step":1})),
        e,
        ev("step/end", 3, json!({"turn":1,"step":1})),
        ev(
            "turn/end",
            4,
            json!({"turn":1,"reason":{"kind":"completed"}}),
        ),
    ])
}
fn seeded(events: Vec<SessionFormatEvent>, cut: u64) -> SessionFormatArtifact {
    let mut a = artifact(events);
    a.header.insert("id".into(), "seeded".into());
    a.header.insert("isSeeded".into(), true.into());
    a.inherited_event_count = cut;
    a
}

#[test]
fn accepts_minimal_and_complete_logical_headers() {
    let mut h = header(2, "validation");
    assert_released_v2_header(&h).unwrap();
    for k in ["cwd", "parentSession", "origin", "agentPreset"] {
        h.insert(
            k.into(),
            json!(match k {
                "cwd" => "/work",
                "parentSession" => "parent",
                "origin" => "subagent",
                _ => "default",
            }),
        );
    }
    assert_released_v2_header(&h).unwrap();
}
#[test]
fn rejects_exact_logical_header_drift() {
    for value in [Value::Null, json!([]), json!("header")] {
        err(assert_released_v2_header_value(&value), "must be an object");
    }
    let mut h = header(2, "validation");
    h.shift_remove("id");
    err(assert_released_v2_header(&h), "lacks required field id");
    for (key, v, msg) in [
        ("extra", json!(true), "unexpected field extra"),
        ("version", json!(1), "expected format v2"),
        ("id", json!(1), "id must be a string"),
        ("createdAt", json!(-1), "createdAt"),
        ("delegationDepth", json!(-1), "delegationDepth"),
        ("isSeeded", json!(1), "isSeeded must be boolean"),
        ("cwd", json!(1), "cwd must be absolute"),
        ("cwd", json!("relative"), "cwd must be absolute"),
        ("parentSession", json!(1), "parentSession must be a string"),
        ("agentPreset", json!(1), "agentPreset must be a string"),
        ("origin", json!("parent"), "origin must be \"subagent\""),
    ] {
        let mut h = header(2, "validation");
        h.insert(key.into(), v);
        err(assert_released_v2_header(&h), msg);
    }
}
#[test]
fn accepts_empty_artifacts_and_ordinary_log_events_with_a_true_ignorable_marker() {
    check(artifact(vec![])).unwrap();
    let mut e = ev("feedback/record", 0, json!({"text":"feedback"}));
    e.insert("ignorable".into(), true.into());
    check(artifact(vec![e])).unwrap();
}
#[test]
fn preserves_the_referenced_session_generation_across_containing_log_migration() {
    for version in 0..=3 {
        let mut e = user(0, None);
        e["data"]["source"] = json!({"kind":"session-reference","form":"recall","version":1,"references":[{"sessionId":"source","label":"Source","capturedFormatVersion":version,"capturedThroughSeq":0,"compacted":false,"originalMessages":1,"retainedMessages":1,"omittedMessages":0,"omittedBytes":0,"truncated":false,"inputIndex":0}]});
        let result = check(artifact(vec![e]));
        if (1..=2).contains(&version) {
            result.unwrap();
        } else {
            err(result, "capturedFormatVersion");
        }
    }
}
fn drift_cases() -> Vec<(SessionFormatArtifact, &'static str)> {
    let mut beyond = artifact(vec![]);
    beyond.inherited_event_count = 1;
    let mut unseeded = artifact(vec![feedback(0)]);
    unseeded.inherited_event_count = 1;
    let mut cases = vec![
        (beyond, "exceeds its events"),
        (unseeded, "unseeded.*inherited events"),
    ];
    for (kind, data, optional, msg) in [
        (
            "feedback/record",
            json!({"text":"x"}),
            json!({"type":1}),
            "type must be a string",
        ),
        (
            "external/unknown",
            json!({}),
            json!({"ignorable":true}),
            "unknown event type.*external/unknown",
        ),
        (
            "feedback/record",
            json!({"text":"x"}),
            json!({"surfaceOp":"append"}),
            "unexpected field surfaceOp",
        ),
        (
            "user/message",
            user_data("user", "text"),
            json!({"surfaceOp":"append","extra":true}),
            "unexpected field extra",
        ),
        (
            "feedback/record",
            json!({"text":"x"}),
            json!({"seq":1}),
            "not dense",
        ),
        (
            "feedback/record",
            json!({"text":"x"}),
            json!({"time":0.5}),
            "time",
        ),
        (
            "feedback/record",
            json!({"text":"x"}),
            json!({"ignorable":false}),
            "ignorable must be true",
        ),
        (
            "feedback/record",
            Value::Null,
            json!({}),
            "data must be an object",
        ),
        (
            "feedback/record",
            json!({}),
            json!({}),
            "lacks required field text",
        ),
        (
            "feedback/record",
            json!({"text":"x","extra":true}),
            json!({}),
            "unexpected field extra",
        ),
        (
            "feedback/record",
            json!({"text":""}),
            json!({}),
            "non-empty string",
        ),
    ] {
        let mut e = ev(kind, 0, data);
        e.extend(obj(optional));
        cases.push((artifact(vec![e]), msg));
    }
    let mut missing = feedback(0);
    missing.shift_remove("data");
    cases.push((artifact(vec![missing]), "lacks required field data"));
    cases
}
#[test]
fn rejects_exact_event_or_payload_drift() {
    // null event is excluded by the base artifact type.
    for (a, msg) in drift_cases() {
        err_pattern(check(a), msg);
    }
}
#[test]
fn rejects_production_artifact_drift_before_payload_restoration() {
    for (a, msg) in drift_cases().into_iter().filter(|(_, msg)| {
        [
            "exceeds its events",
            "unseeded.*inherited events",
            "type must be a string",
            "not dense",
            "ignorable must be true",
        ]
        .contains(msg)
    }) {
        err_pattern(restore(a), msg);
    }
}
#[test]
fn validates_present_and_absent_opaque_payload_members_before_relationship_checks() {
    for include_meta in [false, true] {
        let mut data = json!({"turn":1,"step":1,"message":{"id":"tool","role":"user","content":[{"type":"tool-result","toolCallId":"call","content":[{"type":"text","text":"hello"}],"isError":false}],"source":{"kind":"tool","callId":"call"}}});
        if include_meta {
            data["meta"] = json!({"durable":true});
        }
        let mut e = ev("tool/result", 2, data);
        e.insert("surfaceOp".into(), "append".into());
        err(
            check(artifact(vec![
                ev("turn/start", 0, json!({"turn":1})),
                ev("step/start", 1, json!({"turn":1,"step":1})),
                e,
            ])),
            "no advertised tool lifecycle",
        );
    }
    // JS undefined cannot inhabit Value. Negative zero exercises the opaque JSON guard.
    let mut e = ev(
        "tool/result",
        2,
        json!({"turn":1,"step":1,"message":{},"meta":{"callback":-0.0}}),
    );
    e.insert("surfaceOp".into(), "append".into());
    err(
        check(artifact(vec![
            ev("turn/start", 0, json!({"turn":1})),
            ev("step/start", 1, json!({"turn":1,"step":1})),
            e,
        ])),
        "not lossless JSON",
    );
}
#[test]
fn accepts_successful_interrupted_legacy_empty_and_failed_attempt_streams() {
    check(lifecycle("assistant/message", assistant_data())).unwrap();
    let mut d = assistant_data();
    d["interrupted"] = true.into();
    check(lifecycle("assistant/message", d)).unwrap();
    check(lifecycle("assistant/message", empty_assistant())).unwrap();
    check(lifecycle("assistant/attempt",json!({"turn":1,"step":1,"stream":[{"type":"text-chunks","time0":3,"index":0,"dt":[1],"texts":["a","b"]}]}))).unwrap();
}
#[test]
fn rejects_an_invalid_embedded_stream_or_projection() {
    for stream in [
        Value::Null,
        json!([{"type":"future"}]),
        json!([{"type":"chunk","time":3,"chunk":{"type":"future"}}]),
    ] {
        err(
            check(lifecycle(
                "assistant/attempt",
                json!({"turn":1,"step":1,"stream":stream}),
            )),
            "invalid embedded stream",
        );
        let failure = check(lifecycle(
            "assistant/attempt",
            json!({"turn":1,"step":1,"stream":stream}),
        ))
        .unwrap_err();
        assert!(std::error::Error::source(&failure).is_some(), "{failure:?}");
    }
    for key in ["turn", "step"] {
        let mut d = json!({"turn":1,"step":1,"stream":[]});
        d[key] = json!(-1);
        err(check(lifecycle("assistant/attempt", d)), key);
    }
    let mut content = assistant_data();
    content["message"]["content"] = json!([{"type":"text","text":"different"}]);
    err(
        check(lifecycle("assistant/message", content)),
        "content disagrees",
    );
    let mut usage = assistant_data();
    usage["usage"]["inputTokens"] = 9.into();
    err(
        check(lifecycle("assistant/message", usage)),
        "usage disagrees",
    );
    let mut replay = assistant_data();
    replay["message"]["source"]["replayState"]["response"]["id"] = json!("different");
    err(
        check(lifecycle("assistant/message", replay)),
        "replay state disagrees",
    );
}

#[test]
fn error_assertions_reject_an_unrelated_error_with_the_same_text() {
    assert!(
        std::panic::catch_unwind(|| {
            err::<()>(
                Err(SessionFormatError::other(std::io::Error::other(
                    "same text",
                ))),
                "same text",
            );
        })
        .is_err()
    );
}

#[test]
fn error_patterns_reject_seeded_lineage_and_the_wrong_unknown_type() {
    for (message, pattern) in [
        (
            "format v2 seeded header disagrees with its last inherited end-seed marker",
            "unseeded.*inherited end-seed",
        ),
        (
            "format v2 contains unknown event type external/other at seq 0",
            "unknown event type.*external/unknown",
        ),
        (
            "seeded format v2 Session has inherited events",
            "unseeded.*inherited events",
        ),
    ] {
        assert!(
            std::panic::catch_unwind(|| {
                err_pattern::<()>(Err(SessionFormatError::new(message)), pattern);
            })
            .is_err()
        );
    }
}

#[test]
fn released_event_types_are_the_sorted_disposition_keys() {
    let mut keys: Vec<_> = RELEASED_V2_EVENT_DISPOSITIONS
        .iter()
        .map(|(kind, _)| kind)
        .collect();
    // The frozen ASCII inventory has the same order under English localeCompare.
    keys.sort_unstable();
    assert_eq!(RELEASED_V2_EVENT_TYPES, keys);
}
#[test]
fn accepts_tagged_seeded_lineage_and_an_ordinary_unseeded_marker() {
    check(seeded(
        vec![ev("session/end-seed", 0, json!({"inherited":true}))],
        0,
    ))
    .unwrap();
    check(artifact(vec![ev("session/end-seed", 0, json!({}))])).unwrap();
}
#[test]
fn rejects_seed_lineage_disagreement() {
    err(
        check(seeded(
            vec![ev("session/end-seed", 0, json!({"inherited":true}))],
            1,
        )),
        "seeded header disagrees",
    );
    err(check(seeded(vec![], 0)), "seeded header disagrees");
    err_pattern(
        check(artifact(vec![ev(
            "session/end-seed",
            0,
            json!({"inherited":true}),
        )])),
        "unseeded.*inherited end-seed",
    );
    err(
        check(artifact(vec![ev(
            "session/end-seed",
            0,
            json!({"inherited":false}),
        )])),
        "inherited must be true",
    );
}
#[test]
fn rejects_production_seed_lineage_disagreement() {
    err(restore(seeded(vec![], 0)), "seeded header disagrees");
    err_pattern(
        restore(artifact(vec![ev(
            "session/end-seed",
            0,
            json!({"inherited":true}),
        )])),
        "unseeded.*inherited end-seed",
    );
}
#[test]
fn accepts_append_and_exact_replacement_surface_operations() {
    check(artifact(vec![user(0, None)])).unwrap();
    let mut e = user(1, Some(json!([0])));
    e.insert(
        "surfaceOp".into(),
        json!({"op":"replace","start":0,"end":0}),
    );
    check(artifact(vec![user(0, None), e])).unwrap();
}
#[test]
fn rejects_invalid_surface_metadata_or_relationships() {
    let mut assistant = ev("assistant/message", 1, empty_assistant());
    assistant.insert("surfaceOp".into(), "append".into());
    assistant.insert("sourceEventSeqs".into(), json!([0]));
    err(
        check(artifact(vec![user(0, None), assistant])),
        "obsolete chunk references",
    );
    for (sources, msg) in [
        (json!(0), "must be an array"),
        (json!([-1]), "sourceEventSeqs member"),
        (json!([1]), "unique earlier seqs"),
        (json!([]), "must be non-empty"),
    ] {
        err(
            check(artifact(vec![feedback(0), user(1, Some(sources))])),
            msg,
        );
    }
    err(
        check(artifact(vec![
            feedback(0),
            feedback(1),
            user(2, Some(json!([0, 0]))),
        ])),
        "unique earlier seqs",
    );
    let mut missing = user(0, None);
    missing.shift_remove("surfaceOp");
    err(check(artifact(vec![missing])), "requires a surfaceOp");
    for (op, msg) in [
        (json!(true), "surfaceOp must be a JSON object"),
        (
            json!({"op":"replace","start":0}),
            "lacks required member \"end\"",
        ),
        (
            json!({"op":"replace","start":0,"end":0,"extra":true}),
            "unexpected member \"extra\"",
        ),
        (
            json!({"op":"append","start":0,"end":0}),
            "surfaceOp must replace",
        ),
        (json!({"op":"replace","start":-1,"end":0}), "surface start"),
        (json!({"op":"replace","start":0,"end":-1}), "surface end"),
        (
            json!({"op":"replace","start":1,"end":0}),
            "invalid surface replacement",
        ),
        (
            json!({"op":"replace","start":0,"end":1}),
            "invalid surface replacement",
        ),
    ] {
        let mut e = user(1, None);
        e.insert("surfaceOp".into(), op);
        err(check(artifact(vec![feedback(0), e])), msg);
    }
    let mut e = user(1, Some(json!([0])));
    e.insert(
        "surfaceOp".into(),
        json!({"op":"replace","start":0,"end":0}),
    );
    err(
        check(artifact(vec![feedback(0), e])),
        "range is not on the current surface",
    );
}
#[test]
fn reports_cross_event_relationship_errors_after_exact_local_validation() {
    err(
        check(artifact(vec![ev(
            "turn/end",
            0,
            json!({"turn":1,"reason":{"kind":"completed"}}),
        )])),
        "no matching open turn",
    );
    err(
        check(artifact(vec![ev(
            "session-log-deepseek/delivery-accepted",
            0,
            json!({"sessionId":"other","throughSeq":0,"sessionFormatVersion":2}),
        )])),
        "throughSeq",
    );
}
#[test]
fn returns_the_same_validated_artifact_and_preserves_safe_vocabulary_extensions() {
    let empty = artifact(vec![]);
    assert_eq!(restore(empty.clone()).unwrap(), empty);
    let mut e = ev("external/ignorable", 0, json!({"retained":true}));
    e.insert("ignorable".into(), true.into());
    let a = artifact(vec![e]);
    assert_eq!(restore(a.clone()).unwrap(), a);
    let installed = artifact(vec![ev("external/installed", 0, json!({"retained":true}))]);
    let mut types = known();
    types.insert("external/installed".into());
    assert_eq!(
        restore_released_v2_artifact(installed.clone(), &types, None).unwrap(),
        installed
    );
    let extended = artifact(vec![ev(
        "turn/start",
        0,
        json!({"turn":1,"postReleaseMember":true}),
    )]);
    assert_eq!(restore(extended.clone()).unwrap(), extended);
    err(
        restore(artifact(vec![ev("external/unknown", 0, json!({}))])),
        "unknown event type",
    );
}
