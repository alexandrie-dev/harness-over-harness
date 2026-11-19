mod common;
use common::*;
use hoh_session_format::*;
use hoh_session_format_v2_to_v3::*;
use serde_json::{Value, json};
fn header(version: u64) -> SessionFormatHeader {
    h(version, "canonical:code:session")
}
fn event(kind: &str, seq: u64, data: Value) -> SessionFormatEvent {
    let mut row = e(kind, seq, data);
    row.insert("time".into(), (seq as i64 - 20).into());
    row
}
fn config() -> Value {
    json!({"provider":"mock","model":"mock","stop":[]})
}
fn message() -> Value {
    json!({"id":"message:code:1","role":"user","source":{"kind":"plugin","plugin":"tools-code-mode"},"content":[{"type":"text","text":"tools-code-mode tool/code-dispatch code 图片"}]})
}
fn dispatch() -> Value {
    json!({"rootCallId":"root:code:1","parentCallId":"root:code:1","subCallId":"root:code:1:code:2","name":"run_code","arguments":{"code":"return 3","source":{"kind":"plugin","plugin":"tools-code-mode"},"start":4,"end":6}})
}
fn seeded() -> SessionFormatHeader {
    let mut h = header(2);
    h.insert("isSeeded".into(), true.into());
    h.insert("parentSession".into(), "parent".into());
    h.insert("agentPreset".into(), "code".into());
    h
}
fn history() -> Vec<SessionFormatEvent> {
    let mut first = event("user/message", 4, message());
    first.insert("surfaceOp".into(), "append".into());
    let mut second = first.clone();
    second.insert("seq".into(), 16.into());
    second.insert("time".into(), (-4).into());
    second["data"]["id"] = "message:code:2".into();
    second.insert(
        "surfaceOp".into(),
        json!({"op":"replace","start":4,"end":4}),
    );
    second.insert("sourceEventSeqs".into(), json!([4, 6]));
    let mut settle = dispatch();
    settle["isError"] = true.into();
    settle["content"] = message()["content"].clone();
    vec![
        event("agent-preset/selected", 0, json!({"agentPreset":"code"})),
        event("turn/start", 1, json!({"turn":1})),
        event("step/start", 2, json!({"turn":1,"step":1})),
        event(
            "request/header",
            3,
            json!({"reason":"initial","header":{"config":config(),"system":"seed prompt","tools":[],"adapterDefaults":{}}}),
        ),
        first,
        event("tool/code-dispatch-start", 5, dispatch()),
        event("tool/code-dispatch", 6, settle),
        event("step/end", 7, json!({"turn":1,"step":1})),
        event(
            "turn/end",
            8,
            json!({"turn":1,"reason":{"kind":"completed"}}),
        ),
        event(
            "session-log-deepseek/delivery-accepted",
            9,
            json!({"sessionId":"parent","throughSeq":8,"sessionFormatVersion":2}),
        ),
        event("session/end-seed", 10, json!({"inherited":true})),
        event(
            "agent-preset/selected",
            11,
            json!({"agentPreset":"standard"}),
        ),
        event("agent-preset/selected", 12, json!({"agentPreset":"code"})),
        event("turn/start", 13, json!({"turn":2})),
        event("step/start", 14, json!({"turn":2,"step":1})),
        event(
            "request/header",
            15,
            json!({"reason":"change","header":{"config":config(),"system":"local prompt","tools":[],"adapterDefaults":{}}}),
        ),
        second,
        event("step/end", 17, json!({"turn":2,"step":1})),
        event(
            "turn/end",
            18,
            json!({"turn":2,"reason":{"kind":"completed"}}),
        ),
    ]
}
fn restore_rows(
    rows: Vec<SessionFormatEvent>,
    header: SessionFormatHeader,
) -> R<SessionFormatArtifact> {
    read(
        physical(header),
        rows,
        SessionFormatRecovery::Strict,
        SessionFormatValidation::Current,
    )
}
#[test]
fn keeps_empty_stop_arrays_and_nested_tool_schema_values_while_omitting_only_empty_header_optionals()
 {
    let tools = json!([{"name":"run_code","description":"","parameters":{"type":"object","properties":{},"required":[],"tools":[],"adapterDefaults":{},"system":"","source":{"kind":"plugin","plugin":"tools-code-mode"}}}]);
    let first = event(
        "request/header",
        2,
        json!({"reason":"initial","header":{"config":config(),"system":" \n","tools":tools,"adapterDefaults":{}}}),
    );
    let second = event(
        "request/header",
        3,
        json!({"reason":"series","header":{"config":config(),"system":" \n","tools":[],"adapterDefaults":{}}}),
    );
    let source = vec![
        event("turn/start", 0, json!({"turn":1})),
        event("step/start", 1, json!({"turn":1,"step":1})),
        first.clone(),
        second.clone(),
    ];
    let before = json!(source);
    let a = restore_rows(source.clone(), header(2)).unwrap();
    let mut expected_first = first;
    expected_first.insert("seq".into(), 4.into());
    expected_first["data"]["header"] = json!({"config":config(),"tools":tools});
    let mut expected_second = second;
    expected_second.insert("seq".into(), 5.into());
    expected_second["data"]["header"] = json!({"config":config()});
    assert_eq!(
        a.events
            .iter()
            .filter(|e| e["type"] == "request/header")
            .cloned()
            .collect::<Vec<_>>(),
        vec![expected_first, expected_second]
    );
    assert_eq!(
        systems(&a)
            .iter()
            .map(|e| e["data"]["message"]["content"].clone())
            .collect::<Vec<_>>(),
        vec![json!([]), json!([{"type":"text","text":" \n"}])]
    );
    assert_eq!(round_trip(a.clone()).unwrap(), a);
    assert_eq!(json!(source), before);
}
#[test]
fn preserves_native_code_presets_attribution_and_nested_canonical_looking_extension_values() {
    let mut h = header(3);
    h.insert("agentPreset".into(), "code".into());
    let extension = json!({"tools":[],"adapterDefaults":{},"system":"","surfaceOp":{"op":"replace","start":90,"end":99},"agentPreset":"code"});
    let mut user = event("user/message", 3, message());
    user["data"]["source"]["extension"] = extension.clone();
    user.insert("surfaceOp".into(), "append".into());
    let rows = vec![
        event(
            "agent-preset/selected",
            0,
            json!({"agentPreset":"code","extension":extension}),
        ),
        event("turn/start", 1, json!({"turn":1})),
        event("step/start", 2, json!({"turn":1,"step":1})),
        user,
        event(
            "request/header",
            4,
            json!({"reason":"initial","header":{"config":config(),"extension":extension}}),
        ),
    ];
    let before = json!({"header":h,"rows":rows});
    let a = restore_rows(rows.clone(), h.clone()).unwrap();
    assert_eq!(
        a,
        SessionFormatArtifact {
            header: h.clone(),
            inherited_event_count: 0,
            events: rows.clone()
        }
    );
    assert_eq!(round_trip(a.clone()).unwrap(), a);
    assert_eq!(json!({"header":h,"rows":rows}), before);
}
#[test]
fn composes_inherited_and_local_preset_selections_with_shifted_heads_ptc_failure_replacement_and_historical_delivery()
 {
    let source = history();
    let before = json!({"header":seeded(),"source":source});
    let a = restore_rows(source.clone(), seeded()).unwrap();
    let mapped = [
        0, 1, 2, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 18, 19, 20, 21,
    ];
    let mut expected_header = seeded();
    expected_header.insert("version".into(), 3.into());
    expected_header.insert("agentPreset".into(), "ptc".into());
    assert_eq!(a.header, expected_header);
    assert_eq!(a.inherited_event_count, 12);
    let expected: Vec<_> = source
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let mut row = row.clone();
            row.insert("seq".into(), mapped[index].into());
            match row["type"].as_str().unwrap() {
                "agent-preset/selected" => {
                    if row["data"]["agentPreset"] == "code" {
                        row["data"]["agentPreset"] = "ptc".into();
                    }
                }
                "request/header" => row["data"]["header"] = json!({"config":config()}),
                "user/message" => {
                    row["data"]["source"] = json!({"kind":"plugin","plugin":"tools-ptc"})
                }
                "tool/code-dispatch-start" => {
                    row.insert("type".into(), "tool/ptc-dispatch-start".into());
                }
                "tool/code-dispatch" => {
                    row.insert("type".into(), "tool/ptc-dispatch".into());
                }
                _ => {}
            }
            if index == 16 {
                row.insert(
                    "surfaceOp".into(),
                    json!({"op":"replace","startSeq":6,"endSeq":6}),
                );
                row.insert("sourceEventSeqs".into(), json!([6, 8]));
            }
            row
        })
        .collect();
    assert_eq!(
        a.events
            .iter()
            .filter(|e| e["type"] != "system/message")
            .cloned()
            .collect::<Vec<_>>(),
        expected
    );
    let actual:Vec<_>=systems(&a).iter().map(|e|json!({"seq":e["seq"],"time":e["time"],"surfaceOp":e["surfaceOp"],"sourceEventSeqs":e.get("sourceEventSeqs")})).collect();
    assert_eq!(
        actual,
        vec![
            json!({"seq":3,"time":-18,"surfaceOp":"append","sourceEventSeqs":null}),
            json!({"seq":4,"time":-17,"surfaceOp":{"op":"replace","startSeq":3,"endSeq":3},"sourceEventSeqs":[3]}),
            json!({"seq":17,"time":-5,"surfaceOp":{"op":"replace","startSeq":4,"endSeq":4},"sourceEventSeqs":[4]})
        ]
    );
    let mut expected = source[10].clone();
    expected.insert("seq".into(), 12.into());
    assert_eq!(a.events[12], expected);
    assert_eq!(round_trip(a.clone()).unwrap(), a);
    assert_eq!(json!({"header":seeded(),"source":source}), before);
}
#[test]
fn does_not_treat_ignorable_known_events_as_opaque_envelopes() {
    for kind in [
        "tool/ptc-dispatch-start",
        "tool/ptc-dispatch",
        "agent-preset/selected",
        "session-log-deepseek/delivery-accepted",
    ] {
        let a = restore_rows(history(), seeded()).unwrap();
        let index = a.events.iter().rposition(|e| e["type"] == kind).unwrap();
        for metadata in [
            json!({"surfaceOp":"append"}),
            json!({"sourceEventSeqs":[0]}),
        ] {
            let mut invalid = a.events[index].clone();
            invalid.insert("ignorable".into(), true.into());
            invalid.extend(obj(metadata));
            err(
                RELEASED_V3_SESSION_FORMAT_CODEC.encode_event(&invalid),
                "unexpected field",
            );
            let mut bad = a.clone();
            bad.events[index] = invalid.clone();
            err(restore(bad), "unexpected field");
            let mut reader = catalog()
                .create_restore(
                    &physical(a.header.clone()),
                    SessionFormatRestoreOptions {
                        recovery: SessionFormatRecovery::Strict,
                        validation: SessionFormatValidation::Transformed,
                    },
                )
                .unwrap();
            for row in &a.events[..index] {
                reader
                    .decode_row(Value::Object(
                        RELEASED_V3_SESSION_FORMAT_CODEC.encode_event(row).unwrap(),
                    ))
                    .unwrap();
            }
            err(
                reader.decode_row(Value::Object(invalid)),
                "unexpected field",
            );
        }
    }
}
#[test]
fn refuses_target_generation_delivery_before_and_after_the_inherited_cut_under_all_recovery_and_validation_modes()
 {
    for validation in [
        SessionFormatValidation::Current,
        SessionFormatValidation::Transformed,
    ] {
        for recovery in [
            SessionFormatRecovery::Strict,
            SessionFormatRecovery::Recoverable,
        ] {
            for inherited in [true, false] {
                let mut source = history();
                let seq = if inherited { 9 } else { source.len() };
                let mut marker = event(
                    "session-log-deepseek/delivery-accepted",
                    seq as u64,
                    json!({"sessionId":if inherited{"parent"}else{"canonical:code:session"},"throughSeq":8,"sessionFormatVersion":3}),
                );
                marker.insert("ignorable".into(), true.into());
                if inherited {
                    source[seq] = marker;
                } else {
                    source.push(marker);
                }
                let before = json!(source);
                err(
                    read(physical(seeded()), source.clone(), recovery, validation),
                    "format v2 delivery marker claims target format v3",
                );
                assert_eq!(json!(source), before);
            }
        }
    }
}
#[test]
fn refuses_a_local_foreign_v2_watermark_even_when_an_earlier_inherited_watermark_has_the_same_owner()
 {
    let source = history();
    let mut rows = source.clone();
    rows.push(event(
        "session-log-deepseek/delivery-accepted",
        rows.len() as u64,
        json!({"sessionId":"parent","throughSeq":8,"sessionFormatVersion":2}),
    ));
    err(
        restore_rows(rows, seeded()),
        "current-generation delivery marker names the wrong Session",
    );
    assert_eq!(
        restore_rows(source.clone(), seeded())
            .unwrap()
            .events
            .iter()
            .find(|e| e["type"] == "session-log-deepseek/delivery-accepted")
            .unwrap()["data"],
        source[9]["data"]
    );
}
