mod common;
use common::*;
use hoh_session_format_v3_to_v4::{
    content::*, extension_identities::namespace_v3_opaque_event,
    message_sources::assert_v4_message_sources,
};
fn image() -> Value {
    json!({"type":"image","attachment":{"attachmentId":"image","mediaType":"image/png","bytes":1,"width":1,"height":1}})
}
fn wrapper() -> Value {
    json!({"type":"future","content":[{"type":"tool-result","toolCallId":"opaque","content":[image()]}]})
}
fn converted() -> Value {
    let mut b = wrapper();
    b["type"] = "plugin:future".into();
    b
}
fn message() -> Value {
    json!({"id":"user","role":"user","source":{"kind":"user"},"content":[wrapper()]})
}
fn migrate_one(row: SessionFormatEvent) -> R<SessionFormatEvent> {
    Ok(migrate(vec![row], h(3, "content"), Some(0), vec![])?
        .events
        .remove(0))
}
#[test]
fn preserves_own_extension_keys_without_changing_opaque_values_or_source_content() {
    let old=hoh_util_values::js_json_parse("{\"type\":\"vendor\",\"metadata\":{\"tag\":\"old\"},\"__proto__\":{\"saved\":true},\"constructor\":{\"saved\":false}}").unwrap();
    let mut expected = old.clone();
    expected["type"] = "plugin:vendor".into();
    assert_eq!(
        migrate_v3_content(Some(&json!([old.clone(), image()])), "sample").unwrap(),
        vec![expected, image()]
    );
    assert_eq!(old["type"], "vendor");
}
#[test]
fn keeps_the_unknown_v3_tag_opaque() {
    for tag in [
        "message",
        "extension",
        "tool-addition",
        "tool-removal",
        "",
        "future",
        "plugin:future",
    ] {
        let b = json!({"type":tag,"content":[wrapper()],"message":{"role":"tool","content":[wrapper()]},"data":wrapper()});
        let mut expected = b.clone();
        expected["type"] = format!("plugin:{tag}").into();
        assert_eq!(
            migrate_v3_content(Some(&json!([b])), "sample").unwrap(),
            vec![expected]
        );
    }
}
#[test]
fn shares_unchanged_leaves_and_ignores_json_that_is_not_a_declared_content_slot() {
    let content = json!([{"type":"text","text":"saved","metadata":wrapper()},{"type":"tool-call","id":"call","name":"echo","arguments":hoh_util_values::js_json_stringify(&wrapper())},image()]);
    assert_eq!(
        migrate_v3_content(Some(&content), "sample").unwrap(),
        content.as_array().unwrap().clone()
    );
    let mut m = message();
    m["content"] = content;
    m["replayState"] = wrapper();
    m["schema"] = json!({"examples":[wrapper()]});
    for row in [
        ev("user/message", m),
        ev("future", json!({"content":[wrapper()]})),
        ev("future", Value::Null),
    ] {
        assert_eq!(migrate_v3_event_content(row.clone()).unwrap(), row);
    }
}
#[test]
fn converts_declared_content_without_changing_envelope_coordinates() {
    let m = message();
    let mut cm = m.clone();
    cm["content"] = json!([converted()]);
    let cases = vec![
        ("user/message", m.clone(), cm.clone()),
        (
            "system/message",
            json!({"message":m}),
            json!({"message":cm}),
        ),
        (
            "assistant/message",
            json!({"message":m}),
            json!({"message":cm}),
        ),
        (
            "agent/inbox/spliced",
            json!({"inserted":[m]}),
            json!({"inserted":[cm]}),
        ),
        (
            "session/title-llm-request",
            json!({"messages":[m]}),
            json!({"messages":[cm]}),
        ),
        (
            "compaction/summary",
            json!({"summary":[wrapper()],"rawOutput":[wrapper()]}),
            json!({"summary":[converted()],"rawOutput":[converted()]}),
        ),
        (
            "compaction/summary",
            json!({"summary":[]}),
            json!({"summary":[]}),
        ),
        (
            "tool/ptc-dispatch",
            json!({"content":[wrapper()]}),
            json!({"content":[converted()]}),
        ),
        (
            "team/message/queued",
            json!({"message":m}),
            json!({"message":cm}),
        ),
        (
            "team/message/queued",
            json!({"message":{"content":[]}}),
            json!({"message":{"content":[]}}),
        ),
        (
            "team/message/queued",
            json!({"message":null}),
            json!({"message":null}),
        ),
    ];
    for (tag, data, expected) in cases {
        let mut row = e(tag, 3, data);
        row.insert("sourceEventSeqs".into(), json!([0, 2]));
        row.insert(
            "surfaceOp".into(),
            json!({"op":"replace","startSeq":0,"endSeq":2}),
        );
        row.insert("extra".into(), json!({"content":[wrapper()]}));
        let before = row.clone();
        let mut target = row.clone();
        target.insert("data".into(), expected);
        assert_eq!(migrate_v3_event_content(row.clone()).unwrap(), target);
        assert_eq!(row, before);
    }
}
#[test]
fn converts_raw_start_end_blocks_and_preserves_start_owned_fields() {
    for tag in ["assistant/message", "assistant/attempt"] {
        let stream = json!([{"type":"chunk","chunk":{"type":"block-start","index":0,"blockType":"future","extensionName":"old","metadata":{"old":true},"custom":wrapper()},"time":7},{"type":"chunk","chunk":{"type":"block-end","index":0,"block":wrapper()},"time":7},{"type":"chunk","chunk":{"type":"block-start","index":1,"blockType":"message"},"time":7},{"type":"chunk","chunk":{"type":"block-end","index":1,"block":{"type":"message","content":[wrapper()]}},"time":7},{"type":"chunk","chunk":{"type":"block-start","index":2,"blockType":"text"},"time":7},{"type":"chunk","chunk":{"type":"block-start","index":3,"blockType":"text","metadata":{"original":true}},"time":7},{"type":"chunk","chunk":{"type":"block-end","index":2,"block":{"type":"text","text":"after"}},"time":7},{"type":"chunk","chunk":{"type":"text-delta","index":2,"text":"after"},"time":7},{"type":"chunk","chunk":null,"time":7},null,{"type":"annotation","data":wrapper()}]);
        let mut expected = stream.clone();
        expected[0]["chunk"]["blockType"] = "plugin:future".into();
        expected[1]["chunk"]["block"] = converted();
        expected[2]["chunk"]["blockType"] = "plugin:message".into();
        expected[3]["chunk"]["block"]["type"] = "plugin:message".into();
        let mut m = message();
        m["content"] = json!([]);
        let row = ev(tag, json!({"message":m,"stream":stream}));
        assert_eq!(
            migrate_v3_event_content(row.clone()).unwrap()["data"],
            json!({"message":m,"stream":expected})
        );
        let unchanged = ev(
            tag,
            json!({"message":m,"stream":stream.as_array().unwrap()[6..]}),
        );
        assert_eq!(
            migrate_v3_event_content(unchanged.clone()).unwrap(),
            unchanged
        );
    }
}
#[test]
fn preserves_v3_request_tool_metadata_keys_and_values() {
    let tool = json!({"name":"echo","description":"Echo","parameters":{"type":"object","properties":{},"examples":[wrapper()],"deferLoading":true},"vendor":{"saved":true},"plugin:vendor":{"saved":false},"plugin:deferLoading":false,"__proto__":{"saved":"prototype"},"constructor":{"saved":"constructor"}});
    let row = ev(
        "request/header",
        json!({"reason":"initial","header":{"config":{"provider":"mock","model":"mock"},"tools":[tool]}}),
    );
    assert_eq!(migrate_one(row.clone()).unwrap(), row);
    RELEASED_V4_SESSION_FORMAT_CODEC.encode_event(&row).unwrap();
    assert!(tool.get("__proto__").is_some());
    assert_eq!(tool["parameters"]["deferLoading"], true);
    assert_eq!(tool["plugin:deferLoading"], false);
}
#[test]
fn refuses_an_unsupported_v3_top_level_defer_loading_value_without_altering_the_source() {
    for value in [json!(true), json!(false), Value::Null, json!("custom")] {
        let row = ev(
            "request/header",
            json!({"reason":"initial","header":{"config":{"provider":"mock","model":"mock"},"tools":[{"name":"first","description":"First","parameters":{}},{"name":"echo","description":"Echo","parameters":{},"deferLoading":value}]}}),
        );
        let before = row.clone();
        let error = migrate_one(row.clone()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("request/header at seq 0.header.tools[1] contains deferLoading")
        );
        assert_eq!(row, before);
    }
}
#[test]
fn preserves_stream_start_extra_keys_while_converting_its_unknown_block_type() {
    let start = json!({"type":"block-start","index":7,"blockType":"plugin:text","deferLoading":true,"plugin:deferLoading":false,"metadata":{"saved":1},"plugin:metadata":{"saved":2},"__proto__":{"saved":3}});
    let mut expected = start.clone();
    expected["blockType"] = "plugin:plugin:text".into();
    let row = ev(
        "assistant/attempt",
        json!({"stream":[{"type":"chunk","chunk":start}]}),
    );
    assert_eq!(
        migrate_v3_event_content(row).unwrap()["data"],
        json!({"stream":[{"type":"chunk","chunk":expected}]})
    );
    assert_eq!(start["deferLoading"], true);
    assert!(start.get("__proto__").is_some());
}
#[test]
fn retains_native_v4_deferred_loading_declaration_validation() {
    let mut row = request();
    row["data"]["header"]["tools"] =
        json!([{"name":"echo","description":"Echo","parameters":{},"deferLoading":true}]);
    assert_eq!(
        round_trip(std::slice::from_ref(&row), &h(4, "native"), 0)
            .unwrap()
            .0,
        vec![row.clone()]
    );
    row["data"]["header"]["tools"][0]["deferLoading"] = false.into();
    err(
        RELEASED_V4_SESSION_FORMAT_CODEC.encode_event(&row),
        "deferLoading must be true",
    );
}
#[test]
fn shares_v3_request_tools_with_no_extension_fields_and_leaves_malformed_tools_decoder_owned() {
    for header in [
        Value::Null,
        json!({}),
        json!({"tools":null}),
        json!({"tools":[{"name":"echo","description":"Echo","parameters":{}},null]}),
    ] {
        let row = ev("request/header", json!({"header":header}));
        assert_eq!(migrate_v3_event_content(row.clone()).unwrap(), row);
    }
}
#[test]
fn leaves_missing_message_sources_for_native_admission_to_reject() {
    let row = ev(
        "user/message",
        json!({"id":"missing-source","role":"user","content":[]}),
    );
    let actual = migrate_one(row.clone()).unwrap();
    assert_eq!(actual, row);
    err(
        assert_v4_message_sources(&actual),
        "producer-owned source kind",
    );
}
#[test]
fn rejects_non_array_content_at_the_durable_conversion_boundary() {
    err(
        migrate_v3_content(Some(&json!({})), "sample"),
        "content must be an array",
    );
}
#[test]
fn rejects_type_invalid_content() {
    for block in [Value::Null, json!({}), json!({"type":4})] {
        err(
            migrate_v3_content(Some(&json!([block])), "sample"),
            "string type tags",
        );
    }
}
#[test]
fn rejects_a_stream_start_without_a_string_block_type() {
    err(
        migrate_v3_event_content(ev(
            "assistant/attempt",
            json!({"stream":[{"type":"chunk","chunk":{"type":"block-start","index":0}}]}),
        )),
        "blockType must be a string",
    );
}
#[test]
fn round_trips_extension_content() {
    for role in ["system", "user"] {
        let m = json!({"id":"user","role":role,"source":{"kind":if role=="system" {"system-prompt"} else {"user"}},"content":[converted(),{"type":"plugin:future","content":[wrapper()]}]});
        let row = ev(
            &format!("{role}/message"),
            if role == "user" {
                m
            } else {
                json!({"turn":1,"step":1,"message":m})
            },
        );
        assert_eq!(
            round_trip(std::slice::from_ref(&row), &h(4, "native"), 0)
                .unwrap()
                .0,
            vec![row]
        );
    }
}
#[test]
fn namespaces_while_retaining_opaque_data_and_envelope_fields() {
    for tag in ["developer/message", "plugin:developer/message", "future"] {
        let mut row = ev(tag, json!({"message":message(),"content":[wrapper()]}));
        row.insert("ignorable".into(), true.into());
        row.insert("sourceEventSeqs".into(), json!([0]));
        row.insert("surfaceOp".into(), "append".into());
        row.insert("extra".into(), json!({"saved":true}));
        let mut expected = row.clone();
        expected.insert("type".into(), format!("plugin:{tag}").into());
        assert_eq!(migrate_one(row).unwrap(), expected);
        assert_ne!(
            namespace_v3_opaque_event(expected.clone())["type"],
            expected["type"]
        );
    }
}
#[test]
fn leaves_incomplete_delivery_payloads_for_generation_validation() {
    for data in [Value::Null, json!({}), json!({"sessionFormatVersion":"4"})] {
        let row = ev("session-log-deepseek/delivery-accepted", data);
        assert_eq!(namespace_v3_opaque_event(row.clone()), row);
    }
}
#[test]
fn leaves_required_unknown_event_identities_for_the_migration_stage_to_reject() {
    for row in [ev("future", json!({})), ev("turn/start", json!({"turn":1}))] {
        assert_eq!(namespace_v3_opaque_event(row.clone()), row);
    }
}
