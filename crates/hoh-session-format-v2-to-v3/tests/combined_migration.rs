mod common;
use common::*;
use hoh_session_format::*;
use hoh_session_format_v2_to_v3::*;
use serde_json::{Value, json};
fn read_source(rows: Vec<SessionFormatEvent>) -> R<SessionFormatArtifact> {
    read(
        physical(h(2, "combined")),
        rows,
        SessionFormatRecovery::Strict,
        SessionFormatValidation::Current,
    )
}
#[test]
fn inserts_and_clears_system_prompts_while_remapping_replacement_and_nested_ptc_history_without_rewriting_identities()
 {
    let source = composite_source();
    let before = json!(source);
    let target = read_source(source.clone()).unwrap();
    assert_eq!(target.header["version"], 3);
    assert_eq!(target.events.len(), source.len() + 3);
    assert_composite(&target.events);
    assert_eq!(restore(target.clone()).unwrap(), target);
    assert_eq!(json!(source), before);
    assert_eq!(round_trip(target.clone()).unwrap(), target);
    let mut bad = target;
    let inner = bad
        .events
        .iter_mut()
        .find(|e| e["type"] == "tool/ptc-dispatch" && e["data"]["subCallId"] == "tools-ptc:child")
        .unwrap();
    inner["data"]["parentCallId"] = "missing".into();
    inner["data"]["content"] = json!([]);
    err(restore(bad), "parentCallId");
}
#[test]
fn composes_header_and_scalar_preset_renames_with_every_structural_and_canonical_transformation() {
    let mut header = h(2, "combined");
    header.insert("agentPreset".into(), "code".into());
    let mut rows = composite_source();
    let selection = e(
        "agent-preset/selected",
        rows.len() as u64,
        json!({"agentPreset":"code"}),
    );
    rows.push(selection.clone());
    let a = read(
        physical(header),
        rows,
        SessionFormatRecovery::Strict,
        SessionFormatValidation::Current,
    )
    .unwrap();
    assert_eq!(a.header["version"], 3);
    assert_eq!(a.header["agentPreset"], "ptc");
    let mut expected = selection;
    expected.insert("seq".into(), (composite_source().len() + 3).into());
    expected["data"]["agentPreset"] = "ptc".into();
    assert_eq!(*a.events.last().unwrap(), expected);
    assert_composite(&a.events);
    restore(a).unwrap();
}
#[test]
fn refuses_unaudited_v2_preset_extensions_but_preserves_native_nested_preset_data() {
    let mut source = composite_source();
    let selection = e(
        "agent-preset/selected",
        source.len() as u64,
        json!({"agentPreset":"code","extension":{"agentPreset":"code"}}),
    );
    source.push(selection.clone());
    err(read_source(source), "unexpected field extension");
    let mut native = selection;
    native.insert("seq".into(), 0.into());
    let a = read(
        physical(h(3, "combined")),
        vec![native.clone()],
        SessionFormatRecovery::Strict,
        SessionFormatValidation::Current,
    )
    .unwrap();
    assert_eq!(a.events, vec![native]);
}
#[test]
fn rejects_old_replacement_fields_and_ptc_tags_in_native_encoding_decoding_and_restoration() {
    let source = composite_source();
    for (row, msg) in [
        (source[9].clone(), "exact replace fields"),
        (source[4].clone(), "unknown event type"),
    ] {
        err(RELEASED_V3_SESSION_FORMAT_CODEC.encode_event(&row), msg);
        err(restore(artifact(vec![row.clone()])), msg);
        let mut r = catalog()
            .create_restore(
                &physical(h(3, "combined")),
                SessionFormatRestoreOptions {
                    recovery: SessionFormatRecovery::Strict,
                    validation: SessionFormatValidation::Transformed,
                },
            )
            .unwrap();
        for seq in 0..row["seq"].as_u64().unwrap() {
            let mut row = e("external/event", seq, json!({}));
            row.insert("ignorable".into(), true.into());
            r.decode_row(Value::Object(row)).unwrap();
        }
        err(r.decode_row(Value::Object(row)), msg);
    }
}
#[test]
fn refuses_required_obsolete_tags_during_recoverable_decoding() {
    for kind in ["tool/code-dispatch-start", "tool/code-dispatch"] {
        let mut d = decoder(false, SessionFormatRecovery::Recoverable);
        let mut out = SessionFormatEventCollector::new();
        let first = e("turn/start", 0, json!({"turn":1}));
        d.decode_row(Value::Object(first.clone()), &mut out)
            .unwrap();
        let error = d
            .decode_row(Value::Object(e(kind, 1, dispatch())), &mut out)
            .unwrap_err();
        assert!(error.is_unsupported_migration());
        assert_eq!(out.values, vec![first]);
    }
}
#[test]
fn refuses_unsupported_admission_after_a_canonical_semantic_tail_error() {
    let mut d = decoder(false, SessionFormatRecovery::Recoverable);
    let mut out = SessionFormatEventCollector::new();
    d.decode_row(
        Value::Object(e("turn/start", 0, json!({"turn":1}))),
        &mut out,
    )
    .unwrap();
    d.decode_row(
        Value::Object(e("user/message", 1, user_data("user"))),
        &mut out,
    )
    .unwrap();
    assert!(
        d.decode_row(
            Value::Object(e("tool/code-dispatch-start", 2, dispatch())),
            &mut out
        )
        .unwrap_err()
        .is_unsupported_migration()
    );
}
#[test]
fn withholds_a_malformed_canonical_tail_without_advancing_the_accepted_inherited_cut() {
    let mut d = decoder(true, SessionFormatRecovery::Recoverable);
    let mut out = SessionFormatEventCollector::new();
    let mut first = e("external/event", 0, json!({}));
    first.insert("ignorable".into(), true.into());
    let marker = e("session/end-seed", 1, json!({"inherited":true}));
    d.decode_row(Value::Object(first.clone()), &mut out)
        .unwrap();
    d.decode_row(Value::Object(marker.clone()), &mut out)
        .unwrap();
    let mut bad = composite_source()[9].clone();
    bad.insert("seq".into(), 2.into());
    bad.insert("sourceEventSeqs".into(), json!([0]));
    bad.insert(
        "surfaceOp".into(),
        json!({"op":"replace","start":0,"end":0}),
    );
    d.decode_row(Value::Object(bad), &mut out).unwrap();
    d.decode_row(
        Value::Object(e("session/end-seed", 3, json!({"inherited":true}))),
        &mut out,
    )
    .unwrap();
    assert_eq!(d.finish(&mut out).unwrap(), 1);
    assert_eq!(out.values, vec![first, marker]);
    err(
        d.decode_row(
            Value::Object(e(
                "turn/end",
                4,
                json!({"turn":1,"reason":{"kind":"completed"}}),
            )),
            &mut out,
        ),
        "exact replace fields",
    );
}
#[test]
fn preserves_historical_delivery_generations_while_remapping_the_envelope_and_inherited_cut() {
    for version in [None, Some(0), Some(1), Some(2), Some(4)] {
        let source = composite_source();
        let mut data = json!({"sessionId":"parent","throughSeq":source.len()-1});
        if let Some(version) = version {
            data["sessionFormatVersion"] = json!(version);
        }
        let marker = e(
            "session-log-deepseek/delivery-accepted",
            source.len() as u64,
            data,
        );
        let cut = e(
            "session/end-seed",
            source.len() as u64 + 1,
            json!({"inherited":true}),
        );
        let mut rows = source.clone();
        rows.extend([marker.clone(), cut.clone()]);
        let mut header = seeded_header();
        header.insert("id".into(), "combined".into());
        let a = read(
            physical(header),
            rows,
            SessionFormatRecovery::Strict,
            SessionFormatValidation::Current,
        )
        .unwrap();
        let mut expected = marker;
        expected.insert("seq".into(), (source.len() + 3).into());
        assert_eq!(a.events[a.events.len() - 2], expected);
        assert_eq!(a.inherited_event_count, source.len() as u64 + 4);
        let mut expected = cut;
        expected.insert("seq".into(), a.inherited_event_count.into());
        assert_eq!(*a.events.last().unwrap(), expected);
        assert_composite(&a.events);
        assert_eq!(round_trip(a.clone()).unwrap(), a);
    }
}
#[test]
fn refuses_target_v3_delivery_activation_after_replacement_and_ptc_migration() {
    let mut source = composite_source();
    source.push(e(
        "session-log-deepseek/delivery-accepted",
        source.len() as u64,
        json!({"sessionId":"combined","throughSeq":source.len()-1,"sessionFormatVersion":3}),
    ));
    err(
        read_source(source),
        "format v2 delivery marker claims target format v3",
    );
}
