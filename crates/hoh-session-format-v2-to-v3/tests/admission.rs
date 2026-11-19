mod common;
use common::*;
use hoh_session_format::*;
use hoh_session_format_v2_to_v3::{payload::assert_event, references::remap_event, *};
use serde_json::{Value, json};
use std::collections::HashSet;
fn system() -> SessionFormatEvent {
    surface(
        "system/message",
        json!({"turn":1,"step":1,"message":{"id":"user","role":"system","content":[{"type":"text","text":"input"}],"source":{"kind":"plugin","plugin":"context"}}}),
    )
}
#[test]
fn rejects_malformed_source_records_before_emitting() {
    let mut ignorable = ev("step/start", json!({"turn":1,"step":1}));
    ignorable.insert("ignorable".into(), false.into());
    for bad in [
        ev("step/start", Value::Null),
        ev("step/start", json!({"turn":1})),
        ignorable,
        ev("user/message", user_data("user")),
        ev("session/end-seed", json!({"inherited":false})),
    ] {
        fails(migrate(vec![bad]));
    }
}
#[test]
fn retains_admitted_ignorable_markers_and_does_not_insert_another_head_on_later_steps() {
    let mut rows = opening();
    let mut feedback = ev("feedback/record", json!({"text":"audit"}));
    feedback.insert("ignorable".into(), true.into());
    rows.extend([
        feedback,
        ev("step/end", json!({"turn":1,"step":1})),
        ev("step/start", json!({"turn":1,"step":2})),
    ]);
    let a = migrate(rows).unwrap();
    assert_eq!(systems(&a).len(), 1);
    assert_eq!(a.events[3]["ignorable"], true);
    restore(a).unwrap();
}
#[test]
fn rejects_non_dense_source_stage_input_rather_than_generating_ambiguous_identities() {
    let mut s = stage(
        &h(2, "admission"),
        Some(0),
        SessionFormatSourceKind::Decoded,
    )
    .unwrap();
    let mut out = SessionFormatEventCollector::new();
    let mut row = opening()[0].clone();
    row.insert("seq".into(), 1.into());
    err(s.transform_event(row, &mut out), "dense");
}
#[test]
fn checks_native_metadata_versions_before_applying_released_header_validation() {
    err(
        assert_released_v3_header(&h(2, "admission")),
        "format v3 header",
    );
    assert_eq!(
        RELEASED_V3_SESSION_FORMAT_CODEC
            .decode_header(&physical(h(3, "admission")))
            .unwrap(),
        h(3, "admission")
    );
}
#[test]
fn retains_native_source_extensions_through_payload_validation_without_classifying_their_references()
 {
    let mut extension = user("user");
    extension["data"]["source"] = json!({"kind":"custom-source","localRef":77});
    assert_event(&extension, 3).unwrap();
    err(assert_event(&extension, 2), "unclassified");
}
#[test]
fn rejects_invalid_system_step_coordinates() {
    for step in [json!(0), json!(-1), json!(1.5)] {
        let mut row = system();
        row["data"]["step"] = step;
        fails(RELEASED_V3_SESSION_FORMAT_CODEC.encode_event(&row));
    }
}
#[test]
fn refuses_appending_system_context_after_a_history_with_no_protected_head() {
    let mut rows = opening();
    rows.extend([user("user"), system()]);
    err(restore(artifact(dense(rows))), "protected first");
}
#[test]
fn rejects_ordinary_replacements_that_consume_the_protected_head() {
    let mut rows = opening();
    let mut row = user("user");
    row.insert(
        "surfaceOp".into(),
        json!({"op":"replace","startSeq":2,"endSeq":2}),
    );
    row.insert("sourceEventSeqs".into(), json!([2]));
    rows.extend([system(), row]);
    err(restore(artifact(dense(rows))), "protected");
}
#[test]
fn rejects_malformed_audited_file_metadata() {
    for attachment in [
        json!({"attachmentId":"","name":"file","bytes":1}),
        json!({"attachmentId":1,"name":"file","bytes":1}),
        json!({"attachmentId":"sha256:abc","name":false,"bytes":1}),
    ] {
        let mut rows = opening();
        let mut row = user("user");
        row["data"]["content"] = json!([{"type":"file","attachment":attachment}]);
        rows.push(row);
        err(migrate(rows), "file attachment");
    }
}
#[test]
fn preserves_negative_ratings_notes_and_deletions() {
    let put = ev(
        "feedback/message-put",
        json!({"sessionId":"admission","item":{"messageId":"assistant","rating":"negative","version":"opaque","createdAt":1,"updatedAt":2,"note":"reason"}}),
    );
    let deleted = ev(
        "feedback/message-delete",
        json!({"sessionId":"admission","messageId":"assistant"}),
    );
    assert_eq!(
        migrate(vec![put.clone(), deleted.clone()])
            .unwrap()
            .events
            .iter()
            .map(|e| e["data"].clone())
            .collect::<Vec<_>>(),
        vec![put["data"].clone(), deleted["data"].clone()]
    );
}
#[test]
fn rejects_malformed_feedback() {
    let base = json!({"messageId":"assistant","rating":"negative","version":"opaque","createdAt":1,"updatedAt":2,"note":"reason"});
    for bad in [
        ev(
            "feedback/message-delete",
            json!({"sessionId":1,"messageId":"id"}),
        ),
        ev(
            "feedback/message-delete",
            json!({"sessionId":"session","messageId":1}),
        ),
    ] {
        err(migrate(vec![bad]), "feedback");
    }
    for (key, v) in [
        ("messageId", json!(false)),
        ("version", json!(0)),
        ("rating", json!("neutral")),
        ("note", json!(12)),
    ] {
        let mut item = base.clone();
        item[key] = v;
        err(
            migrate(vec![ev(
                "feedback/message-put",
                json!({"sessionId":"session","item":item}),
            )]),
            "feedback",
        );
    }
}
#[test]
fn restores_started_tool_results_without_repair_identity_projection() {
    for error in [None, Some(json!({"name":"ToolError","code":"TOOL_ERROR"}))] {
        let mut rows = tool_log(true, error);
        rows[4].shift_remove("sourceEventSeqs");
        let a = migrate(rows).unwrap();
        assert_eq!(restore(a.clone()).unwrap(), a);
    }
}
#[test]
fn rejects_untagged_repair_identities_in_native_restoration_instead_of_projecting_arbitrary_ids() {
    let mut rows = tool_log(
        true,
        Some(json!({"name":"ToolNotStartedError","code":"TOOL_NOT_STARTED"})),
    );
    rows[4].shift_remove("sourceEventSeqs");
    rows.remove(3);
    rows = dense(rows);
    err(restore(artifact(rows.clone())), "exact TOOL_NOT_STARTED");
    rows[3]["data"]["message"]["source"]["callId"] = 123.into();
    err(restore(artifact(rows)), "advertised tool");
}
#[test]
fn rejects_dangling_mappings_and_malformed_reference_arrays_at_the_mapper_boundary() {
    let source = e("command/done", 2, json!({"sourceEventSeq":1}));
    err(remap_event(source.clone(), 3, &[]), "earlier");
    let mut current = source.clone();
    current["data"]["sourceEventSeq"] = 2.into();
    err(remap_event(current, 3, &[0, 1, 2]), "earlier");
    let mut malformed = source;
    malformed.insert("sourceEventSeqs".into(), Value::Null);
    err(remap_event(malformed, 3, &[0, 1]), "array");
}
#[test]
fn leaves_command_completions_without_a_source_unchanged() {
    let a = migrate(vec![
        ev(
            "command/run",
            json!({"commandId":"cmd","name":"test","source":{"kind":"user"}}),
        ),
        ev(
            "command/done",
            json!({"commandId":"cmd","kind":"error","text":"failed"}),
        ),
    ])
    .unwrap();
    assert_eq!(restore(a.clone()).unwrap(), a);
}
#[test]
fn remaps_a_successful_compaction_summary_and_preserves_its_token_accounting() {
    let summary = ev(
        "compaction/summary",
        json!({"compactionId":"compact","summary":[{"type":"text","text":"summary"}],"shadowedRange":{"start":2,"end":2},"shadowedSeqs":[2],"shadowedTokenCount":123,"provider":"mock","model":"mock"}),
    );
    let mut rows = opening();
    rows.extend([
        user("user"),
        ev(
            "compaction/start",
            json!({"compactionId":"compact","turn":1}),
        ),
        summary.clone(),
        ev("compaction/end", json!({"compactionId":"compact","turn":1})),
    ]);
    let a = migrate(rows).unwrap();
    let actual = &a
        .events
        .iter()
        .find(|e| e["type"] == "compaction/summary")
        .unwrap()["data"];
    let mut expected = summary["data"].clone();
    expected["shadowedRange"] = json!({"start":3,"end":3});
    expected["shadowedSeqs"] = json!([3]);
    assert_eq!(*actual, expected);
    restore_released_v3_artifact(a, &HashSet::new()).unwrap();
}
