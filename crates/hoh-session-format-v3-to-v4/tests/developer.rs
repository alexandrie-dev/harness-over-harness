mod common;
use common::*;
fn bad(row: &SessionFormatEvent, message: &str) {
    err(RELEASED_V4_SESSION_FORMAT_CODEC.encode_event(row), message);
    err(assert_v4_row_admission(&row.clone().into(), None), message);
}
fn restored(events: Vec<SessionFormatEvent>) -> R<SessionFormatArtifact> {
    let k = known(&events);
    restore_released_v4_artifact(artifact(events, h(4, "developer"), 0), &k)
}
#[test]
fn round_trips_native_messages_and_deferred_schemas_without_rewriting_native_payloads() {
    let input = developer_events();
    assert_eq!(round_trip(&input, &h(4, "developer"), 0).unwrap().0, input);
    let a = artifact(input.clone(), h(4, "developer"), 0);
    assert_eq!(
        restore_released_v4_artifact(a.clone(), &known(&input)).unwrap(),
        a
    );
}
#[test]
fn preserves_additional_json_fields_in_developer_data_messages_sources_and_blocks() {
    let mut input = developer_events();
    input[3]["data"]["metadata"] = json!({"future":true});
    input[3]["data"]["message"]["metadata"] = json!(["retained"]);
    input[3]["data"]["message"]["source"]["future"] = 1.into();
    for block in input[3]["data"]["message"]["content"]
        .as_array_mut()
        .unwrap()
    {
        block["future"] = true.into();
    }
    input[2]["data"]["header"]["tools"][0]["future"] = json!({"retained":true});
    assert_eq!(round_trip(&input, &h(4, "developer"), 0).unwrap().0, input);
    assert_eq!(restored(input.clone()).unwrap().events, input);
}
#[test]
fn rejects_a_malformed_addition_name() {
    for value in [None, Some(Value::Null), Some(json!("")), Some(json!(1))] {
        let mut row = developer_events()[3].clone();
        let mut block = json!({"type":"tool-addition"});
        if let Some(value) = value {
            block["toolName"] = value;
        }
        row["data"]["message"]["content"] = json!([block]);
        bad(&row, "nonempty toolName");
    }
}
#[test]
fn refuses_an_inline_definition_even_when_a_header_and_name_are_present() {
    let mut row = developer_events()[3].clone();
    row["data"]["message"]["content"] = json!([{"type":"tool-addition","toolName":"search","tool":{"name":"search","description":"Duplicate","parameters":{}}}]);
    bad(&row, "omit inline tool definitions");
}
#[test]
fn rejects_an_absent_or_invalid_addition_header_sequence() {
    for value in [
        None,
        Some(Value::Null),
        Some(json!(-1)),
        Some(json!(-0.0)),
        Some(json!(1.5)),
        Some(json!("2")),
    ] {
        let mut row = developer_events()[3].clone();
        row["data"]
            .as_object_mut()
            .unwrap()
            .shift_remove("headerSeq");
        if let Some(value) = value {
            row["data"]["headerSeq"] = value;
        }
        bad(&row, "headerSeq");
    }
}
#[test]
fn requires_removal_only_messages_to_omit_the_header_sequence() {
    let mut row = developer_events()[3].clone();
    row["data"]["message"]["content"] = json!([{"type":"tool-removal","toolName":"search"}]);
    err(
        RELEASED_V4_SESSION_FORMAT_CODEC.encode_event(&row),
        "omit headerSeq",
    );
}
#[test]
fn refuses_a_non_header_or_non_earlier_schema_reference() {
    for header_seq in [0, 3, 50] {
        let mut input = developer_events();
        input[3]["data"]["headerSeq"] = header_seq.into();
        let error = restored(input).unwrap_err().to_string();
        assert!(
            error.contains("headerSeq") && error.contains("earlier"),
            "{error}"
        );
    }
}
#[test]
fn refuses_missing_or_ambiguous_names_in_the_referenced_header() {
    for tools in [
        None,
        Some(json!([null])),
        Some(json!([{"name":"different","description":"","parameters":{}}])),
        Some(
            json!([{"name":"search","description":"First","parameters":{}},{"name":"search","description":"Second","parameters":{}}]),
        ),
    ] {
        let mut input = developer_events();
        input[2]["data"]["header"]
            .as_object_mut()
            .unwrap()
            .shift_remove("tools");
        if let Some(tools) = tools {
            input[2]["data"]["header"]["tools"] = tools;
        }
        err(restored(input), "exactly one tool");
    }
}
#[test]
fn refuses_an_incomplete_referenced_definition() {
    for tool in [
        json!({"name":"search"}),
        json!({"name":"search","description":null,"parameters":{}}),
        json!({"name":"search","description":"","parameters":[]}),
        json!({"name":"search","description":"","parameters":null}),
    ] {
        let mut input = developer_events();
        input[2]["data"]["header"]["tools"] = json!([tool]);
        err(restored(input), "complete tool definition");
    }
}
#[test]
fn does_not_interpret_an_unknown_ignorable_header_referenced_by_a_known_developer_event() {
    for data in [Value::Null, developer_events()[2]["data"].clone()] {
        let mut input = developer_events();
        input[2].insert("ignorable".into(), true.into());
        input[2].insert("data".into(), data);
        let mut k = known(&input);
        k.remove("request/header");
        err(
            restore_released_v4_artifact(artifact(input.clone(), h(4, "developer"), 0), &k),
            "known request/header",
        );
        input[3].insert("ignorable".into(), true.into());
        input[3].insert(
            "data".into(),
            json!({"opaque":true,"headerSeq":{"future":true}}),
        );
        input[3].insert("surfaceOp".into(), json!({"future":true}));
        k.remove("developer/message");
        let a = artifact(input, h(4, "developer"), 0);
        assert_eq!(restore_released_v4_artifact(a.clone(), &k).unwrap(), a);
    }
}
#[test]
fn keeps_historical_schema_references_when_compaction_shadows_their_developer_nodes() {
    let mut input = developer_events();
    let original = input[2].clone();
    input.insert(
        4,
        ev(
            "compaction/prune",
            json!({"shadowedRange":{"start":3,"end":3},"shadowedSeqs":[3],"shadowedTokenCount":0}),
        ),
    );
    let input = numbered(input, 1);
    assert!(restored(input.clone()).is_ok());
    assert_eq!(input[2], original);
}
#[test]
fn refuses_non_object_developer_data_before_recovery() {
    for data in [Value::Null, json!([]), json!("invalid")] {
        let mut row = developer_events()[3].clone();
        row.insert("data".into(), data);
        bad(&row, "developer/message data must be an object");
    }
}
#[test]
fn rejects_malformed_developer_event_fields() {
    for data in [
        json!({"turn":1,"step":1}),
        json!({"turn":1,"step":1,"message":null}),
    ] {
        let mut row = developer_events()[3].clone();
        row.insert("data".into(), data);
        err(
            RELEASED_V4_SESSION_FORMAT_CODEC.encode_event(&row),
            "requires turn, step, and a developer message",
        );
    }
}
#[test]
fn leaves_malformed_legacy_stream_chunks_to_canonical_decoding() {
    assert_v4_row_admission(&json!({"type":"assistant/attempt","seq":2,"time":3,"data":{"stream":[{"type":"chunk","chunk":null}]}}),None).unwrap();
}
#[test]
fn leaves_malformed_ordinary_arrays_to_canonical_decoding() {
    for (tag, key) in [
        ("agent/inbox/spliced", "inserted"),
        ("session/title-llm-request", "messages"),
    ] {
        assert_v4_row_admission(
            &json!({"type":tag,"seq":2,"time":3,"data":{key:null}}),
            None,
        )
        .unwrap();
    }
}
#[test]
fn preserves_ordinary_tools_alongside_deferred_tools() {
    for mixed in [false, true] {
        let mut tools = vec![json!({"name":"ordinary","description":"Ordinary","parameters":{}})];
        if mixed {
            tools.push(json!({"name":"deferred","description":"Ordinary","parameters":{},"deferLoading":true}));
        }
        let mut input = developer_events();
        input[2]["data"]["header"]["tools"] = tools.into();
        assert_eq!(round_trip(&input, &h(4, "developer"), 0).unwrap().0, input);
    }
}
#[test]
fn does_not_interpret_tool_entries_without_deferred_markers() {
    let mut row = developer_events()[2].clone();
    row["data"]["header"]["tools"] = json!([null]);
    assert_v4_row_admission(&row.into(), None).unwrap();
}
#[test]
fn preserves_the_historical_header_while_replacing_a_developer_surface_node() {
    let mut input = developer_events();
    let mut replacement = input[3].clone();
    replacement.insert(
        "surfaceOp".into(),
        json!({"op":"replace","startSeq":3,"endSeq":3}),
    );
    replacement.insert("sourceEventSeqs".into(), json!([3]));
    replacement["data"]["message"]["id"] = "developer-2".into();
    input.insert(4, replacement);
    let input = numbered(input, 1);
    assert_eq!(round_trip(&input, &h(4, "developer"), 0).unwrap().0, input);
    assert_eq!(restored(input.clone()).unwrap().events, input);
}
#[test]
fn rejects_invalid_deferred_marker() {
    for value in [json!(false), Value::Null, json!("true"), json!(1)] {
        let mut row = developer_events()[2].clone();
        row["data"]["header"]["tools"][0]["deferLoading"] = value;
        bad(&row, "deferLoading");
    }
}
#[test]
fn rejects_malformed_developer_message() {
    let m = developer_message();
    let mut cases = vec![];
    for (key, value) in [
        ("role", json!("user")),
        ("id", json!("")),
        ("source", json!({"kind":"plugin"})),
        ("content", json!([{"type":"tool-addition","toolName":""}])),
        ("content", json!([{"type":"tool-removal","toolName":""}])),
    ] {
        let mut invalid = m.clone();
        invalid[key] = value;
        cases.push(invalid);
    }
    for invalid in cases {
        let row = surface(
            "developer/message",
            json!({"turn":1,"step":1,"message":invalid}),
        );
        assert!(RELEASED_V4_SESSION_FORMAT_CODEC.encode_event(&row).is_err());
        assert!(assert_v4_row_admission(&row.into(), None).is_err());
    }
}
#[test]
fn preserves_developer_messages_inside_an_inherited_prefix() {
    let mut input = developer_events();
    input.insert(4, ev("session/end-seed", json!({"inherited":true})));
    let input = numbered(input, 1);
    let mut header = h(4, "developer");
    header.insert("isSeeded".into(), true.into());
    header.insert("parentSession".into(), "parent".into());
    let (events, cut) = round_trip(&input, &header, 4).unwrap();
    assert_eq!(events, input);
    assert_eq!(cut, 4);
    let a = artifact(events, header, cut);
    assert_eq!(
        restore_released_v4_artifact(a.clone(), &known(&input)).unwrap(),
        a
    );
}
#[test]
fn refuses_developer_history_when_the_reader_does_not_know_the_event_type() {
    let input = developer_events();
    let mut k = known(&input);
    k.remove("developer/message");
    err(
        restore_released_v4_artifact(artifact(input, h(4, "developer"), 0), &k),
        "unknown event type",
    );
}
#[test]
fn checks_request_coordinates_and_the_open_step() {
    let mut input = developer_events();
    input[3]["data"]["step"] = 2.into();
    err(restored(input), "open turn and step");
    let mut row = developer_events()[3].clone();
    row["data"]["turn"] = 0.into();
    err(
        RELEASED_V4_SESSION_FORMAT_CODEC.encode_event(&row),
        "positive",
    );
}
#[test]
fn rejects_tool_change_blocks_in_ordinary_messages() {
    for role in ["user", "system", "assistant", "tool"] {
        for tag in ["tool-addition", "tool-removal"] {
            let source = match role {
                "system" => json!({"kind":"system-prompt"}),
                "assistant" => json!({"kind":"model","provider":"test","model":"test"}),
                "tool" => json!({"kind":"tool","callId":"call"}),
                _ => json!({"kind":"user"}),
            };
            let mut m = developer_message();
            m["role"] = role.into();
            m["source"] = source;
            m["content"] = json!([{"type":tag,"toolName":"search"}]);
            if role == "tool" {
                m["toolCallId"] = "call".into();
                m["isError"] = false.into();
            }
            let event_type = if role == "tool" {
                "tool/result".into()
            } else {
                format!("{role}/message")
            };
            let data = if role == "user" {
                m
            } else {
                json!({"turn":1,"step":1,"message":m})
            };
            let row = e(&event_type, 3, data);
            bad(&row, "developer role");
            let mut d = RELEASED_V4_SESSION_FORMAT_CODEC
                .create_decoder(
                    &RELEASED_V4_SESSION_FORMAT_CODEC
                        .encode_header(&h(4, "developer"), 0)
                        .unwrap()
                        .into(),
                    SessionFormatRecovery::Strict,
                )
                .unwrap();
            err(
                d.decode_row(row.clone().into(), &mut SessionFormatEventCollector::new()),
                "developer role",
            );
            let mut input = developer_events();
            input[3] = row;
            err(restored(input), "developer role");
        }
    }
}
#[test]
fn rejects_developer_role_inside_ordinary_messages_before_payload_recovery() {
    for tag in [
        "user/message",
        "system/message",
        "assistant/message",
        "tool/result",
    ] {
        let row = e(
            tag,
            3,
            if tag == "user/message" {
                developer_message()
            } else {
                json!({"turn":1,"step":1,"message":developer_message()})
            },
        );
        bad(&row, "must occur together");
        let mut d = RELEASED_V4_SESSION_FORMAT_CODEC
            .create_decoder(
                &RELEASED_V4_SESSION_FORMAT_CODEC
                    .encode_header(&h(4, "developer"), 0)
                    .unwrap()
                    .into(),
                SessionFormatRecovery::Strict,
            )
            .unwrap();
        err(
            d.decode_row(row.into(), &mut SessionFormatEventCollector::new()),
            "must occur together",
        );
    }
}
#[test]
fn rejects_developer_role_in_message_arrays() {
    for (tag, key) in [
        ("agent/inbox/spliced", "inserted"),
        ("session/title-llm-request", "messages"),
    ] {
        let row = e(tag, 3, json!({key:[null,developer_message()]}));
        bad(&row, "must occur together");
        let mut d = RELEASED_V4_SESSION_FORMAT_CODEC
            .create_decoder(
                &RELEASED_V4_SESSION_FORMAT_CODEC
                    .encode_header(&h(4, "developer"), 0)
                    .unwrap()
                    .into(),
                SessionFormatRecovery::Strict,
            )
            .unwrap();
        err(
            d.decode_row(row.clone().into(), &mut SessionFormatEventCollector::new()),
            "must occur together",
        );
        let mut input = developer_events();
        input[3] = row;
        err(restored(input), "must occur together");
    }
}
#[test]
fn retains_unknown_ignorable_developer_records_without_interpreting_their_payload_or_surface() {
    let mut input = developer_events();
    input[3].insert("ignorable".into(), true.into());
    input[3].insert("data".into(), json!({"future":true}));
    input[3].insert("surfaceOp".into(), json!({"future":true}));
    let mut k = known(&input);
    k.remove("developer/message");
    let a = artifact(input, h(4, "developer"), 0);
    assert_eq!(restore_released_v4_artifact(a.clone(), &k).unwrap(), a);
}
#[test]
fn still_validates_known_ignorable_developer_records() {
    let mut input = developer_events();
    input[3].insert("ignorable".into(), true.into());
    input[3]["data"]["step"] = 2.into();
    err(restored(input), "open turn and step");
}
#[test]
fn rejects_tool_changes_outside_developer_content_including_streamed_blocks() {
    for tag in ["tool-addition", "tool-removal"] {
        let block = json!({"type":tag,"toolName":"search"});
        let mut ordinary = developer_message();
        ordinary["role"] = "user".into();
        ordinary["content"] = json!([block]);
        for (tag, data) in [
            ("compaction/summary", json!({"summary":[block]})),
            ("compaction/summary", json!({"rawOutput":[block]})),
            (
                "assistant/attempt",
                json!({"stream":[{"type":"chunk","chunk":{"type":"block-end","block":block}}]}),
            ),
            (
                "assistant/attempt",
                json!({"stream":[{"type":"chunk","chunk":{"type":"block-start","blockType":tag}}]}),
            ),
            ("agent/inbox/spliced", json!({"inserted":[ordinary]})),
            ("session/title-llm-request", json!({"messages":[ordinary]})),
        ] {
            err(
                assert_v4_row_admission(&e(tag, 2, data).into(), None),
                "developer role",
            );
        }
    }
}
