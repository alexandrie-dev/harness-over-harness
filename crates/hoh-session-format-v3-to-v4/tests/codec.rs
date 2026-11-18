mod common;
use common::*;
use hoh_session_format_v2_to_v3::RELEASED_V2_SESSION_FORMAT_CODEC;
use hoh_session_format_v3_to_v4::validation::assert_released_v4_header_value;
fn fact() -> SessionFormatEvent {
    ev("feedback/record", json!({"text":"retained"}))
}
fn catalog() -> SessionFormatEvent {
    ev(
        "subagent/catalog",
        json!({"version":0,"childId":"child","childCreatedAt":2,"mode":"one-shot"}),
    )
}
fn types() -> HashSet<String> {
    [
        "subagent/catalog",
        "feedback/record",
        "session-log-deepseek/delivery-accepted",
        "session/end-seed",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
fn restore(events: Vec<SessionFormatEvent>) -> R<SessionFormatArtifact> {
    restore_released_v4_artifact(artifact(events, h(4, "parent"), 0), &types())
}
fn delivery(version: Option<Value>, id: &str) -> SessionFormatEvent {
    let mut e = e(
        "session-log-deepseek/delivery-accepted",
        1,
        json!({"sessionId":id,"throughSeq":0}),
    );
    if let Some(v) = version {
        e["data"]["sessionFormatVersion"] = v;
    }
    e
}
#[test]
fn reuses_released_v2_physical_framing_and_round_trips_native_v4_rows() {
    let header = h(4, "parent");
    let physical = RELEASED_V4_SESSION_FORMAT_CODEC
        .encode_header(&header, 0)
        .unwrap();
    let mut expected = obj(json!({"type":"session"}));
    expected.extend(header.clone());
    assert_eq!(physical, expected);
    assert_eq!(
        RELEASED_V4_SESSION_FORMAT_CODEC
            .decode_header(&physical.clone().into())
            .unwrap(),
        header
    );
    let mut decoder = RELEASED_V4_SESSION_FORMAT_CODEC
        .create_decoder(&physical.into(), SessionFormatRecovery::Strict)
        .unwrap();
    assert_eq!(decoder.header(), &header);
    let row = RELEASED_V4_SESSION_FORMAT_CODEC
        .encode_event(&fact())
        .unwrap();
    assert_eq!(
        row,
        RELEASED_V2_SESSION_FORMAT_CODEC
            .encode_event(&fact())
            .unwrap()
    );
    let mut out = SessionFormatEventCollector::new();
    decoder.decode_row(row.into(), &mut out).unwrap();
    assert_eq!(decoder.finish(&mut out).unwrap(), 0);
    assert_eq!(out.values, vec![fact()]);
    let a = artifact(out.values, header, 0);
    assert_eq!(
        restore_released_v4_artifact(a.clone(), &types()).unwrap(),
        a
    );
}
#[test]
fn rejects_mismatched_versions_and_malformed_logical_and_physical_metadata() {
    err(assert_released_v4_header(&h(3, "parent")), "v4 header");
    let mut header = h(4, "parent");
    header.insert("createdAt".into(), (-1).into());
    err(assert_released_v4_header(&header), "createdAt");
    err(
        RELEASED_V4_SESSION_FORMAT_CODEC.decode_header(&Value::Null),
        "v4 physical",
    );
    err(
        RELEASED_V4_SESSION_FORMAT_CODEC
            .create_decoder(&json!({"version":3}), SessionFormatRecovery::Strict),
        "v4 physical",
    );
    let mut physical = obj(json!({"type":"session"}));
    physical.extend(h(4, "parent"));
    physical.insert("extra".into(), 1.into());
    err(
        RELEASED_V4_SESSION_FORMAT_CODEC.decode_header(&physical.into()),
        "field",
    );
}
#[test]
fn rejects_malformed_optional_and_required_native_header_fields() {
    let mut missing = h(4, "parent");
    missing.shift_remove("id");
    err(
        assert_released_v4_header(&missing),
        "lacks required field id",
    );
    for (key, value, message) in [
        ("extra", json!(true), "unexpected field extra"),
        ("id", json!(7), "id must be a string"),
        ("isSeeded", json!("no"), "isSeeded must be boolean"),
        ("cwd", json!("relative"), "cwd must be absolute"),
        ("cwd", json!(7), "cwd must be absolute"),
        ("parentSession", json!(7), "parentSession must be a string"),
        ("agentPreset", json!(7), "agentPreset must be a string"),
        ("origin", json!("parent"), "origin must be \"subagent\""),
    ] {
        let mut header = h(4, "parent");
        header.insert(key.into(), value);
        err(assert_released_v4_header(&header), message);
    }
    err(
        assert_released_v4_header_value(&Value::Null),
        "expected format v4 header",
    );
}
#[test]
fn rejects_released_wrapper_rows_before_structural_recovery_can_admit_them() {
    err(
        assert_v4_row_admission(
            &ev("tool/result", json!({"message":{"role":"user"}})).into(),
            None,
        ),
        "first-class message",
    );
}
#[test]
fn retains_predecessor_and_future_delivery_generations_without_activating_their_watermarks() {
    for version in [None, Some(0), Some(1), Some(2), Some(3), Some(5)] {
        let mut marker = delivery(version.map(Value::from), "foreign");
        marker["data"]["throughSeq"] = 500.into();
        assert_eq!(
            restore(vec![fact(), marker.clone()]).unwrap().events[1],
            marker
        );
    }
    let marker = delivery(Some(4.into()), "parent");
    assert_eq!(
        restore(vec![fact(), marker.clone()]).unwrap().events[1],
        marker
    );
    err(
        restore(vec![fact(), delivery(Some(4.into()), "foreign")]),
        "wrong Session",
    );
}
#[test]
fn rejects_malformed_active_delivery_coordinates_and_malformed_generation_identifiers() {
    for value in [json!(-1), json!(1), json!(1.5)] {
        let mut marker = delivery(Some(4.into()), "parent");
        marker["data"]["throughSeq"] = value;
        err(restore(vec![fact(), marker]), "throughSeq");
    }
    for id in [json!(""), Value::Null, json!(3)] {
        let mut marker = delivery(Some(4.into()), "parent");
        marker["data"]["sessionId"] = id;
        err(restore(vec![fact(), marker]), "nonempty");
    }
    for v in [json!(-1), json!(-0.0), json!(1.5), json!("4"), Value::Null] {
        err(
            restore(vec![fact(), delivery(Some(v), "parent")]),
            "sessionFormatVersion",
        );
    }
    let mut marker = delivery(Some(4.into()), "parent");
    marker.insert("data".into(), Value::Null);
    err(restore(vec![fact(), marker]), "data must be an object");
    let mut marker = delivery(Some(4.into()), "parent");
    marker["data"]["throughSeq"] = 1.into();
    let (decoded, _) = round_trip(&[fact(), marker], &h(4, "parent"), 0).unwrap();
    err(restore(decoded), "throughSeq");
}
#[test]
fn preserves_inherited_and_local_events_and_admits_foreign_active_deliveries_inside_the_seed_only()
{
    let mut header = h(4, "parent");
    header.insert("isSeeded".into(), true.into());
    header.insert("parentSession".into(), "ancestor".into());
    let a = artifact(
        vec![
            fact(),
            delivery(Some(4.into()), "ancestor"),
            e("session/end-seed", 2, json!({"inherited":true})),
            e("feedback/record", 3, json!({"text":"retained"})),
        ],
        header,
        2,
    );
    assert_eq!(
        restore_released_v4_artifact(a.clone(), &types()).unwrap(),
        a
    );
    let mut bad = a;
    bad.inherited_event_count = 1;
    err(restore_released_v4_artifact(bad, &types()), "marker");
}
#[test]
fn rejects_invalid_inherited_cuts_sparse_rows_and_unseeded_inherited_markers() {
    for (events, cut, message) in [
        (vec![], 1, "exceeds its events"),
        (
            vec![fact()],
            1,
            "unseeded format v4 Session has inherited events",
        ),
        (
            vec![e("feedback/record", 1, json!({"text":"retained"}))],
            0,
            "not dense",
        ),
        (
            vec![ev("session/end-seed", json!({"inherited":true}))],
            0,
            "unseeded Session contains an inherited end-seed marker",
        ),
    ] {
        err(
            restore_released_v4_artifact(artifact(events, h(4, "parent"), cut), &types()),
            message,
        );
    }
}
#[test]
fn preserves_unknown_ignorable_values_and_requires_installed_vocabulary_for_unknown_required_events()
 {
    let mut opaque = ev("external/opaque", json!({"untouched":["a",3]}));
    opaque.insert("ignorable".into(), true.into());
    assert_eq!(restore(vec![opaque.clone()]).unwrap().events[0], opaque);
    let required = ev("external/required", Value::Null);
    err(restore(vec![required.clone()]), "unknown event type");
    let a = artifact(vec![required.clone()], h(4, "parent"), 0);
    assert_eq!(
        restore_released_v4_artifact(a.clone(), &known(&[required])).unwrap(),
        a
    );
}
#[test]
fn refuses_wrapper_shaped_and_malformed_native_tool_results_before_recovery() {
    let wrapper = result(
        json!({"id":"result","role":"user","source":{"kind":"tool","callId":"call"},"content":[]}),
    );
    let bad = result(native_message("call", "result", Some("yes".into())));
    for row in [wrapper, bad] {
        assert!(assert_v4_row_admission(&row.clone().into(), None).is_err());
        let physical = RELEASED_V4_SESSION_FORMAT_CODEC
            .encode_header(&h(4, "parent"), 0)
            .unwrap();
        let mut d = RELEASED_V4_SESSION_FORMAT_CODEC
            .create_decoder(&physical.into(), SessionFormatRecovery::Recoverable)
            .unwrap();
        let mut out = SessionFormatEventCollector::new();
        d.decode_row(Value::Null, &mut out).unwrap();
        assert!(d.decode_row(row.into(), &mut out).is_err());
    }
}
#[test]
fn leaves_ordinary_framing_recovery_to_the_released_decoder() {
    assert_v4_row_admission(&Value::Null, None).unwrap();
    assert_v4_row_admission(&fact().into(), None).unwrap();
    let header = RELEASED_V4_SESSION_FORMAT_CODEC
        .encode_header(&h(4, "parent"), 0)
        .unwrap();
    let mut d = RELEASED_V4_SESSION_FORMAT_CODEC
        .create_decoder(&header.into(), SessionFormatRecovery::Recoverable)
        .unwrap();
    let mut out = SessionFormatEventCollector::new();
    d.decode_row(fact().into(), &mut out).unwrap();
    d.decode_row(Value::Null, &mut out).unwrap();
    assert_eq!(d.finish(&mut out).unwrap(), 0);
    assert_eq!(out.values, vec![fact()]);
}
#[test]
fn rejects_duplicate_own_catalog_membership() {
    err(
        restore(dense(vec![catalog(), catalog()])),
        "duplicate catalog child",
    );
}
#[test]
fn keeps_inherited_catalog_entries_outside_own_uniqueness_checks() {
    let mut header = h(4, "parent");
    header.insert("isSeeded".into(), true.into());
    header.insert("parentSession".into(), "ancestor".into());
    let a = artifact(
        dense(vec![
            catalog(),
            ev("session/end-seed", json!({"inherited":true})),
            catalog(),
        ]),
        header,
        1,
    );
    assert_eq!(
        restore_released_v4_artifact(a.clone(), &types()).unwrap(),
        a
    );
}
#[test]
fn restores_external_membership_and_rejects_duplicate_catalog_children() {
    let mut entry = catalog();
    entry["data"]["version"] = 2.into();
    entry["data"]["mode"] = "external".into();
    entry["data"]["label"] = "External".into();
    assert_eq!(
        restore(vec![entry.clone()]).unwrap().events,
        vec![entry.clone()]
    );
    err(
        restore(dense(vec![entry.clone(), entry])),
        "duplicate catalog child",
    );
}
#[test]
fn rejects_invalid_names_in_native_catalog_v3() {
    for name in ["", "Reviewer", "a/b", "1reviewer", &"a".repeat(33), "lead"] {
        for mode in ["continuable", "external"] {
            let row = ev(
                "subagent/catalog",
                json!({"version":3,"childId":"child","childCreatedAt":2,"mode":mode,"label":"Named","name":name}),
            );
            err(restore(vec![row]), "supported versioned catalog fact");
        }
    }
}
#[test]
fn rejects_a_repeated_sibling_name_in_own_catalog_facts() {
    let named = |seq, id, name| {
        e(
            "subagent/catalog",
            seq,
            json!({"version":3,"childId":id,"childCreatedAt":2,"mode":"continuable","label":"Named","name":name}),
        )
    };
    assert_eq!(
        restore(vec![
            named(0, "first", "reviewer"),
            named(1, "second", "checker")
        ])
        .unwrap()
        .events
        .len(),
        2
    );
    err(
        restore(vec![
            named(0, "first", "reviewer"),
            named(1, "second", "reviewer"),
        ]),
        "duplicate catalog child name reviewer",
    );
}
#[test]
fn rejects_a_catalog_mode_outside_its_payload_version() {
    for (version, mode) in [
        (0, "external"),
        (1, "external"),
        (2, "one-shot"),
        (2, "continuable"),
        (2, "unknown"),
    ] {
        let mut entry = catalog();
        entry["data"]["version"] = version.into();
        entry["data"]["mode"] = mode.into();
        err(restore(vec![entry]), "supported versioned catalog fact");
    }
}
#[test]
fn admits_a_native_named_catalog_fact() {
    for (mode, label, name) in [
        ("continuable", Some("Named"), "reviewer"),
        ("external", Some("Named"), "auditor"),
        ("external", None, "auditor"),
    ] {
        let mut entry = catalog();
        entry["data"]["version"] = 3.into();
        entry["data"]["mode"] = mode.into();
        entry["data"]["name"] = name.into();
        if let Some(label) = label {
            entry["data"]["label"] = label.into();
        }
        assert_eq!(restore(vec![entry.clone()]).unwrap().events, vec![entry]);
    }
}
#[test]
fn rejects_an_invalid_native_named_catalog_fact() {
    for override_ in [
        json!({"version":3,"mode":"continuable","label":"Named"}),
        json!({"version":3,"mode":"continuable","name":"reviewer"}),
        json!({"version":3,"mode":"external","label":1,"name":"auditor"}),
        json!({"version":3,"mode":"one-shot","name":"reviewer"}),
        json!({"version":3,"mode":"unknown","name":"reviewer"}),
        json!({"version":3,"mode":"continuable","label":"Named","name":"reviewer","childId":1}),
    ] {
        let mut entry = catalog();
        entry["data"]
            .as_object_mut()
            .unwrap()
            .extend(obj(override_));
        err(restore(vec![entry]), "supported versioned catalog fact");
    }
}
