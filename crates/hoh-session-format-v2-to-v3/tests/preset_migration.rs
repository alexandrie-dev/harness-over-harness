mod common;
use common::*;
use hoh_session_format::*;
use hoh_session_format_v2_to_v3::*;
use serde_json::{Value, json};
#[test]
fn migrates_only_the_exact_legacy_header_preset() {
    for preset in [
        None,
        Some("code"),
        Some("ptc"),
        Some("standard"),
        Some("minimal"),
        Some("custom-agent"),
        Some("code-custom"),
        Some("Code"),
    ] {
        let mut header = h(2, "code");
        if let Some(preset) = preset {
            header.insert("agentPreset".into(), preset.into());
        }
        let before = header.clone();
        let mut expected = header.clone();
        expected.insert("version".into(), 3.into());
        if preset == Some("code") {
            expected.insert("agentPreset".into(), "ptc".into());
        }
        assert_eq!(
            SESSION_FORMAT_V2_TO_V3
                .migrate_header(header.clone())
                .unwrap(),
            expected
        );
        assert_eq!(header, before);
    }
}
#[test]
fn preserves_admitted_selection_metadata() {
    for preset in [
        "code",
        "ptc",
        "standard",
        "minimal",
        "custom-agent",
        "code-custom",
        "Code",
    ] {
        let mut event = e("agent-preset/selected", 0, json!({"agentPreset":preset}));
        event.insert("time".into(), (-7).into());
        event.insert("ignorable".into(), true.into());
        let mut expected = event.clone();
        expected["data"]["agentPreset"] = json!(if preset == "code" { "ptc" } else { preset });
        assert_eq!(migrate(vec![event.clone()]).unwrap().events, vec![expected]);
        assert_eq!(event["data"]["agentPreset"], preset);
    }
}
#[test]
fn refuses_a_malformed_durable_selection() {
    for data in [Value::Null, json!([]), json!({}), json!({"agentPreset":1})] {
        fails(migrate(vec![ev("agent-preset/selected", data)]));
    }
}
#[test]
fn refuses_unaudited_selection_fields() {
    let mut extra = ev("agent-preset/selected", json!({"agentPreset":"code"}));
    extra.insert("extension".into(), "code".into());
    for event in [
        extra,
        ev(
            "agent-preset/selected",
            json!({"agentPreset":"code","extension":{"agentPreset":"code"}}),
        ),
    ] {
        err(migrate(vec![event]), "unexpected field extension");
    }
}
#[test]
fn refuses_sparse_source_sequences() {
    err(
        raw(
            vec![e(
                "agent-preset/selected",
                19,
                json!({"agentPreset":"code"}),
            )],
            h(2, "code"),
            Some(0),
        ),
        "source events must be dense",
    );
}
#[test]
fn refuses_unclassified_preset_looking_events_even_when_ignorable() {
    let mut event = ev(
        "external/selected",
        json!({"agentPreset":"code","code":"code"}),
    );
    event.insert("ignorable".into(), true.into());
    assert!(migrate(vec![event]).unwrap_err().is_unsupported_migration());
}
