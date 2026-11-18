mod common;
use common::*;
use hoh_session_format_v3_to_v4::message_sources::*;
fn event(source: Value) -> SessionFormatEvent {
    surface(
        "user/message",
        json!({"id":"u","role":"user","source":source,"content":[{"type":"text","text":"hello"}]}),
    )
}
fn restore(source: Value, version: u64) -> R<SessionFormatArtifact> {
    let row = event(source);
    let a = if version == 3 {
        migrate(vec![row], h(3, "sources"), Some(0), vec![])?
    } else {
        artifact(vec![row], h(4, "sources"), 0)
    };
    restore_released_v4_artifact(
        a,
        &["user/message"].into_iter().map(str::to_owned).collect(),
    )
}
#[test]
fn migrates_external_producers_without_interpreting_inherited_object_keys() {
    for plugin in ["constructor", "toString", "__proto__"] {
        let before = json!({"kind":"plugin","plugin":plugin,"__proto__":{"tag":"metadata"},"nested":{"kind":"plugin","plugin":"opaque"}});
        let expected = json!({"kind":format!("plugin:{plugin}"),"__proto__":{"tag":"metadata"},"nested":{"kind":"plugin","plugin":"opaque"}});
        let migrated = restore(before.clone(), 3).unwrap();
        assert_eq!(migrated.events[0]["data"]["source"], expected);
        assert!(
            migrated.events[0]["data"]["source"]
                .get("__proto__")
                .is_some()
        );
        assert_eq!(restore(expected, 4).unwrap(), migrated);
        assert_eq!(before["kind"], "plugin");
    }
}
#[test]
fn retains_direct_source_kind_through_migration_and_native_restore() {
    for kind in ["acme", "plugin:acme", "source:acme", "__proto__"] {
        let source = json!({"kind":kind,"extra":{"kind":"plugin","plugin":"opaque"}});
        let migrated = restore(source.clone(), 3).unwrap();
        assert_eq!(migrated.events[0]["data"]["source"], source);
        assert_eq!(restore(source, 4).unwrap(), migrated);
    }
}
#[test]
fn rejects_missing_malformed_empty_and_retired_source_kinds() {
    for source in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"kind":""}),
        json!({"kind":1}),
        json!({"kind":"plugin","plugin":"compact"}),
    ] {
        err(restore(source.clone(), 4), "producer-owned source kind");
        err(
            assert_v4_message_sources(&event(source)),
            "producer-owned source kind",
        );
    }
}
#[test]
fn rejects_retired_wrappers_in_every_declared_message_slot_before_recovery_can_discard_them() {
    let m = event(json!({"kind":"plugin","plugin":"compact"}))["data"].clone();
    let mut rows = vec![event(json!({"kind":"plugin","plugin":"compact"}))];
    for tag in ["system/message", "assistant/message", "tool/result"] {
        rows.push(ev(tag, json!({"message":m})));
    }
    rows.extend([
        ev("agent/inbox/spliced", json!({"inserted":[m]})),
        ev("session/title-llm-request", json!({"messages":[m]})),
    ]);
    for row in rows {
        let mut d = RELEASED_V4_SESSION_FORMAT_CODEC
            .create_decoder(
                &RELEASED_V4_SESSION_FORMAT_CODEC
                    .encode_header(&h(4, "sources"), 0)
                    .unwrap()
                    .into(),
                SessionFormatRecovery::Recoverable,
            )
            .unwrap();
        let mut out = SessionFormatEventCollector::new();
        d.decode_row(Value::Null, &mut out).unwrap();
        err(
            d.decode_row(row.clone().into(), &mut out),
            "producer-owned source kind",
        );
        err(
            RELEASED_V4_SESSION_FORMAT_CODEC.encode_event(&row),
            "producer-owned source kind",
        );
    }
}
#[test]
fn leaves_malformed_incomplete_rows_for_normal_scanner_corruption_handling() {
    for row in [
        Value::Null,
        json!({"type":"user/message","data":null}),
        json!({"type":"system/message","data":{}}),
        json!({"type":"agent/inbox/spliced","data":{"inserted":false}}),
        json!({"type":"agent/inbox/spliced","data":{"inserted":[null]}}),
        json!({"type":"session/title-llm-request","data":{"messages":[event(json!({"kind":"user"}))["data"]]}}),
        json!({"type":"external/event","data":{"source":{"kind":"plugin"}}}),
    ] {
        assert_v4_source_row_admission(&row).unwrap();
    }
}
