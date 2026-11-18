mod common;
use common::*;
#[test]
fn composes_seeded_systems_source_ownership_ptc_canonical_replacements_and_native_round_trip() {
    let target = restore_v2(&canonical_source(), canonical_header()).unwrap();
    assert_eq!(target.header["version"], 4);
    assert_eq!(target.inherited_event_count, 13);
    let systems: Vec<_> = target
        .events
        .iter()
        .filter(|e| e["type"] == "system/message")
        .collect();
    assert_eq!(systems.len(), 3);
    for system in systems {
        assert_eq!(system["data"]["message"]["role"], "system");
        assert_eq!(
            system["data"]["message"]["source"],
            json!({"kind":"system-prompt"})
        );
    }
    let users: Vec<_> = target
        .events
        .iter()
        .filter(|e| e["type"] == "user/message")
        .collect();
    assert_eq!(
        users[0]["data"]["source"],
        json!({"kind":"runtime-context"})
    );
    assert_eq!(
        users[1]["surfaceOp"],
        json!({"op":"replace","startSeq":5,"endSeq":5})
    );
    assert_eq!(users[1]["sourceEventSeqs"], json!([5]));
    assert_eq!(users[1]["data"]["source"], json!({"kind":"ptc-mode"}));
    assert_eq!(
        target
            .events
            .iter()
            .find(|e| e["type"] == "tool/ptc-dispatch")
            .unwrap()["data"]["arguments"],
        json!({"kind":"plugin","plugin":"tools-ptc"})
    );
    assert_eq!(
        restore_released_v4_artifact(target.clone(), &known(&target.events)).unwrap(),
        target
    );
    assert_eq!(
        round_trip(&target.events, &target.header, target.inherited_event_count).unwrap(),
        (target.events.clone(), target.inherited_event_count)
    );
}
#[test]
fn leaves_common_event_and_message_validation_to_current_session_restoration() {
    let mut header = h(4, "canonical-v4");
    header.insert("parentSession".into(), "parent".into());
    let mut d = RELEASED_V4_SESSION_FORMAT_CODEC
        .create_decoder(
            &RELEASED_V4_SESSION_FORMAT_CODEC
                .encode_header(&header, 0)
                .unwrap()
                .into(),
            SessionFormatRecovery::Recoverable,
        )
        .unwrap();
    let mut out = SessionFormatEventCollector::new();
    d.decode_row(Value::Null, &mut out).unwrap();
    assert_eq!(d.finish(&mut out).unwrap(), 0);
}
#[test]
fn keeps_earlier_generation_delivery_markers_ignored_and_validates_native_current_ownership() {
    let marker = ev(
        "session-log-deepseek/delivery-accepted",
        json!({"sessionId":"other","throughSeq":0,"sessionFormatVersion":3}),
    );
    let a = artifact(vec![marker.clone()], h(4, "canonical-v4"), 0);
    assert_eq!(
        restore_released_v4_artifact(a.clone(), &known(&a.events)).unwrap(),
        a
    );
    let mut current = marker.clone();
    current["data"]["sessionFormatVersion"] = 4.into();
    let rows = dense(vec![ev("feedback/record", json!({})), current]);
    err(
        restore_released_v4_artifact(
            artifact(rows.clone(), h(4, "canonical-v4"), 0),
            &known(&rows),
        ),
        "wrong Session",
    );
    let rows = dense(vec![ev("turn/start", json!({"turn":1})), marker]);
    err(
        restore_v3(&rows, h(3, "canonical-v4"), SessionFormatRecovery::Strict),
        "wrong Session",
    );
}
#[test]
fn retains_native_compaction_checkpoint_sources_during_restoration() {
    let mut checkpoint = user("checkpoint");
    checkpoint["data"]["source"] = json!({"kind":"compact-checkpoint","compactionId":"compact-1"});
    checkpoint["data"]["content"] = json!([{"type":"text","text":"summary"}]);
    checkpoint.insert(
        "surfaceOp".into(),
        json!({"op":"replace","startSeq":1,"endSeq":1}),
    );
    checkpoint.insert("sourceEventSeqs".into(), json!([2, 3, 1]));
    let rows = dense(vec![
        ev("turn/start", json!({"turn":1})),
        user("input"),
        ev(
            "compaction/start",
            json!({"compactionId":"compact-1","turn":1}),
        ),
        ev(
            "compaction/summary",
            json!({"compactionId":"compact-1","summary":[{"type":"text","text":"summary"}],"shadowedRange":{"start":1,"end":1},"shadowedSeqs":[1],"shadowedTokenCount":1,"provider":"mock","model":"mock"}),
        ),
        checkpoint,
        ev(
            "compaction/end",
            json!({"compactionId":"compact-1","turn":1}),
        ),
        ev("turn/end", json!({"turn":1,"reason":{"kind":"completed"}})),
    ]);
    let a = artifact(rows, h(4, "compact-v4"), 0);
    assert_eq!(
        restore_released_v4_artifact(a.clone(), &known(&a.events)).unwrap(),
        a
    );
    assert_eq!(
        a.events[4]["data"]["source"],
        json!({"kind":"compact-checkpoint","compactionId":"compact-1"})
    );
}
