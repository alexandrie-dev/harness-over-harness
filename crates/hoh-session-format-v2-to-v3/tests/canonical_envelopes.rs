mod common;
use common::*;
use hoh_session_format::*;
use hoh_session_format_v2_to_v3::{payload::canonicalize_transformed_event, *};
use serde_json::{Value, json};
use std::collections::HashSet;
fn u(seq: u64, metadata: Value) -> SessionFormatEvent {
    let mut e = e(
        "user/message",
        seq,
        json!({"id":format!("user-{seq}"),"role":"user","content":[{"type":"text","text":"hello"}],"source":{"kind":"user"}}),
    );
    e.extend(obj(metadata));
    e
}
fn open() -> Vec<SessionFormatEvent> {
    vec![
        e("turn/start", 0, json!({"turn":1})),
        e("step/start", 1, json!({"turn":1,"step":1})),
    ]
}
fn raw_rows(rows: Vec<SessionFormatEvent>) -> R<SessionFormatArtifact> {
    raw(rows, h(2, "canonical-envelopes"), Some(0))
}
fn admit(rows: Vec<SessionFormatEvent>, migration: bool) -> R<SessionFormatArtifact> {
    if migration {
        raw_rows(rows)
    } else {
        restore(artifact(rows))
    }
}
fn with(
    mut a: Vec<SessionFormatEvent>,
    b: impl IntoIterator<Item = SessionFormatEvent>,
) -> Vec<SessionFormatEvent> {
    a.extend(b);
    a
}
#[test]
fn renames_replacement_keys_after_structural_insertion_and_remaps_source_event_references() {
    let first = u(2, json!({"surfaceOp":"append"}));
    let replacement = u(
        3,
        json!({"surfaceOp":{"op":"replace","start":2,"end":2},"sourceEventSeqs":[2]}),
    );
    let source = with(open(), [first.clone(), replacement.clone()]);
    let before = json!(source);
    let target = raw_rows(source.clone()).unwrap();
    let mut first = first;
    first.insert("seq".into(), 3.into());
    let mut replacement = replacement;
    replacement.insert("seq".into(), 4.into());
    replacement.insert(
        "surfaceOp".into(),
        json!({"op":"replace","startSeq":3,"endSeq":3}),
    );
    replacement.insert("sourceEventSeqs".into(), json!([3]));
    assert_eq!(&target.events[3..], &[first, replacement]);
    assert_eq!(target.events[0], source[0]);
    assert_eq!(json!(source), before);
    assert_eq!(restore(target.clone()).unwrap(), target);
}
#[test]
fn remaps_the_seed_cut_and_retains_other_session_captured_references() {
    let reference = json!({"sessionId":"source","label":"source","capturedFormatVersion":2,"capturedThroughSeq":9,"compacted":false,"originalMessages":1,"retainedMessages":1,"omittedMessages":0,"omittedBytes":0,"truncated":false,"inputIndex":0});
    let mut captured = u(2, json!({"surfaceOp":"append"}));
    captured["data"]["source"] =
        json!({"kind":"session-reference","form":"recall","version":1,"references":[reference]});
    let source = with(
        open(),
        [
            captured,
            e("session/end-seed", 3, json!({"inherited":true})),
            u(4, json!({"surfaceOp":"append"})),
        ],
    );
    let target = raw(source.clone(), seeded_header(), Some(3)).unwrap();
    assert_eq!(target.inherited_event_count, 4);
    for (old, new) in [(0, 0), (2, 3), (3, 4), (4, 5)] {
        let mut expected = source[old].clone();
        expected.insert("seq".into(), new.into());
        assert_eq!(target.events[new], expected);
    }
    assert_eq!(
        target.events[3]["data"]["source"]["references"],
        json!([reference])
    );
    restore(target).unwrap();
}
#[test]
fn omits_empty_request_header_optionals_after_extracting_system() {
    for (empty, diagnostic) in [
        (json!({"system":""}), "header.system"),
        (json!({"tools":[]}), "empty optional header fields"),
        (
            json!({"adapterDefaults":{}}),
            "empty optional header fields",
        ),
        (
            json!({"system":"","tools":[],"adapterDefaults":{}}),
            "header.system",
        ),
    ] {
        let mut request = e(
            "request/header",
            2,
            json!({"reason":"initial","header":{"config":{"provider":"mock","model":"mock"}}}),
        );
        request["data"]["header"]
            .as_object_mut()
            .unwrap()
            .extend(obj(empty));
        let source = with(open(), [request.clone()]);
        let before = json!(source);
        let target = raw_rows(source.clone()).unwrap();
        request.insert("seq".into(), 3.into());
        request["data"]["header"] = json!({"config":{"provider":"mock","model":"mock"}});
        assert_eq!(target.events[3], request);
        assert_eq!(json!(source), before);
        restore(target).unwrap();
        err(restore(artifact(source)), diagnostic);
    }
}
#[test]
fn retains_nonempty_tools_and_defaults_while_extracting_every_nonempty_system_prompt() {
    let tools = json!([{"name":"read","description":"read","parameters":{}}]);
    let defaults = json!({"maxTokens":true});
    for system in [None, Some(" ")] {
        let mut request = e(
            "request/header",
            2,
            json!({"reason":"initial","header":{"config":{"provider":"mock","model":"mock","maxTokens":20},"tools":tools,"adapterDefaults":defaults}}),
        );
        if let Some(system) = system {
            request["data"]["header"]["system"] = system.into();
        }
        let a = raw_rows(with(open(), [request])).unwrap();
        assert_eq!(
            a.events.last().unwrap()["data"]["header"],
            json!({"config":{"provider":"mock","model":"mock","maxTokens":20},"tools":tools,"adapterDefaults":defaults})
        );
        assert_eq!(systems(&a).len(), if system.is_some() { 2 } else { 1 });
        restore(a).unwrap();
    }
}
#[test]
fn refuses_malformed_v2_replacement_rather_than_repairing_it() {
    for (op, diagnostic) in [
        (Value::Null, "surfaceOp"),
        (json!([]), "surfaceOp"),
        (json!(true), "surfaceOp"),
        (json!({"op":"replace","start":0}), "surfaceOp"),
        (json!({"op":"append","start":0,"end":0}), "surfaceOp"),
        (
            json!({"op":"replace","start":0,"end":0,"extra":true}),
            "surfaceOp",
        ),
        (json!({"op":"replace","startSeq":0,"endSeq":0}), "surfaceOp"),
        (json!({"op":"replace","start":0,"endSeq":0}), "surfaceOp"),
        (
            json!({"op":"replace","start":0,"end":0,"startSeq":0,"endSeq":0}),
            "surfaceOp",
        ),
        (
            json!({"op":"replace","start":-1,"end":0}),
            "surface start must be a non-negative safe integer",
        ),
        (
            json!({"op":"replace","start":0,"end":0.5}),
            "surface end must be a non-negative safe integer",
        ),
        (json!({"op":"replace","start":2,"end":3}), "replacement"),
    ] {
        let result = raw_rows(with(
            open(),
            [
                u(2, json!({"surfaceOp":"append"})),
                u(3, json!({"surfaceOp":op,"sourceEventSeqs":[2]})),
            ],
        ));
        err(result, diagnostic);
    }
}
#[test]
fn refuses_malformed_released_markers_at_transformed_event_admission() {
    for op in [
        json!({"op":"replace","start":0,"end":0,"extra":true}),
        json!({"op":"append","start":0,"end":0}),
        json!({"op":"replace","startSeq":0,"end":0}),
        json!({"op":"replace","start":0,"endSeq":0}),
    ] {
        let source = u(1, json!({"surfaceOp":op,"sourceEventSeqs":[0]}));
        let before = source.clone();
        err(
            canonicalize_transformed_event(source.clone()),
            "format v2 user/message at seq 1 requires exact replace fields op/start/end",
        );
        assert_eq!(source, before);
    }
}
#[test]
fn requires_canonical_replacement_fields_on_native_v3() {
    for op in [
        json!({"op":"replace","start":0,"end":0}),
        json!({"op":"replace","start":0,"endSeq":0}),
        json!({"op":"replace","start":0,"end":0,"startSeq":0,"endSeq":0}),
        json!({}),
        json!({"op":"replace","startSeq":0}),
        json!({"op":"replace","startSeq":0,"endSeq":0,"extra":true}),
    ] {
        err(
            restore(artifact(vec![
                u(0, json!({"surfaceOp":"append"})),
                u(1, json!({"surfaceOp":op,"sourceEventSeqs":[0]})),
            ])),
            "exact replace fields",
        );
    }
}
macro_rules! admission_tests{($module:ident,$migration:expr)=>{mod $module{use super::*;
#[test]fn requires_a_surface_marker(){for kind in ["user/message","assistant/message","tool/result"]{err(admit(vec![e(kind,0,json!({}))],$migration),if $migration{"requires surfaceOp"}else{"requires a surfaceOp"});}}
#[test]fn forbids_surface_metadata_on_log_only_events(){for kind in ["feedback/record","assistant/attempt"]{for metadata in [json!({"surfaceOp":"append"}),json!({"sourceEventSeqs":[]})]{let mut row=e(kind,0,json!({}));row.extend(obj(metadata));err(admit(vec![row],$migration),"unexpected field");}}}
#[test]fn forbids_assistant_source_event_seqs(){for sources in [json!([]),json!([0]),Value::Null,json!(false)]{let mut rows=tool_log(false,None);rows[2].insert("sourceEventSeqs".into(),sources);err(admit(rows,$migration),if $migration{"chunk references"}else{"cannot carry sourceEventSeqs"});}}
#[test]fn refuses_invalid_source_event_references(){for refs in [json!([]),json!(0),Value::Null,json!([-1]),json!([0.5]),json!([9_007_199_254_740_992_u64]),json!([3]),json!([4]),json!([0,0])]{err(admit(with(open(),[u(2,json!({"surfaceOp":"append"})),u(3,json!({"surfaceOp":"append","sourceEventSeqs":refs}))]),$migration),"sourceEventSeqs");}}
#[test]fn requires_logical_envelope_members(){for key in ["type","seq","time","data"]{let mut row=u(0,json!({"surfaceOp":"append"}));row.shift_remove(key);let diagnostic=if $migration&&key=="type"{"unclassified event undefined"}else if $migration&&key=="seq"{"source events must be dense"}else{"lacks required field"};err(admit(vec![row],$migration),diagnostic);}}
#[test]fn rejects_invalid_envelope_values(){for (key,v,diagnostic) in [("type",json!(false),if $migration{"unclassified event false"}else{"type must be a string"}),("ignorable",json!(false),"ignorable must be true"),("ignorable",Value::Null,"ignorable must be true")]{let mut row=u(0,json!({"surfaceOp":"append"}));row.insert(key.into(),v);err(admit(vec![row],$migration),diagnostic);}}
#[test]fn rejects_a_successful_tool_result_carrying_error_metadata(){err(admit(tool_log(false,Some(json!({"name":"ReadError","code":"READ_FAILED"}))),$migration),"error metadata for a non-error tool result");}
#[test]fn preserves_failed_tool_results_without_inventing_or_deleting_error_metadata(){for error in [None,Some(json!({"name":"ReadError","code":"READ_FAILED"}))]{let rows=tool_log(true,error.clone());let a=admit(rows.clone(),$migration).unwrap();let result=a.events.iter().find(|e|e["type"]=="tool/result").unwrap();assert_eq!(result["data"],rows[4]["data"]);assert_eq!(result["seq"],if $migration{5}else{4});assert_eq!(result["sourceEventSeqs"],if $migration{json!([4])}else{json!([3])});assert_eq!(result["data"].get("error"),error.as_ref());restore(a).unwrap();}}
}};}
admission_tests!(migration_admission, true);
admission_tests!(native_admission, false);
#[test]
fn preserves_arbitrary_data_and_surface_metadata_on_ignorable_events() {
    for kind in [
        "external/future",
        "tool/code-dispatch-start",
        "tool/code-dispatch",
    ] {
        for data in [
            Value::Null,
            json!(false),
            json!(["opaque"]),
            json!({"nested":{"sourceEventSeqs":[900],"surfaceOp":null}}),
        ] {
            for metadata in [
                json!({"sourceEventSeqs":null,"surfaceOp":false}),
                json!({"sourceEventSeqs":[900,900],"surfaceOp":{"future":["opaque"]}}),
            ] {
                let mut row = e(kind, 0, data.clone());
                row.insert("ignorable".into(), true.into());
                row.extend(obj(metadata));
                let a = artifact(vec![row.clone()]);
                assert_eq!(restore(a.clone()).unwrap(), a);
                err(
                    raw_rows(vec![row]),
                    if kind == "external/future" {
                        "cannot safely transform unclassified event"
                    } else {
                        "unexpected field sourceEventSeqs"
                    },
                );
            }
        }
    }
}
#[test]
fn round_trips_ignorable_events_with_physical_source_event_ranges_and_arbitrary_data() {
    for kind in [
        "external/future",
        "tool/code-dispatch-start",
        "tool/code-dispatch",
    ] {
        for data in [
            Value::Null,
            json!(false),
            json!(["opaque"]),
            json!({"nested":{"sourceEventSeqs":null}}),
        ] {
            let mut row = e(kind, 1, data);
            row.extend(obj(
                json!({"ignorable":true,"sourceEventSeqs":[0],"surfaceOp":{"future":["opaque"]}}),
            ));
            let encoded = RELEASED_V3_SESSION_FORMAT_CODEC.encode_event(&row).unwrap();
            let mut d = decoder(false, SessionFormatRecovery::Strict);
            let mut out = SessionFormatEventCollector::new();
            let first = e("feedback/record", 0, json!({"text":"prior"}));
            d.decode_row(Value::Object(first.clone()), &mut out)
                .unwrap();
            d.decode_row(Value::Object(encoded), &mut out).unwrap();
            assert_eq!(d.finish(&mut out).unwrap(), 0);
            assert_eq!(out.values, vec![first, row]);
        }
    }
}
#[test]
fn requires_exact_replacement_envelope_fields_on_all_surface_events() {
    for kind in [
        "system/message",
        "user/message",
        "assistant/message",
        "tool/result",
    ] {
        let data = if kind == "system/message" {
            json!({"turn":1,"step":1,"message":{"id":"system","role":"system","content":[],"source":{"kind":"plugin","plugin":"context"}}})
        } else {
            json!({})
        };
        for op in [
            json!({"op":"replace","start":0,"end":0}),
            json!({"op":"replace","startSeq":0,"endSeq":0,"extra":true}),
        ] {
            let mut row = e(kind, 1, data.clone());
            row.insert("surfaceOp".into(), op);
            err(
                RELEASED_V3_SESSION_FORMAT_CODEC.encode_event(&row),
                "exact replace fields",
            );
        }
    }
}
#[test]
fn hard_refuses_native_header_system_after_corruption() {
    for system in [
        json!(""),
        json!("prompt"),
        Value::Null,
        json!(false),
        json!([]),
        json!({}),
    ] {
        let row = e(
            "request/header",
            2,
            json!({"reason":"initial","header":{"config":{"provider":"mock","model":"mock"},"system":system}}),
        );
        for recovery in [
            SessionFormatRecovery::Strict,
            SessionFormatRecovery::Recoverable,
        ] {
            let mut d = decoder(false, recovery);
            let mut out = SessionFormatEventCollector::new();
            if recovery == SessionFormatRecovery::Recoverable {
                d.decode_row(Value::Null, &mut out).unwrap();
            }
            err(
                d.decode_row(Value::Object(row.clone()), &mut out),
                "header.system",
            );
            err(
                RELEASED_V3_SESSION_FORMAT_CODEC.encode_event(&row),
                "header.system",
            );
        }
    }
}
fn invalid_rows() -> Vec<(SessionFormatEvent, &'static str)> {
    let mut bad = tool_log(false, Some(json!({"code":"FAILED"})))[4].clone();
    bad.insert("seq".into(), 1.into());
    bad.insert("sourceEventSeqs".into(), json!([0]));
    vec![
        (u(1, json!({})), "requires a surfaceOp"),
        (
            u(1, json!({"surfaceOp":{"op":"replace","start":0,"end":0}})),
            "exact replace fields",
        ),
        (
            u(
                1,
                json!({"surfaceOp":{"op":"replace","startSeq":1,"endSeq":0}}),
            ),
            "replacement endpoints must reference earlier events",
        ),
        (
            u(
                1,
                json!({"surfaceOp":{"op":"replace","startSeq":2,"endSeq":0}}),
            ),
            "replacement endpoints must reference earlier events",
        ),
        (
            u(
                1,
                json!({"surfaceOp":{"op":"replace","startSeq":0,"endSeq":1}}),
            ),
            "replacement endpoints must reference earlier events",
        ),
        (
            u(
                1,
                json!({"surfaceOp":{"op":"replace","startSeq":0,"endSeq":2}}),
            ),
            "replacement endpoints must reference earlier events",
        ),
        (
            e(
                "request/header",
                1,
                json!({"reason":"initial","header":{"config":{"provider":"mock","model":"mock"},"tools":[]}}),
            ),
            "empty optional header fields",
        ),
        (bad, "non-error tool result"),
    ]
}
#[test]
fn rejects_invalid_rows_through_strict_decoding_and_encoding() {
    for (row, msg) in invalid_rows() {
        let mut d = decoder(false, SessionFormatRecovery::Strict);
        let mut out = SessionFormatEventCollector::new();
        d.decode_row(Value::Object(open()[0].clone()), &mut out)
            .unwrap();
        err(d.decode_row(Value::Object(row.clone()), &mut out), msg);
        err(RELEASED_V3_SESSION_FORMAT_CODEC.encode_event(&row), msg);
    }
}
#[test]
fn rejects_invalid_rows_even_when_native_catalog_validation_is_transformed() {
    for (row, msg) in invalid_rows() {
        let mut r = catalog()
            .create_restore(
                &physical(h(3, "identity")),
                SessionFormatRestoreOptions {
                    recovery: SessionFormatRecovery::Strict,
                    validation: SessionFormatValidation::Transformed,
                },
            )
            .unwrap();
        r.decode_row(Value::Object(open()[0].clone())).unwrap();
        err(r.decode_row(Value::Object(row)), msg);
    }
}
#[test]
fn discards_an_incomplete_invalid_tail() {
    for (row, _) in invalid_rows() {
        let mut d = decoder(false, SessionFormatRecovery::Recoverable);
        let mut out = SessionFormatEventCollector::new();
        let first = open()[0].clone();
        for e in [first.clone(), row, u(2, json!({"surfaceOp":"append"}))] {
            d.decode_row(Value::Object(e), &mut out).unwrap();
        }
        assert_eq!(d.finish(&mut out).unwrap(), 0);
        assert_eq!(out.values, vec![first]);
    }
}
#[test]
fn refuses_a_later_commit_after_an_invalid_tail() {
    for (row, msg) in invalid_rows() {
        let mut d = decoder(false, SessionFormatRecovery::Recoverable);
        let mut out = SessionFormatEventCollector::new();
        d.decode_row(Value::Object(open()[0].clone()), &mut out)
            .unwrap();
        d.decode_row(Value::Object(row), &mut out).unwrap();
        err(
            d.decode_row(
                Value::Object(e(
                    "turn/end",
                    2,
                    json!({"turn":1,"reason":{"kind":"completed"}}),
                )),
                &mut out,
            ),
            msg,
        );
    }
}
fn tagged(seq: u64) -> SessionFormatEvent {
    e("session/end-seed", seq, json!({"inherited":true}))
}
fn opaque(seq: u64) -> SessionFormatEvent {
    let mut row = e("external/event", seq, json!({}));
    row.insert("ignorable".into(), true.into());
    row
}
#[test]
fn retains_the_accepted_seeded_cut_before_a_discarded_invalid_marker() {
    let mut d = decoder(true, SessionFormatRecovery::Recoverable);
    let mut out = SessionFormatEventCollector::new();
    let mut bad = tagged(2);
    bad.insert("surfaceOp".into(), "append".into());
    for row in [
        opaque(0),
        tagged(1),
        bad,
        u(3, json!({"surfaceOp":"append"})),
    ] {
        d.decode_row(Value::Object(row), &mut out).unwrap();
    }
    assert_eq!(d.finish(&mut out).unwrap(), 1);
    assert_eq!(out.values, vec![opaque(0), tagged(1)]);
}
#[test]
fn rejects_a_seeded_log_whose_only_inherited_marker_was_discarded() {
    let mut d = decoder(true, SessionFormatRecovery::Recoverable);
    let mut out = SessionFormatEventCollector::new();
    let mut bad = tagged(0);
    bad.insert("surfaceOp".into(), "append".into());
    d.decode_row(Value::Object(bad), &mut out).unwrap();
    err(
        d.finish(&mut out),
        "lacks an accepted inherited end-seed marker",
    );
    assert!(out.values.is_empty());
}
#[test]
fn ignores_a_discarded_inherited_marker_in_an_unseeded_semantic_tail() {
    let mut d = decoder(false, SessionFormatRecovery::Recoverable);
    let mut out = SessionFormatEventCollector::new();
    for row in [opaque(0), u(1, json!({})), tagged(2)] {
        d.decode_row(Value::Object(row), &mut out).unwrap();
    }
    assert_eq!(d.finish(&mut out).unwrap(), 0);
    assert_eq!(out.values, vec![opaque(0)]);
}
#[test]
fn rejects_an_accepted_inherited_marker_in_an_unseeded_log_with_a_discarded_tail() {
    for seq in [0, 1] {
        let mut d = decoder(false, SessionFormatRecovery::Recoverable);
        let mut out = SessionFormatEventCollector::new();
        if seq == 1 {
            d.decode_row(Value::Object(opaque(0)), &mut out).unwrap();
        }
        d.decode_row(Value::Object(tagged(seq)), &mut out).unwrap();
        d.decode_row(Value::Object(u(seq + 1, json!({}))), &mut out)
            .unwrap();
        err(
            d.finish(&mut out),
            "unseeded Session contains an inherited end-seed marker",
        );
    }
}
#[test]
fn preserves_ordinary_untagged_markers_during_recovery() {
    let mut d = decoder(false, SessionFormatRecovery::Recoverable);
    let mut out = SessionFormatEventCollector::new();
    let marker = e("session/end-seed", 0, json!({}));
    d.decode_row(Value::Object(marker.clone()), &mut out)
        .unwrap();
    d.decode_row(Value::Object(u(1, json!({}))), &mut out)
        .unwrap();
    assert_eq!(d.finish(&mut out).unwrap(), 0);
    assert_eq!(out.values, vec![marker]);
}
#[test]
fn preserves_a_canonical_native_artifact_through_transformed_catalog_decoding() {
    let rows = tool_log(false, None);
    let encoded = rows
        .iter()
        .map(|e| RELEASED_V3_SESSION_FORMAT_CODEC.encode_event(e).unwrap())
        .collect();
    assert_eq!(
        read(
            physical(h(3, "identity")),
            encoded,
            SessionFormatRecovery::Strict,
            SessionFormatValidation::Transformed
        )
        .unwrap(),
        artifact(rows)
    );
}
#[test]
fn defers_unclassified_required_metadata_until_vocabulary_aware_restoration() {
    for metadata in [json!({"surfaceOp":"append"}), json!({"sourceEventSeqs":[]})] {
        let mut row = e("future/required", 0, json!({}));
        row.extend(obj(metadata));
        let mut d = decoder(false, SessionFormatRecovery::Recoverable);
        let mut out = SessionFormatEventCollector::new();
        d.decode_row(Value::Object(row.clone()), &mut out).unwrap();
        assert_eq!(d.finish(&mut out).unwrap(), 0);
        assert_eq!(out.values, vec![row]);
        err(restore(artifact(out.values.clone())), "unknown event type");
        err(
            restore_released_v3_artifact(
                artifact(out.values),
                &HashSet::from(["future/required".into()]),
            ),
            "unexpected field",
        );
    }
}
#[test]
fn admits_installed_required_extension_payloads_without_surface_metadata() {
    let a = artifact(vec![e("future/required", 0, json!({"extension":true}))]);
    assert_eq!(
        restore_released_v3_artifact(a.clone(), &HashSet::from(["future/required".into()]))
            .unwrap(),
        a
    );
}
fn replacements(start: u64, end: u64, sources: Option<Value>) -> Vec<SessionFormatEvent> {
    let mut fields = json!({"surfaceOp":{"op":"replace","start":start,"end":end}});
    if let Some(s) = sources {
        fields["sourceEventSeqs"] = s;
    }
    with(
        open(),
        [
            u(2, json!({"surfaceOp":"append"})),
            u(3, json!({"surfaceOp":"append"})),
            u(4, json!({"surfaceOp":"append"})),
            u(
                5,
                json!({"surfaceOp":{"op":"replace","start":2,"end":3},"sourceEventSeqs":[2,3]}),
            ),
            u(6, fields),
        ],
    )
}
#[test]
fn accepts_repeated_replacement_whose_endpoints_reverse_numeric_order() {
    let a = raw_rows(replacements(5, 4, Some(json!([5, 4])))).unwrap();
    assert_eq!(
        a.events[7]["surfaceOp"],
        json!({"op":"replace","startSeq":6,"endSeq":5})
    );
    restore(a).unwrap();
    let rows = vec![
        u(0, json!({"surfaceOp":"append"})),
        u(1, json!({"surfaceOp":"append"})),
        u(2, json!({"surfaceOp":"append"})),
        u(
            3,
            json!({"surfaceOp":{"op":"replace","startSeq":0,"endSeq":1},"sourceEventSeqs":[0,1]}),
        ),
        u(
            4,
            json!({"surfaceOp":{"op":"replace","startSeq":3,"endSeq":2},"sourceEventSeqs":[3,2]}),
        ),
    ];
    restore(artifact(rows)).unwrap();
}
#[test]
fn retains_range_and_source_coverage_validation() {
    for (start, end, sources, msg) in [
        (
            2,
            4,
            Some(json!([2, 4])),
            "range is not on the current surface",
        ),
        (
            4,
            5,
            Some(json!([4, 5])),
            "range is not on the current surface",
        ),
        (5, 4, None, "omit a shadowed surface node"),
        (5, 4, Some(json!([5])), "omit a shadowed surface node"),
    ] {
        err(
            restore(raw_rows(replacements(start, end, sources)).unwrap()),
            msg,
        );
    }
}
#[test]
fn retains_advertised_tool_lifecycle_validation_after_local_envelope_admission() {
    let mut rows = tool_log(true, None);
    rows[3] = e("feedback/record", 3, json!({"text":"not a tool call"}));
    err(
        restore(raw_rows(rows).unwrap()),
        "not the exact TOOL_NOT_STARTED repair",
    );
}
