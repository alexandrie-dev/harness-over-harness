mod common;
use common::*;
fn fact() -> SessionFormatEvent {
    let mut e = ev("feedback/record", json!({"text":"retained"}));
    e.insert("time".into(), 2.into());
    e
}
fn seed() -> SessionFormatEvent {
    e("session/end-seed", 1, json!({"inherited":true}))
}
fn delivery(version: Option<u64>, id: &str) -> SessionFormatEvent {
    let mut marker = e(
        "session-log-deepseek/delivery-accepted",
        1,
        json!({"sessionId":id,"throughSeq":0}),
    );
    if let Some(v) = version {
        marker["data"]["sessionFormatVersion"] = v.into();
    }
    marker
}
fn run(
    events: Vec<SessionFormatEvent>,
    header: SessionFormatHeader,
    cut: Option<u64>,
) -> R<SessionFormatArtifact> {
    migrate(events, header, cut, vec![])
}
fn seeded(parent: bool) -> SessionFormatHeader {
    let mut header = h(3, "identity");
    header.insert("isSeeded".into(), true.into());
    if parent {
        header.insert("parentSession".into(), "ancestor".into());
    }
    header
}
#[test]
fn rejects_required_v3_developer_events_while_preserving_ignorable_events_and_native_v4_admission()
{
    let mut rows = opening();
    rows.push(surface("developer/message",json!({"turn":1,"step":1,"message":{"id":"developer","role":"developer","source":{"kind":"tool-registry"},"content":[]}})));
    rows.extend(closing());
    let rows = numbered(rows, 1);
    let before = rows.clone();
    let message = "format v3 contains unknown event type \"developer/message\" at seq 2";
    err(run(rows.clone(), h(3, "identity"), None), message);
    err(
        restore_v3(&rows, h(3, "identity"), SessionFormatRecovery::Strict),
        message,
    );
    let mut ignorable = rows.clone();
    ignorable[2].insert("ignorable".into(), true.into());
    let mut expected = ignorable.clone();
    expected[2].insert("type".into(), "plugin:developer/message".into());
    assert_eq!(
        restore_v3(&ignorable, h(3, "identity"), SessionFormatRecovery::Strict)
            .unwrap()
            .events,
        expected
    );
    assert_eq!(native(rows.clone()).unwrap().events, rows);
    assert_eq!(rows, before);
}
#[test]
fn rejects_generic_required_v3_extensions_before_target_vocabulary_admission() {
    let mut required = fact();
    required.insert("type".into(), "external/required".into());
    err(
        run(vec![required.clone()], h(3, "identity"), None),
        "format v3 contains unknown event type \"external/required\" at seq 0",
    );
    err(
        restore_v3(
            std::slice::from_ref(&required),
            h(3, "identity"),
            SessionFormatRecovery::Strict,
        ),
        "format v3 contains unknown event type",
    );
    required.insert("ignorable".into(), true.into());
    let mut expected = required.clone();
    expected.insert("type".into(), "plugin:external/required".into());
    assert_eq!(
        restore_v3(&[required], h(3, "identity"), SessionFormatRecovery::Strict)
            .unwrap()
            .events,
        vec![expected]
    );
}
#[test]
fn changes_only_the_header_version_and_retains_event_objects_payloads_timestamps_and_coordinates() {
    let rows = vec![fact(), delivery(Some(3), "identity")];
    let before = rows.clone();
    let out = run(rows.clone(), h(3, "identity"), None).unwrap();
    assert_eq!(out.header, h(4, "identity"));
    assert_eq!(out.events, rows);
    assert_eq!(out.inherited_event_count, 0);
    assert_eq!(rows, before);
    assert_eq!(
        stage(&h(3, "identity"), None, vec![])
            .unwrap()
            .header_inherited_event_count(),
        Some(0)
    );
    assert!(
        run(vec![], h(3, "identity"), None)
            .unwrap()
            .events
            .is_empty()
    );
    assert_eq!(
        restore_v3(&rows, h(3, "identity"), SessionFormatRecovery::Strict).unwrap(),
        out
    );
    assert_eq!(
        run(rows.clone(), h(3, "identity"), None).unwrap(),
        run(rows, h(3, "identity"), None).unwrap()
    );
}
#[test]
fn keeps_interleaved_stage_state_independent_and_derives_an_inherited_cut_unavailable_before_eof() {
    let mut inherited = stage(&seeded(true), None, vec![]).unwrap();
    let mut local = stage(&h(3, "identity"), None, vec![]).unwrap();
    let mut a = SessionFormatEventCollector::new();
    let mut b = SessionFormatEventCollector::new();
    assert_eq!(inherited.header_inherited_event_count(), None);
    inherited.transform_event(fact(), &mut a).unwrap();
    local.transform_event(fact(), &mut b).unwrap();
    inherited.transform_event(seed(), &mut a).unwrap();
    assert_eq!(local.finish(&mut b).unwrap(), 0);
    assert_eq!(inherited.finish(&mut a).unwrap(), 1);
    assert_eq!(a.values, vec![fact(), seed()]);
    assert_eq!(b.values, vec![fact()]);
    assert_eq!(
        run(vec![fact(), seed()], seeded(false), Some(1))
            .unwrap()
            .inherited_event_count,
        1
    );
}
#[test]
fn consumes_compact_runs_without_changing_their_event_values() {
    let mut s = stage(&h(3, "identity"), None, vec![]).unwrap();
    let mut out = SessionFormatEventCollector::new();
    s.transform_run(Box::new(Run(vec![fact()])), &mut out)
        .unwrap();
    assert_eq!(s.finish(&mut out).unwrap(), 0);
    assert_eq!(out.values, vec![fact()]);
}
#[test]
fn rejects_sparse_sequences_and_inconsistent_inheritance() {
    let mut sparse = fact();
    sparse.insert("seq".into(), 1.into());
    err(run(vec![sparse], h(3, "identity"), None), "dense");
    err(
        run(vec![fact(), seed()], h(3, "identity"), None),
        "unseeded",
    );
    err(
        run(vec![fact()], seeded(false), None),
        "inherited event count",
    );
    err(
        run(vec![fact(), seed()], seeded(false), Some(0)),
        "disagrees",
    );
    err(
        run(
            vec![fact(), e("session/end-seed", 1, json!({}))],
            seeded(false),
            None,
        ),
        "inherited event count",
    );
}
#[test]
fn refuses_target_generation_delivery_while_retaining_other_generations_unchanged() {
    err(
        run(
            vec![fact(), delivery(Some(4), "identity")],
            h(3, "identity"),
            None,
        ),
        "claims target format v4",
    );
    err(
        restore_v3(
            &[fact(), delivery(Some(4), "identity")],
            h(3, "identity"),
            SessionFormatRecovery::Strict,
        ),
        "claims target format v4",
    );
    for version in [None, Some(0), Some(1), Some(2), Some(3), Some(5), Some(99)] {
        let marker = delivery(version, "identity");
        assert_eq!(
            run(vec![fact(), marker.clone()], h(3, "identity"), None)
                .unwrap()
                .events[1],
            marker
        );
        assert_eq!(
            restore_v3(
                &[fact(), marker.clone()],
                h(3, "identity"),
                SessionFormatRecovery::Strict
            )
            .unwrap()
            .events[1],
            marker
        );
    }
}
#[test]
fn checks_active_v3_coordinates_before_the_delivery_becomes_historical() {
    for through in [json!(-1), json!(1), json!(1.5)] {
        let mut marker = delivery(Some(3), "identity");
        marker["data"]["throughSeq"] = through;
        err(
            run(vec![fact(), marker], h(3, "identity"), None),
            "throughSeq",
        );
    }
    let mut marker = delivery(Some(3), "");
    err(
        run(vec![fact(), marker.clone()], h(3, "identity"), None),
        "nonempty",
    );
    marker["data"]["sessionId"] = "identity".into();
    marker["data"]["throughSeq"] = 1.into();
    err(
        restore_v3(
            &[fact(), marker],
            h(3, "identity"),
            SessionFormatRecovery::Strict,
        ),
        "throughSeq",
    );
    let mut historical = delivery(Some(2), "ancestor");
    historical["data"]["throughSeq"] = 500.into();
    assert_eq!(
        restore_v3(
            &[fact(), historical.clone()],
            h(3, "identity"),
            SessionFormatRecovery::Strict
        )
        .unwrap()
        .events[1],
        historical
    );
}
#[test]
fn permits_foreign_active_deliveries_only_inside_a_parent_seed() {
    let foreign = delivery(Some(3), "ancestor");
    err(
        run(vec![fact(), foreign.clone()], h(3, "identity"), None),
        "wrong Session",
    );
    let mut header = h(3, "identity");
    header.insert("parentSession".into(), "ancestor".into());
    err(
        run(vec![fact(), foreign.clone()], header, None),
        "wrong Session",
    );
    let rows = dense(vec![fact(), foreign.clone(), seed()]);
    assert_eq!(
        run(rows.clone(), seeded(true), None)
            .unwrap()
            .inherited_event_count,
        2
    );
    err(run(rows, seeded(false), None), "wrong Session");
    err(
        run(dense(vec![fact(), seed(), foreign]), seeded(true), None),
        "wrong Session",
    );
}
#[test]
fn retains_unknown_ignorable_values_and_refuses_unknown_required_events_during_strict_restoration()
{
    let mut opaque = ev(
        "external/opaque",
        json!({"nested":{"seq":40},"content":["opaque"]}),
    );
    opaque.insert("time".into(), (-5).into());
    opaque.insert("ignorable".into(), true.into());
    opaque.insert("surfaceOp".into(), json!({"future":{"ref":9}}));
    let mut expected = opaque.clone();
    expected.insert("type".into(), "plugin:external/opaque".into());
    assert_eq!(
        restore_v3(
            &[opaque.clone()],
            h(3, "identity"),
            SessionFormatRecovery::Strict
        )
        .unwrap()
        .events,
        vec![expected]
    );
    opaque.shift_remove("ignorable");
    err(
        restore_v3(&[opaque], h(3, "identity"), SessionFormatRecovery::Strict),
        "unknown event type",
    );
    let mut fractional = fact();
    fractional.insert("time".into(), json!(1.5));
    err(
        restore_v3(
            &[fractional],
            h(3, "identity"),
            SessionFormatRecovery::Strict,
        ),
        "time",
    );
    let mut extra = fact();
    extra.insert("extra".into(), 1.into());
    err(
        restore_v3(&[extra], h(3, "identity"), SessionFormatRecovery::Strict),
        "field",
    );
}
#[test]
fn retains_delivery_inside_a_nested_inherited_prefix_during_restoration() {
    let rows = dense(vec![fact(), seed(), delivery(Some(3), "ancestor"), seed()]);
    let physical = {
        let mut p = obj(json!({"type":"session"}));
        p.extend(seeded(true));
        Value::Object(p)
    };
    let mut decoder = RELEASED_V3_SESSION_FORMAT_CODEC
        .create_decoder(&physical, SessionFormatRecovery::Strict)
        .unwrap();
    let mut out = SessionFormatEventCollector::new();
    for row in &rows {
        decoder.decode_row(row.clone().into(), &mut out).unwrap();
    }
    let cut = decoder.finish(&mut out).unwrap();
    let original = hoh_session_format_v2_to_v3::restore_released_v3_artifact(
        artifact(out.values, decoder.header().clone(), cut),
        &known(&rows),
    )
    .unwrap();
    assert_eq!(original.inherited_event_count, 3);
    for recovery in [
        SessionFormatRecovery::Strict,
        SessionFormatRecovery::Recoverable,
    ] {
        let restored = restore_v3(&rows, seeded(true), recovery).unwrap();
        assert_eq!(restored.events, original.events);
        assert_eq!(restored.inherited_event_count, 3);
        assert_eq!(restored.header, h_with_seed());
    }
}
fn h_with_seed() -> SessionFormatHeader {
    let mut h = seeded(true);
    h.insert("version".into(), 4.into());
    h
}
#[test]
fn refuses_foreign_delivery_after_the_final_inherited_cut_during_restoration() {
    let rows = dense(vec![
        fact(),
        seed(),
        delivery(Some(3), "ancestor"),
        seed(),
        delivery(Some(3), "ancestor"),
    ]);
    for recovery in [
        SessionFormatRecovery::Strict,
        SessionFormatRecovery::Recoverable,
    ] {
        err(restore_v3(&rows, seeded(true), recovery), "wrong Session");
    }
}
