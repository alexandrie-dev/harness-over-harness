mod common;
use common::*;
use hoh_session_format_v3_to_v4::tool_role::*;
fn native_row(overrides: Value) -> SessionFormatEvent {
    let mut message = native_message("call-1", "result", Some(false.into()));
    for (k, v) in obj(overrides) {
        message[k] = v;
    }
    result(message)
}
fn lift(message: Value) -> R<SessionFormatEvent> {
    lift_tool_result(result(message))
}
#[test]
fn serializes_lifted_result_fields_in_current_writer_order() {
    for flag in [false, true] {
        let mut m = wrapper_message();
        m["content"][0]["isError"] = flag.into();
        let target = lift(m).unwrap();
        assert_eq!(
            hoh_util_values::js_json_stringify(&target["data"]["message"]),
            format!(
                "{{\"role\":\"tool\",\"source\":{{\"kind\":\"tool\",\"callId\":\"call-1\"}},\"toolCallId\":\"call-1\",\"content\":[{{\"type\":\"text\",\"text\":\"result\"}}],\"isError\":{flag},\"id\":\"tool-result-call-1\"}}"
            )
        );
    }
}
#[test]
fn preserves_outer_fields_without_activating_their_target_names() {
    for flag in [None, Some(false), Some(true)] {
        let mut m = wrapper_message();
        for (k, v) in obj(
            json!({"__proto__":{"saved":"prototype"},"constructor":{"saved":"constructor"},"extension":{"saved":true}}),
        ) {
            m[k] = v;
        }
        m["toolCallId"] = "call-1".into();
        m["content"][0]
            .as_object_mut()
            .unwrap()
            .shift_remove("isError");
        if let Some(flag) = flag {
            m["isError"] = flag.into();
            m["content"][0]["isError"] = flag.into();
        }
        let before = m.clone();
        let lifted = lift(m.clone()).unwrap();
        let target = &lifted["data"]["message"];
        assert_eq!(target["plugin:message:toolCallId"], "call-1");
        for key in ["__proto__", "constructor", "extension"] {
            assert_eq!(target[format!("plugin:message:{key}")], m[key]);
        }
        if let Some(flag) = flag {
            assert_eq!(target["isError"], flag);
            assert_eq!(target["plugin:message:isError"], flag);
        } else {
            assert!(target.get("isError").is_none());
        }
        assert_eq!(m, before);
        assert_eq!(
            RELEASED_V4_SESSION_FORMAT_CODEC
                .encode_event(&lifted)
                .unwrap(),
            lifted
        );
    }
}
#[test]
fn keeps_outer_data_separate_from_the_interpreted_result() {
    for (key, value, flag) in [
        ("toolCallId", json!("other-call"), Some(false)),
        ("isError", json!(true), Some(false)),
        ("isError", json!(true), None),
        ("isError", json!("false"), Some(false)),
    ] {
        let mut m = wrapper_message();
        m[key] = value.clone();
        m["content"][0]["content"] = json!([]);
        m["content"][0]
            .as_object_mut()
            .unwrap()
            .shift_remove("isError");
        if let Some(flag) = flag {
            m["content"][0]["isError"] = flag.into();
        }
        let before = m.clone();
        let actual = lift(m.clone()).unwrap();
        let target = &actual["data"]["message"];
        assert_eq!(target[format!("plugin:message:{key}")], value);
        assert_eq!(target["toolCallId"], "call-1");
        assert_eq!(
            target.get("isError"),
            flag.as_ref().map(|f| if *f {
                &Value::Bool(true)
            } else {
                &Value::Bool(false)
            })
        );
        assert_eq!(m, before);
    }
}
#[test]
fn preserves_an_outer_flag_without_adding_an_absent_result_flag() {
    let mut m = wrapper_message();
    m["isError"] = false.into();
    m["content"][0]
        .as_object_mut()
        .unwrap()
        .shift_remove("isError");
    let t = lift(m).unwrap();
    assert_eq!(t["data"]["message"]["plugin:message:isError"], false);
    assert!(t["data"]["message"].get("isError").is_none());
}
#[test]
fn preserves_result_owned_fields_without_adding_a_core_field() {
    for field in ["extension", "__proto__", "constructor"] {
        let mut m = wrapper_message();
        m["content"][0][field] = json!({"saved":true});
        let before = m.clone();
        let t = lift(m.clone()).unwrap();
        assert_eq!(
            t["data"]["message"][format!("plugin:result:{field}")],
            json!({"saved":true})
        );
        assert!(t["data"]["message"].get(field).is_none());
        assert_eq!(m, before);
    }
}
#[test]
fn keeps_repeated_owner_names_and_already_prefixed_field_names_distinct() {
    let mut m = wrapper_message();
    m["note"] = "outer".into();
    m["plugin:result:note"] = "outer prefixed".into();
    m["content"][0]["note"] = "inner".into();
    m["content"][0]["plugin:message:note"] = "inner prefixed".into();
    let t = lift(m).unwrap();
    for (key, value) in [
        ("plugin:message:note", "outer"),
        ("plugin:message:plugin:result:note", "outer prefixed"),
        ("plugin:result:note", "inner"),
        ("plugin:result:plugin:message:note", "inner prefixed"),
    ] {
        assert_eq!(t["data"]["message"][key], value);
    }
}
#[test]
fn lifts_a_released_wrapper_into_the_native_tool_role_message() {
    assert_eq!(
        lift(wrapper_message()).unwrap()["data"],
        json!({"turn":1,"step":1,"message":native_message("call-1","result",Some(false.into()))})
    );
}
#[test]
fn omits_is_error_when_a_released_wrapper_did_not_persist_the_flag() {
    let mut m = wrapper_message();
    m["content"][0]
        .as_object_mut()
        .unwrap()
        .shift_remove("isError");
    let t = lift(m).unwrap();
    assert_eq!(t["data"]["message"]["role"], "tool");
    assert_eq!(
        t["data"]["message"]["content"],
        json!([{"type":"text","text":"result"}])
    );
    assert!(t["data"]["message"].get("isError").is_none());
}
#[test]
fn rejects_a_released_wrapper_with_a_non_boolean_is_error_flag() {
    let mut m = wrapper_message();
    m["content"][0]["isError"] = "yes".into();
    err(lift(m), "tool-result isError must be boolean");
}
#[test]
fn passes_non_tool_rows_through_the_wrapper_conversion() {
    let row = user("u");
    assert_eq!(lift_tool_result(row.clone()).unwrap(), row);
}
#[test]
fn preserves_is_error_and_repair_identity_across_the_lift() {
    let mut row = result(wrapper_message());
    row["data"]["message"]["id"] = "interrupted-tool-result-call-1-0".into();
    row["data"]["message"]["content"][0]["isError"] = true.into();
    row["data"]["message"]["content"][0]["content"] = json!([{"type":"text","text":"The tool call was interrupted before the Harness recorded it as started. Retry it if it is still needed."}]);
    row["data"]["error"] = json!({"name":"ToolNotStartedError","code":"TOOL_NOT_STARTED"});
    let t = lift_tool_result(row.clone()).unwrap();
    assert_eq!(t["data"]["message"]["id"], row["data"]["message"]["id"]);
    assert_eq!(t["data"]["message"]["toolCallId"], "call-1");
    assert_eq!(t["data"]["message"]["isError"], true);
    assert_eq!(t["data"]["error"], row["data"]["error"]);
}
#[test]
fn refuses_nested_released_results_without_discarding_their_identity_or_error_status() {
    let mut m = wrapper_message();
    m["content"][0]["content"] = json!([{"type":"tool-result","toolCallId":"nested-call","isError":true,"content":[{"type":"text","text":"hidden"}]}]);
    let before = m.clone();
    let error = lift(m.clone()).unwrap_err();
    assert!(error.is_unsupported_migration());
    assert!(error.to_string().contains("unsupported by this converter"));
    assert_eq!(m, before);
}
#[test]
fn rejects_a_released_wrapper_with_extra_top_level_content_blocks() {
    let mut m = wrapper_message();
    m["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"text","text":"extra"}));
    err(lift(m), "exactly one tool-result wrapper");
}
#[test]
fn encodes_and_decodes_a_first_class_tool_result_unchanged() {
    let row = native_row(json!({}));
    assert_eq!(
        RELEASED_V4_SESSION_FORMAT_CODEC.encode_event(&row).unwrap(),
        row
    );
    assert_eq!(
        round_trip(std::slice::from_ref(&row), &h(4, "tool-role"), 0)
            .unwrap()
            .0,
        vec![row]
    );
}
#[test]
fn rejects_a_released_wrapper_row_in_the_current_v4_decoder() {
    let mut d=RELEASED_V4_SESSION_FORMAT_CODEC.create_decoder(&json!({"type":"session","version":4,"id":"tool-role","createdAt":1,"isSeeded":false,"delegationDepth":0}),SessionFormatRecovery::Strict).unwrap();
    err(
        d.decode_row(
            result(wrapper_message()).into(),
            &mut SessionFormatEventCollector::new(),
        ),
        "requires a tool-role message",
    );
}
#[test]
fn keeps_native_semantic_failures_hard_even_in_recoverable_mode() {
    let mut d=RELEASED_V4_SESSION_FORMAT_CODEC.create_decoder(&json!({"type":"session","version":4,"id":"tool-role","createdAt":1,"isSeeded":false,"delegationDepth":0}),SessionFormatRecovery::Recoverable).unwrap();
    err(
        d.decode_row(
            native_row(json!({"isError":"yes"})).into(),
            &mut SessionFormatEventCollector::new(),
        ),
        "isError must be boolean",
    );
}
#[test]
fn refuses_a_native_row_whose_error_metadata_lacks_the_message_is_error_flag() {
    let mut row = native_row(json!({}));
    row["data"]["error"] = json!({"name":"ToolNotStartedError","code":"TOOL_NOT_STARTED"});
    err(
        RELEASED_V4_SESSION_FORMAT_CODEC.encode_event(&row),
        "error metadata for a non-error tool result",
    );
}
#[test]
fn restores_a_first_class_artifact_through_the_released_v3_relationship_rules() {
    let mut rows = opening();
    rows.push(assistant(
        json!([{"type":"tool-call","id":"call-1","name":"bash","arguments":"{}"}]),
    ));
    rows.push(ev(
        "tool/call",
        json!({"turn":1,"step":1,"callId":"call-1","name":"bash","arguments":"{}"}),
    ));
    rows.push(native_row(json!({})));
    rows.extend(closing());
    let output = native(rows).unwrap();
    assert_eq!(output.events[4]["data"]["message"]["role"], "tool");
    assert_eq!(output.events[4]["data"]["message"]["toolCallId"], "call-1");
}
#[test]
fn lifts_a_released_wrapper_tool_result_into_the_native_message() {
    let mut rows = opening();
    rows.push(result(wrapper_message()));
    let out = migrate(dense(rows), h(3, "tool-role"), Some(0), vec![]).unwrap();
    assert_eq!(
        out.events[2]["data"]["message"],
        native_message("call-1", "result", Some(false.into()))
    );
}
#[test]
fn keeps_non_tool_rows_intact_and_expands_runs_through_the_same_transform() {
    let row = ev("turn/start", json!({"turn":1}));
    let mut s = stage(&h(3, "tool-role"), Some(0), vec![]).unwrap();
    let mut output = SessionFormatEventCollector::new();
    s.transform_run(Box::new(Run(vec![row.clone()])), &mut output)
        .unwrap();
    assert_eq!(output.values, vec![row]);
}
#[test]
fn accepts_a_native_message_and_ignores_rows_of_other_event_types() {
    assert_v4_tool_result_message(&native_row(json!({}))).unwrap();
    assert_v4_tool_result_message(&ev("user/message", json!({"role":"user","content":[]})))
        .unwrap();
}
#[test]
fn refuses_data_and_messages_that_are_not_objects() {
    for (data, message) in [
        (json!("nope"), "data must be an object"),
        (json!({"message":"nope"}), "message must be an object"),
    ] {
        err(
            assert_v4_tool_result_message(&ev("tool/result", data)),
            message,
        );
    }
}
macro_rules! bad_native {($name:ident,$message:literal,$($value:expr),+)=>{#[test]fn $name(){for overrides in [$($value),+] {err(assert_v4_tool_result_message(&native_row(overrides)),$message);}}};}
bad_native!(
    refuses_ids_that_are_not_non_empty_strings,
    "string id",
    json!({"id":""}),
    json!({"id":7})
);
bad_native!(
    refuses_a_message_that_is_not_the_tool_role,
    "tool-role message",
    json!({"role":"user"})
);
bad_native!(
    refuses_a_tool_call_id_that_does_not_match_the_tool_source,
    "matching its tool source",
    json!({"toolCallId":"other"}),
    json!({"toolCallId":""}),
    json!({"source":{"kind":"tool","callId":"other"}}),
    json!({"source":"nope"})
);
bad_native!(
    refuses_a_source_that_is_not_the_tool_source,
    "requires a tool source",
    json!({"source":{"kind":"model","callId":"call-1"}})
);
bad_native!(
    refuses_content_that_is_not_an_array,
    "array content",
    json!({"content":"nope"})
);
bad_native!(
    refuses_an_is_error_flag_that_is_not_boolean,
    "isError must be boolean",
    json!({"isError":"yes"})
);
#[test]
fn round_trips_a_native_message_without_an_is_error_flag() {
    let row = result(native_message("call-1", "result", None));
    assert_eq!(lift_tool_result(row.clone()).unwrap(), row);
}
#[test]
fn refuses_a_released_wrapper_without_its_wrapper_block() {
    let mut m = wrapper_message();
    m["content"] = json!([]);
    err(lift(m), "exactly one tool-result wrapper");
}
#[test]
fn refuses_wrapper_rows_with_invalid_source_or_content_fields() {
    for (key, value) in [("source", json!("nope")), ("content", json!("nope"))] {
        let mut m = wrapper_message();
        m[key] = value;
        err(lift(m), "exactly one tool-result wrapper");
    }
    let mut m = wrapper_message();
    m["content"][0]["content"] = "nope".into();
    err(lift(m), "tool-result content must be an array");
}
bad_native!(
    refuses_a_native_message_that_still_contains_a_released_wrapper,
    "must not contain a released tool-result wrapper",
    json!({"content":[{"type":"tool-result","toolCallId":"call-1","content":[]}]})
);
