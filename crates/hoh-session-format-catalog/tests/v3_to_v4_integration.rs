//! Source edge cases requiring the full catalog/Session live here to keep the
//! edge's manifest within its permitted production-only local dependencies.
mod common;
use common::*;
use hoh_session::SessionSeq;
#[test]
fn derives_checkpoint_and_capture_after_all_restart_references_are_remapped() {
    let (rows, _) = complex_restart_source();
    let before = rows.clone();
    let a = restore(
        &rows,
        &physical(3, "restart"),
        SessionFormatValidation::Current,
    )
    .unwrap();
    let session = detached(&a, false).unwrap();
    assert_eq!(
        session
            .derive_messages()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        vec!["checkpoint", "capture"]
    );
    assert_eq!(rows, before);
    assert_eq!(reopen(&a).unwrap(), a);
}
#[test]
fn rejects_noncanonical_native_metadata() {
    let mut physical = obj(json!({"type":"session"}));
    physical.extend(canonical_header());
    let a = restore(
        &canonical_source(),
        &physical.into(),
        SessionFormatValidation::Current,
    )
    .unwrap();
    let index = a
        .events
        .iter()
        .position(|e| e["type"] == "system/message")
        .unwrap();
    for override_ in [
        json!({"sourceEventSeqs":[]}),
        json!({"surfaceOp":{"op":"replace","start":0,"end":0}}),
    ] {
        let mut invalid = a.clone();
        invalid.events[index].extend(obj(override_));
        assert!(reopen(&invalid).is_err());
    }
}
#[test]
fn restores_a_seeded_chain_including_upstream_cardinality_changes_and_reopens_v4() {
    for version in [0, 1, 2, 3] {
        let mut req = request();
        req["data"]["header"]["system"] = "seed prompt".into();
        let mut rows = opening();
        rows.push(user("user"));
        rows.push(req);
        rows.extend(closing());
        rows.push(ev(
            "session/end-seed",
            if version < 2 {
                json!({})
            } else {
                json!({"inherited":true})
            },
        ));
        let mut rows = numbered(rows, 1);
        if version == 3 {
            let mut header = physical(2, "identity");
            header["isSeeded"] = true.into();
            let mut r = historical_session_format_catalog()
                .unwrap()
                .create_restore(&header, policy(SessionFormatValidation::Current))
                .unwrap();
            for row in &rows {
                r.decode_row(row.clone().into()).unwrap();
            }
            rows = r.finish().unwrap().events;
        }
        let header = if version < 2 {
            json!({"type":"session","version":version,"id":"identity","createdAt":1,"delegationDepth":0,"parentSession":"ancestor","seedLength":6})
        } else {
            let mut h = physical(version, "identity");
            h["isSeeded"] = true.into();
            h["parentSession"] = "ancestor".into();
            h
        };
        let before = (header.clone(), rows.clone());
        let c = create_session_format_catalog_with_children(vec![]).unwrap();
        let mut r = c
            .create_restore(&header, policy(SessionFormatValidation::Current))
            .unwrap();
        for row in &rows {
            let value = if version == 3 {
                RELEASED_V3_SESSION_FORMAT_CODEC.encode_event(row).unwrap()
            } else {
                row.clone()
            };
            r.decode_row(value.into()).unwrap();
        }
        let a = r.finish().unwrap();
        assert_eq!(a.header["version"], 4);
        assert_eq!(a.inherited_event_count, 8);
        assert_eq!(
            a.events
                .iter()
                .filter(|e| e["type"] == "system/message")
                .count(),
            2
        );
        assert_eq!(a.events.last().unwrap()["seq"], 8);
        assert_eq!(reopen(&a).unwrap(), a);
        assert_eq!((header, rows), before);
    }
}
#[test]
fn preserves_user_and_developer_attribution_through_installed_session_derivation() {
    for role in ["user", "developer"] {
        let message = json!({"id":"attributed-message","role":role,"source":{"kind":"external-attribution","location":{"file":"notes.txt","lines":[2,5]},"enabled":false},"content":[{"type":"text","text":"Retain this context."}]});
        let mut rows = opening();
        rows.push(surface(
            &format!("{role}/message"),
            if role == "user" {
                message.clone()
            } else {
                json!({"turn":1,"step":1,"message":message})
            },
        ));
        rows.extend(closing());
        let rows = numbered(rows, 1);
        let a = restore(
            &rows,
            &physical(4, "unknown-attribution"),
            SessionFormatValidation::Current,
        )
        .unwrap();
        let s = detached(&a, false).unwrap();
        assert_eq!(
            s.derive_messages()
                .unwrap()
                .iter()
                .map(|m| (**m).clone())
                .collect::<Vec<_>>(),
            vec![message]
        );
        assert_eq!(a.events, rows);
    }
}
#[test]
fn retains_an_empty_developer_position_while_replacing_the_protected_system_head() {
    let mut developer = surface(
        "developer/message",
        json!({"turn":1,"step":1,"message":{"id":"developer","role":"developer","source":{"kind":"tool-registry"},"content":[]}}),
    );
    developer.insert("seq".into(), 3.into());
    let mut replacement = system();
    replacement.insert(
        "surfaceOp".into(),
        json!({"op":"replace","startSeq":2,"endSeq":2}),
    );
    replacement.insert("sourceEventSeqs".into(), json!([2]));
    replacement["data"]["message"]["id"] = "replacement".into();
    replacement["data"]["message"]["content"] = json!([{"type":"text","text":"Updated"}]);
    let mut rows = opening();
    rows.extend([system(), developer.clone(), replacement]);
    let rows = numbered(rows, 1);
    let a = restore(
        &rows,
        &physical(4, "developer-relations"),
        SessionFormatValidation::Current,
    )
    .unwrap();
    let s = detached(&a, false).unwrap();
    assert_eq!(
        s.surface()
            .unwrap()
            .nodes()
            .unwrap()
            .iter()
            .map(|n| n.get())
            .collect::<Vec<_>>(),
        vec![4, 3]
    );
    assert_eq!(
        s.derive_messages()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        vec!["replacement"]
    );
    assert_eq!(a.events[3]["data"], developer["data"]);
}
#[test]
fn remaps_offload_targets_and_derives_the_selected_image_after_restart() {
    let mut rows = opening();
    rows.push(ev("step/end", json!({"turn":1,"step":1})));
    rows.push(ev(
        "agent/inbox/spliced",
        json!({"target":"next-turn","inserted":[user_data("next")]}),
    ));
    rows.push(ev("turn/start", json!({"turn":2})));
    let image = json!({"type":"image","attachment":{"attachmentId":"image","mediaType":"image/png","bytes":1,"width":1,"height":1}});
    let mut picture = user("picture");
    picture["data"]["content"] = json!([image]);
    rows.push(picture);
    rows.push(ev(
        "image/offload",
        json!({"targets":[{"seq":5,"imageIndexes":[0]}]}),
    ));
    rows.push(ev(
        "turn/end",
        json!({"turn":2,"reason":{"kind":"completed"}}),
    ));
    let a = restore(
        &numbered(rows, 10),
        &physical(3, "restart"),
        SessionFormatValidation::Current,
    )
    .unwrap();
    assert_eq!(
        a.events[7]["data"]["targets"],
        json!([{"seq":6,"imageIndexes":[0]}])
    );
    let s = detached(&a, true).unwrap();
    let messages = s.derive_messages().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["id"], "picture");
    assert_eq!(messages[0]["content"][0]["attachment"], image["attachment"]);
    assert_eq!(messages[0]["content"][0]["offloaded"], true);
    assert_eq!(reopen(&a).unwrap(), a);
}
#[test]
fn builds_real_fork_seed_and_preserves_original_fork_identity_through_catalog() {
    let fixture = fork_artifact();
    let parent_artifact = artifact(fixture.events[..3].to_vec(), h(4, "fork-parent"), 0);
    let parent = detached(&parent_artifact, false).unwrap();
    let seed = hoh_session::build_fork_seed(
        &parent.snapshot_events(None, None),
        SessionSeq::new(2).unwrap(),
    )
    .unwrap();
    let child = Session::create(
        "fork-child".into(),
        Some(&seed),
        Some(fixture.header.clone().into()),
        Some(SessionLogOffset::new(3).unwrap()),
        None,
    )
    .unwrap();
    let events = child
        .snapshot_events(None, None)
        .iter()
        .map(|e| obj(e.as_value().clone()))
        .collect();
    let actual = artifact(
        events,
        obj((*child.header()).clone()),
        child.inherited_event_count().get(),
    );
    assert_eq!(actual, fixture);
    let original: Vec<_> = actual
        .events
        .iter()
        .map(|e| hoh_util_values::js_json_stringify(&Value::Object(e.clone())))
        .collect();
    let reopened = reopen(&actual).unwrap();
    assert_eq!(reopened, actual);
    assert_eq!(
        reopened
            .events
            .iter()
            .map(|e| hoh_util_values::js_json_stringify(&Value::Object(e.clone())))
            .collect::<Vec<_>>(),
        original
    );
    let nested_seed = hoh_session::build_fork_seed(
        &child.snapshot_events(None, None),
        SessionSeq::new(4).unwrap(),
    )
    .unwrap();
    let mut header = h(4, "fork-grandchild");
    header.insert("isSeeded".into(), true.into());
    header.insert("parentSession".into(), "fork-child".into());
    let grandchild = Session::create(
        "fork-grandchild".into(),
        Some(&nested_seed),
        Some(header.into()),
        Some(SessionLogOffset::new(5).unwrap()),
        None,
    )
    .unwrap();
    let events = grandchild
        .snapshot_events(None, None)
        .iter()
        .map(|e| obj(e.as_value().clone()))
        .collect();
    let a = artifact(events, obj((*grandchild.header()).clone()), 5);
    assert_eq!(reopen(&a).unwrap(), a);
}
#[test]
fn restores_all_open_tool_exchange_prefixes_with_installed_session() {
    let mut rows = opening();
    rows.push(user("input"));
    rows.push(request());
    rows.push(assistant(json!([call_block()])));
    rows.push(ev(
        "tool/call",
        json!({"turn":1,"step":1,"callId":"call","name":"read","arguments":"{}"}),
    ));
    rows.push(result(native_message("call", "done", None)));
    rows.extend(closing());
    let rows = numbered(rows, 1);
    let a = restore(
        &rows,
        &physical(4, "native-relations"),
        SessionFormatValidation::Current,
    )
    .unwrap();
    assert_eq!(
        detached(&a, false)
            .unwrap()
            .derive_messages()
            .unwrap()
            .iter()
            .map(|m| m["role"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        vec!["user", "assistant", "tool"]
    );
    for length in 0..rows.len() {
        restore(
            &rows[..length],
            &physical(4, "native-relations"),
            SessionFormatValidation::Current,
        )
        .unwrap();
    }
}
