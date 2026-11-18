mod common;
use common::*;
use hoh_session_format_v3_to_v4::sources::*;
#[test]
fn returns_events_whose_data_is_not_an_object() {
    let row = ev("turn/start", json!("nope"));
    assert_eq!(map_event_messages(row.clone(), Ok).unwrap(), row);
}
#[test]
fn transforms_the_user_message_payload_and_preserves_identity_when_unchanged() {
    let row = ev(
        "user/message",
        json!({"id":"u","role":"user","source":{"kind":"user"},"content":[]}),
    );
    assert_eq!(map_event_messages(row.clone(), Ok).unwrap(), row);
    let mapped = map_event_messages(row.clone(), |mut m| {
        m.insert("id".into(), "u2".into());
        Ok(m)
    })
    .unwrap();
    assert_eq!(mapped["data"]["id"], "u2");
    assert_eq!(row["data"]["id"], "u");
}
#[test]
fn transforms_the_message_payload_of_system_assistant_and_tool_result_rows() {
    let row = ev(
        "system/message",
        json!({"turn":1,"step":1,"message":{"id":"s","role":"system","source":{"kind":"system-prompt"},"content":[]}}),
    );
    assert_eq!(map_event_messages(row.clone(), Ok).unwrap(), row);
    assert_eq!(
        map_event_messages(row, |mut m| {
            m.insert("id".into(), "s2".into());
            Ok(m)
        })
        .unwrap()["data"]["message"]["id"],
        "s2"
    );
}
#[test]
fn refuses_a_message_payload_that_is_not_an_object() {
    err(
        map_event_messages(ev("system/message", json!({"message":"nope"})), Ok),
        "requires a message",
    );
}
#[test]
fn transforms_message_arrays_on_spliced_and_title_request_rows() {
    for (tag, key) in [
        ("agent/inbox/spliced", "inserted"),
        ("session/title-llm-request", "messages"),
    ] {
        let row = ev(tag, json!({key:[user_data("u")]}));
        assert_eq!(map_event_messages(row.clone(), Ok).unwrap(), row);
        let mapped = map_event_messages(row, |mut m| {
            m.insert("id".into(), "u2".into());
            Ok(m)
        })
        .unwrap();
        assert_eq!(mapped["data"][key][0]["id"], "u2");
    }
}
#[test]
fn refuses_a_message_array_that_is_not_an_array_and_members_that_are_not_objects() {
    for value in [json!("nope"), json!(["nope"])] {
        err(
            map_event_messages(ev("agent/inbox/spliced", json!({"inserted":value})), Ok),
            "requires message",
        );
    }
}
#[test]
fn returns_unrelated_events_unchanged() {
    let row = ev("turn/start", json!({"turn":1}));
    assert_eq!(map_event_messages(row.clone(), Ok).unwrap(), row);
}
#[test]
fn rewrites_renamed_and_role_sensitive_producers_dropping_the_plugin_field() {
    for (plugin, role, expected) in [
        ("tools-ptc", "user", "ptc-mode"),
        ("tools-code-mode", "user", "ptc-mode"),
        ("@deepseek-ai/dsh-system-prompt", "system", "system-prompt"),
        ("@deepseek-ai/dsh-system-prompt", "user", "runtime-context"),
    ] {
        assert_eq!(
            rewrite_plugin_source(
                obj(json!({"kind":"plugin","plugin":plugin})),
                1,
                Some(&json!(role))
            )
            .unwrap(),
            obj(json!({"kind":expected}))
        );
    }
    assert_eq!(
        rewrite_plugin_source(
            obj(json!({"kind":"plugin","plugin":"external","extra":true})),
            1,
            Some(&json!("user"))
        )
        .unwrap(),
        obj(json!({"kind":"plugin:external","extra":true}))
    );
}
#[test]
fn refuses_a_plugin_field_that_is_not_a_string() {
    for plugin in [Value::Null, json!(7)] {
        err(
            rewrite_plugin_source(
                obj(json!({"kind":"plugin","plugin":plugin})),
                3,
                Some(&json!("user")),
            ),
            "not canonical",
        );
    }
}
#[test]
fn namespaces_external_plugin_names_while_retaining_released_first_party_producers() {
    for plugin in [
        "plugin",
        "user",
        "model",
        "tool",
        "system-prompt",
        "runtime-context",
        "compact-checkpoint",
        "ptc-mode",
        "compact-basic",
        "auto-review",
    ] {
        assert_eq!(
            rewrite_plugin_source(
                obj(json!({"kind":"plugin","plugin":plugin})),
                3,
                Some(&json!("user"))
            )
            .unwrap(),
            obj(json!({"kind":format!("plugin:{plugin}")}))
        );
    }
    let source = obj(
        json!({"kind":"plugin","plugin":"agent-instructions","form":"instructions","changes":[]}),
    );
    assert_eq!(
        rewrite_plugin_source(source, 3, Some(&json!("user"))).unwrap(),
        obj(json!({"kind":"agent-instructions","form":"instructions","changes":[]}))
    );
    for (plugin, role, kind) in [
        ("tool-jobs", "user", "tool-jobs"),
        ("@deepseek-ai/dsh-system-prompt", "system", "system-prompt"),
    ] {
        assert_eq!(
            rewrite_plugin_source(
                obj(json!({"kind":"plugin","plugin":plugin})),
                3,
                Some(&json!(role))
            )
            .unwrap(),
            obj(json!({"kind":kind}))
        );
    }
}
#[test]
fn prefixes_the_complete_plugin_name() {
    for name in [
        "",
        "acme",
        "plugin:x",
        "source:x",
        "message",
        "x/y",
        "__proto__",
    ] {
        assert_eq!(
            rewrite_plugin_source(
                obj(json!({"kind":"plugin","plugin":name})),
                1,
                Some(&json!("user"))
            )
            .unwrap(),
            obj(json!({"kind":format!("plugin:{name}")}))
        );
    }
}
#[test]
fn preserves_direct_source_kind_and_its_metadata() {
    for kind in [
        "acme",
        "plugin:acme",
        "source:acme",
        "message",
        "x/y",
        "__proto__",
    ] {
        let source = obj(json!({"kind":kind,"payload":{"kind":"plugin","plugin":kind}}));
        assert_eq!(
            rewrite_v3_message_source(source.clone(), 1, Some(&json!("user"))).unwrap(),
            source
        );
    }
}
#[test]
fn preserves_native_v3_sources_and_converts_plugin_wrappers() {
    let source = obj(json!({"kind":"tool","callId":"call","extra":true}));
    assert_eq!(
        rewrite_v3_message_source(source.clone(), 1, Some(&json!("user"))).unwrap(),
        source
    );
    assert_eq!(
        rewrite_v3_message_source(
            obj(json!({"kind":"plugin","plugin":"compact"})),
            1,
            Some(&json!("user"))
        )
        .unwrap(),
        obj(json!({"kind":"compact-checkpoint"}))
    );
}
#[test]
fn rejects_type_invalid_source_kind() {
    for kind in [Value::Null, json!(0), json!("")] {
        err(
            rewrite_v3_message_source(obj(json!({"kind":kind})), 1, Some(&json!("user"))),
            "requires a nonempty kind",
        );
    }
}
