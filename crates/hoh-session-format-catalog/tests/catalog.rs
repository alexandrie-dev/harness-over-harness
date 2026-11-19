mod common;
use common::*;
use hoh_session_format_catalog::{
    current::validate_installed_current_session_artifact,
    message_projections::current_session_message_projections,
};
#[test]
fn supplies_every_required_current_message_interpreter_and_validates_its_durable_references() {
    let p = current_session_message_projections();
    let again = current_session_message_projections();
    assert!(Arc::ptr_eq(
        &p.lock().unwrap()[0],
        &again.lock().unwrap()[0]
    ));
    let mut actual: Vec<_> = p
        .lock()
        .unwrap()
        .iter()
        .map(|p| p.event_type().to_owned())
        .collect();
    actual.sort();
    let mut expected = hoh_session::known_event_types::MESSAGE_PROJECTION_EVENT_TYPES.to_vec();
    expected.sort();
    assert_eq!(actual, expected);
    let rows = vec![
        user("no image"),
        e(
            "image/offload",
            1,
            json!({"targets":[{"seq":0,"imageIndexes":[0]}]}),
        ),
    ];
    let error =
        validate_installed_current_session_artifact(&artifact(rows, h(4, "projection-catalog"), 0))
            .unwrap_err();
    assert!(
        error.to_string().contains("image index 0 does not exist"),
        "{error}"
    );
}
#[test]
fn statically_owns_every_adjacent_edge_through_the_current_writer() {
    let header = json!({"type":"session","version":0,"id":"catalog","createdAt":1,"seedLength":0,"delegationDepth":0});
    let c = session_format_catalog().unwrap();
    assert_eq!(c.current_version(), hoh_session::SESSION_FORMAT_VERSION);
    assert_eq!(
        c.read_header(&header),
        SessionFormatHeaderReadResult::MigrationRequired {
            stored_version: 0,
            target_version: 4,
            header: obj(
                json!({"version":4,"id":"catalog","createdAt":1,"isSeeded":true,"delegationDepth":0})
            )
        }
    );
    let mut v1 = header;
    v1["version"] = 1.into();
    let a = restore(
        &[ev("turn/start", json!({"turn":1}))],
        &v1,
        SessionFormatValidation::Current,
    )
    .unwrap();
    assert_eq!(a.header["version"], 4);
    assert_eq!(a.header["id"], "catalog");
}
#[test]
fn restores_the_installed_current_vocabulary_without_freezing_ordinary_payload_additions() {
    let header = physical(4, "current-growth");
    let row = ev("turn/start", json!({"turn":1,"postReleaseMember":true}));
    assert_eq!(
        restore(
            std::slice::from_ref(&row),
            &header,
            SessionFormatValidation::Current
        )
        .unwrap()
        .events,
        vec![row]
    );
    err(
        restore(
            &[ev("ordinary/not-installed", json!("future"))],
            &header,
            SessionFormatValidation::Current,
        ),
        "unknown event type",
    );
    let mut extension = ev("ordinary/external", Value::Null);
    extension.insert("ignorable".into(), true.into());
    assert_eq!(
        restore(
            std::slice::from_ref(&extension),
            &header,
            SessionFormatValidation::Current
        )
        .unwrap()
        .events,
        vec![extension]
    );
}
#[test]
fn restores_empty_and_non_empty_inherited_prefixes_through_every_adjacent_edge() {
    for version in [0, 1] {
        for seed_length in [0, 1] {
            let header = json!({"type":"session","version":version,"id":"seed-chain","createdAt":1,"parentSession":"parent","seedLength":seed_length,"delegationDepth":0});
            let rows = if seed_length > 0 {
                vec![ev("feedback/record", json!({"text":"inherited"}))]
            } else {
                vec![]
            };
            let a = restore(&rows, &header, SessionFormatValidation::Current).unwrap();
            assert_eq!(a.header["version"], 4);
            assert_eq!(a.inherited_event_count, seed_length);
            assert_eq!(
                a.events.last().unwrap(),
                &obj(
                    json!({"type":"session/end-seed","seq":seed_length,"time":1,"data":{"inherited":true}})
                )
            );
            assert_eq!(header["version"], version);
        }
    }
}
#[test]
fn inserts_the_v3_system_head_and_remaps_the_inherited_cut() {
    for seeded in [false, true] {
        let mut header = physical(2, "v2-identity");
        header["isSeeded"] = seeded.into();
        let mut rows = opening();
        rows.push(ev("feedback/record", json!({"text":"unchanged"})));
        if seeded {
            rows.push(ev("session/end-seed", json!({"inherited":true})));
        }
        let rows = numbered(rows, 1);
        let before = (header.clone(), rows.clone());
        let a = restore(&rows, &header, SessionFormatValidation::Current).unwrap();
        assert_eq!(
            a.header,
            obj(
                json!({"version":4,"id":"v2-identity","createdAt":1,"isSeeded":seeded,"delegationDepth":0})
            )
        );
        assert_eq!(a.inherited_event_count, if seeded { 4 } else { 0 });
        let mut expected = rows[..2].to_vec();
        expected.push(obj(json!({"type":"system/message","seq":2,"time":2,"surfaceOp":"append","data":{"turn":1,"step":1,"message":{"id":"v2-to-v3-system-9673c4ed630de6c21ea6bd6b573094ea8e5e216843a1b572a68657499ad9667b","role":"system","source":{"kind":"system-prompt"},"content":[]}}})));
        for mut row in rows[2..].iter().cloned() {
            row.insert("seq".into(), (row["seq"].as_u64().unwrap() + 1).into());
            expected.push(row);
        }
        assert_eq!(a.events, expected);
        assert_eq!((header, rows), before);
    }
}
#[test]
fn refuses_unclassified_ignorable_v2_events() {
    for validation in [
        SessionFormatValidation::Current,
        SessionFormatValidation::Transformed,
    ] {
        let header = physical(2, "v2-unknown");
        let mut row = ev("external/event", json!({"extra":["unchanged"]}));
        row.insert("ignorable".into(), true.into());
        let before = (header.clone(), row.clone());
        let mut r = create_session_format_catalog_with_children(vec![])
            .unwrap()
            .create_restore(&header, policy(validation))
            .unwrap();
        err(
            r.decode_row(row.clone().into()),
            "cannot safely transform unclassified event external/event",
        );
        assert_eq!((header, row), before);
    }
}
fn ptc_source() -> Vec<SessionFormatEvent> {
    let m = |id: &str| json!({"id":id,"role":"user","source":{"kind":"plugin","plugin":"tools-code-mode"},"content":[{"type":"text","text":"tool/code-dispatch-start tools-code-mode tools-ptc"}]});
    let dispatch = json!({"rootCallId":"tools-code-mode:root","parentCallId":"tools-code-mode:root","subCallId":"tools-code-mode:child","name":"read","arguments":{"text":"tools-code-mode","nested":{"type":"tool/code-dispatch"}}});
    let mut settled = dispatch.clone();
    settled["isError"] = false.into();
    settled["content"] = json!([{"type":"text","text":"tools-code-mode:child"}]);
    let mut replacement = surface("user/message", m("tools-code-mode:replacement"));
    replacement.insert("sourceEventSeqs".into(), json!([2, 3]));
    replacement.insert(
        "surfaceOp".into(),
        json!({"op":"replace","start":2,"end":3}),
    );
    let mut rows = opening();
    rows.extend([surface("user/message",m("tools-code-mode:message")),surface("user/message",m("tools-ptc:message")),ev("agent/inbox/spliced",json!({"target":"next-turn","start":0,"inserted":[m("tools-code-mode:message"),m("tools-ptc:message")]})),ev("tool/code-dispatch-start",dispatch),ev("tool/code-dispatch",settled),replacement]);
    rows.extend(closing());
    for (seq, row) in rows.iter_mut().enumerate() {
        row.insert("seq".into(), seq.into());
        row.insert(
            "time".into(),
            match seq {
                0 | 1 => 10,
                2 => 11,
                3 => 12,
                4 => 13,
                5 => 14,
                6 => 15,
                7 => 16,
                _ => 17,
            }
            .into(),
        );
    }
    rows
}
#[test]
fn migrates_frozen_ptc_records_and_reopens_the_actual_current_representation_without_rewriting_ids()
{
    for version in [0, 1, 2] {
        let mut header = json!({"type":"session","version":version,"id":"tools-code-mode:session","createdAt":1,"delegationDepth":0});
        if version == 2 {
            header["isSeeded"] = false.into();
        }
        let rows = ptc_source();
        let before = (header.clone(), rows.clone());
        let a = restore(&rows, &header, SessionFormatValidation::Current).unwrap();
        let mut expected = rows[..2].to_vec();
        let system = a.events[2].clone();
        assert_eq!(system["type"], "system/message");
        assert_eq!(system["seq"], 2);
        expected.push(system);
        for (index, row) in rows[2..].iter().enumerate() {
            let mut target = row.clone();
            target.insert("seq".into(), (index + 3).into());
            match index + 2 {
                2 | 3 | 7 => {
                    target["data"]["source"] = json!({"kind":"ptc-mode"});
                }
                4 => {
                    for message in target["data"]["inserted"].as_array_mut().unwrap() {
                        message["source"] = json!({"kind":"ptc-mode"});
                    }
                }
                5 => {
                    target.insert("type".into(), "tool/ptc-dispatch-start".into());
                }
                6 => {
                    target.insert("type".into(), "tool/ptc-dispatch".into());
                }
                _ => {}
            }
            if index + 2 == 7 {
                target.insert("sourceEventSeqs".into(), json!([3, 4]));
                target.insert(
                    "surfaceOp".into(),
                    json!({"op":"replace","startSeq":3,"endSeq":4}),
                );
            }
            expected.push(target);
        }
        assert_eq!(a.header, h(4, "tools-code-mode:session"));
        assert_eq!(a.inherited_event_count, 0);
        assert_eq!(a.events, expected);
        assert_eq!(reopen(&a).unwrap(), a);
        assert_eq!((header, rows), before);
    }
}
#[test]
fn rejects_obsolete_required_tags_in_v3_input_and_retains_ignorable_tags() {
    for validation in [
        SessionFormatValidation::Current,
        SessionFormatValidation::Transformed,
    ] {
        for tag in ["tool/code-dispatch-start", "tool/code-dispatch"] {
            let header = physical(3, "native-ptc");
            let row = ev(tag, Value::Null);
            err(
                restore(std::slice::from_ref(&row), &header, validation),
                "unknown event type",
            );
            let mut ignorable = row;
            ignorable.insert("ignorable".into(), true.into());
            ignorable.insert("data".into(),json!({"text":"tools-code-mode","source":{"kind":"plugin","plugin":"tools-code-mode"}}));
            let mut expected = ignorable.clone();
            expected.insert("type".into(), format!("plugin:{tag}").into());
            assert_eq!(
                restore(&[ignorable], &header, validation).unwrap().events,
                vec![expected]
            );
        }
    }
}
#[test]
fn refuses_v3_dispatch_tag_collisions_in_v2_input() {
    for validation in [
        SessionFormatValidation::Current,
        SessionFormatValidation::Transformed,
    ] {
        for tag in ["tool/ptc-dispatch-start", "tool/ptc-dispatch"] {
            for ignorable in [false, true] {
                let mut row = ev(tag, Value::Null);
                if ignorable {
                    row.insert("ignorable".into(), true.into());
                }
                let error = restore(&[row], &physical(2, "v2-collision"), validation)
                    .unwrap_err()
                    .to_string();
                assert!(
                    error.contains("format v2") && error.contains("ptc-dispatch"),
                    "{error}"
                );
            }
        }
    }
}
#[test]
fn renames_v2_owned_ignorable_dispatch_tags() {
    for validation in [
        SessionFormatValidation::Current,
        SessionFormatValidation::Transformed,
    ] {
        let data = json!({"rootCallId":"root","parentCallId":"root","subCallId":"child","name":"read","arguments":{}});
        let mut settled = data.clone();
        settled["isError"] = false.into();
        settled["content"] = json!([]);
        let mut start = ev("tool/code-dispatch-start", data);
        start.insert("ignorable".into(), true.into());
        let mut end = ev("tool/code-dispatch", settled);
        end.insert("ignorable".into(), true.into());
        let rows = numbered(vec![opening()[0].clone(), start, end], 1);
        let mut expected = rows.clone();
        expected[1].insert("type".into(), "tool/ptc-dispatch-start".into());
        expected[2].insert("type".into(), "tool/ptc-dispatch".into());
        assert_eq!(
            restore(&rows, &physical(2, "v2-owned"), validation)
                .unwrap()
                .events,
            expected
        );
    }
}
#[test]
fn refuses_a_v3_delivery_marker_in_v2_input() {
    for validation in [
        SessionFormatValidation::Current,
        SessionFormatValidation::Transformed,
    ] {
        let rows = vec![
            ev("feedback/record", json!({"text":"unaccepted"})),
            e(
                "session-log-deepseek/delivery-accepted",
                1,
                json!({"sessionId":"future-delivery","throughSeq":0,"sessionFormatVersion":3}),
            ),
        ];
        err(
            restore(&rows, &physical(2, "future-delivery"), validation),
            "format v2 delivery marker claims target format v3",
        );
    }
}
#[test]
fn finishes_a_streaming_migration_with_the_target_header_and_dense_rows() {
    let header =
        json!({"type":"session","version":1,"id":"streaming","createdAt":1,"delegationDepth":0});
    let mut row = ev("feedback/record", json!({"text":"retained"}));
    row.insert("time".into(), 2.into());
    let a = restore(&[row], &header, SessionFormatValidation::Current).unwrap();
    assert_eq!(a.header["version"], 4);
    assert_eq!(a.header["id"], "streaming");
    assert_eq!(a.inherited_event_count, 0);
    assert_eq!(a.events[0]["type"], "feedback/record");
    assert_eq!(a.events[0]["seq"], 0);
}
#[test]
fn uses_the_v3_event_vocabulary_for_ignorable_events_whose_names_become_known_later() {
    let rows = vec![
        ev("feedback/record", json!({"text":"saved"})),
        obj(
            json!({"type":"developer/message","seq":1,"time":2,"ignorable":true,"surfaceOp":"append","sourceEventSeqs":[0],"data":{"content":[{"type":"tool-result","toolCallId":"opaque","content":[]}],"vendor":true}}),
        ),
    ];
    let header = physical(3, "old-extension");
    let mut reader = historical_session_format_catalog()
        .unwrap()
        .create_restore(&header, policy(SessionFormatValidation::Current))
        .unwrap();
    for row in &rows {
        reader.decode_row(row.clone().into()).unwrap();
    }
    assert_eq!(reader.finish().unwrap().events, rows);
    let mut expected = rows.clone();
    expected[1].insert("type".into(), "plugin:developer/message".into());
    assert_eq!(
        restore(&rows, &header, SessionFormatValidation::Current)
            .unwrap()
            .events,
        expected
    );
}
#[test]
fn restores_v3_and_preceding_versions_without_the_incoming_catalog_completion_edge() {
    let header = physical(2, "historical");
    let c = historical_session_format_catalog().unwrap();
    assert_eq!(c.read_header(&header).status(), "migration-required");
    for validation in [
        SessionFormatValidation::Current,
        SessionFormatValidation::Transformed,
    ] {
        let a = c
            .create_restore(&header, policy(validation))
            .unwrap()
            .finish()
            .unwrap();
        assert_eq!(a.header["version"], 3);
        assert_eq!(
            c.create_restore(&physical(3, "historical"), policy(validation))
                .unwrap()
                .finish()
                .unwrap(),
            a
        );
    }
}
