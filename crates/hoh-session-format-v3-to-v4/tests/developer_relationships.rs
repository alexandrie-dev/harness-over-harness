mod common;
use common::*;
fn empty() -> SessionFormatEvent {
    surface(
        "developer/message",
        json!({"turn":1,"step":1,"message":{"id":"developer","role":"developer","source":{"kind":"tool-registry"},"content":[]}}),
    )
}
fn initial() -> Vec<SessionFormatEvent> {
    let mut rows = opening();
    rows.push(system());
    rows.push(empty());
    rows
}
#[test]
fn retains_an_empty_developer_position_while_replacing_the_protected_system_head() {
    let mut replacement = system();
    replacement.insert(
        "surfaceOp".into(),
        json!({"op":"replace","startSeq":2,"endSeq":2}),
    );
    replacement.insert("sourceEventSeqs".into(), json!([2]));
    replacement["data"]["message"]["id"] = "replacement".into();
    replacement["data"]["message"]["content"] = json!([{"type":"text","text":"Updated"}]);
    let mut rows = initial();
    rows.push(replacement);
    let a = native(rows).unwrap();
    assert_eq!(a.events[3]["data"], empty()["data"]);
}
#[test]
fn rejects_an_empty_developer_replacement_of_the_protected_system_head() {
    let mut replacement = empty();
    replacement.insert(
        "surfaceOp".into(),
        json!({"op":"replace","startSeq":2,"endSeq":2}),
    );
    replacement.insert("sourceEventSeqs".into(), json!([2]));
    let mut rows = initial();
    rows.pop();
    rows.push(replacement);
    err(native(rows), "protected system head");
}
#[test]
fn includes_empty_developer_nodes_in_compaction_spans_and_protects_the_system_head() {
    let mut rows = initial();
    rows.push(ev(
        "compaction/prune",
        json!({"shadowedRange":{"start":3,"end":3},"shadowedSeqs":[3],"shadowedTokenCount":0}),
    ));
    assert!(native(rows.clone()).is_ok());
    rows.last_mut().unwrap().insert(
        "data".into(),
        json!({"shadowedRange":{"start":2,"end":3},"shadowedSeqs":[2,3],"shadowedTokenCount":0}),
    );
    err(native(rows), "protected system head");
}
#[test]
fn requires_known_developer_messages_to_match_the_current_open_step() {
    err(
        native(vec![opening()[0].clone(), empty()]),
        "open turn and step",
    );
    let mut rows = opening();
    let mut d = empty();
    d["data"]["step"] = 2.into();
    rows.push(d);
    err(native(rows), "open turn and step");
}
#[test]
fn defers_ignorable_developer_payloads_during_physical_decoding_while_keeping_encoder_validation() {
    let mut opaque = ev(
        "developer/message",
        json!({"message":{"role":"developer","source":{"kind":"plugin"}},"future":true}),
    );
    opaque.insert("ignorable".into(), true.into());
    opaque.insert("surfaceOp".into(), json!({"future":true}));
    let mut rows = vec![opaque];
    rows.extend(opening());
    rows.push(system());
    let rows = dense(rows);
    let header = h(4, "developer-relations");
    let mut d = RELEASED_V4_SESSION_FORMAT_CODEC
        .create_decoder(
            &RELEASED_V4_SESSION_FORMAT_CODEC
                .encode_header(&header, 0)
                .unwrap()
                .into(),
            SessionFormatRecovery::Strict,
        )
        .unwrap();
    let mut out = SessionFormatEventCollector::new();
    for e in &rows {
        d.decode_row(e.clone().into(), &mut out).unwrap();
    }
    d.finish(&mut out).unwrap();
    assert_eq!(out.values, rows);
    assert!(
        RELEASED_V4_SESSION_FORMAT_CODEC
            .encode_event(&rows[0])
            .is_err()
    );
    assert_v4_row_admission(&rows[0].clone().into(), Some(&HashSet::new())).unwrap();
    let k = ["developer/message"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    assert!(assert_v4_row_admission(&rows[0].clone().into(), Some(&k)).is_err());
    let mut known = rows[0].clone();
    known.insert("data".into(), empty()["data"].clone());
    assert_eq!(
        RELEASED_V4_SESSION_FORMAT_CODEC
            .encode_event(&known)
            .unwrap()["ignorable"],
        true
    );
}
#[test]
fn keeps_unknown_ignorable_developer_fields_and_coordinates_opaque_before_an_ordinary_step() {
    let mut opaque = ev(
        "developer/message",
        json!({"message":{"role":"developer","source":{"kind":"plugin"}},"future":true}),
    );
    opaque.insert("ignorable".into(), true.into());
    opaque.insert("surfaceOp".into(), json!({"future":true}));
    opaque.insert("sourceEventSeqs".into(), json!({"future":true}));
    let mut rows = vec![opaque];
    rows.extend(opening());
    rows.push(system());
    let rows = dense(rows);
    let mut k = known(&rows);
    k.remove("developer/message");
    let a = artifact(rows, h(4, "developer-relations"), 0);
    assert_eq!(restore_released_v4_artifact(a.clone(), &k).unwrap(), a);
    assert_eq!(
        a.events
            .iter()
            .map(|e| e["seq"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    k.insert("developer/message".into());
    assert!(restore_released_v4_artifact(a, &k).is_err());
}
