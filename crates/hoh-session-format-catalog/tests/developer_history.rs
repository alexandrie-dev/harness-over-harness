mod common;
use common::*;
use hoh_session_format_catalog::generated::session_format_catalog_options;
use serde_json::Map;
#[test]
fn defers_ignorable_developer_interpretation_until_catalog_completion() {
    for knows_developer in [false, true] {
        let header = h(4, "opaque-developer");
        let mut row = ev(
            "developer/message",
            json!({"message":{"role":"developer","source":{"kind":"plugin"}},"future":true}),
        );
        row.insert("ignorable".into(), true.into());
        row.insert("surfaceOp".into(), json!({"future":true}));
        let mut events = vec![row];
        events.extend(opening());
        let events = numbered(events, 1);
        let mut known = known(&events);
        if !knows_developer {
            known.remove("developer/message");
        }
        let mut options = session_format_catalog_options();
        options.restore_current =
            Arc::new(move |candidate| restore_released_v4_artifact(candidate, &known));
        let c = create_session_format_catalog(options).unwrap();
        let physical = c.encode_current_header(&header, 0).unwrap();
        let mut reader = c
            .create_restore(&physical.into(), policy(SessionFormatValidation::Current))
            .unwrap();
        for event in &events {
            reader.decode_row(event.clone().into()).unwrap();
        }
        if knows_developer {
            assert!(reader.finish().is_err());
        } else {
            assert_eq!(reader.finish().unwrap(), artifact(events, header, 0));
        }
    }
}
fn tool_states(session: &Session) -> Vec<Option<Value>> {
    let events = session.snapshot_events(None, None);
    let initial = events
        .iter()
        .find(|e| e.event_type() == "request/header")
        .unwrap();
    let mut active = Map::new();
    for tool in initial.data()["header"]["tools"].as_array().unwrap() {
        active.insert(tool["name"].as_str().unwrap().into(), tool.clone());
    }
    let mut states = vec![active.get("search").cloned()];
    for seq in session.surface().unwrap().nodes().unwrap() {
        let event = &events[seq.get() as usize];
        if event.event_type() != "developer/message" {
            continue;
        }
        for block in event.data()["message"]["content"].as_array().unwrap() {
            let name = block["toolName"].as_str().unwrap();
            match block["type"].as_str() {
                Some("tool-addition") => {
                    let seq = event.data()["headerSeq"].as_u64().unwrap() as usize;
                    let tool = events[seq].data()["header"]["tools"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|t| t["name"] == name)
                        .unwrap();
                    active.insert(name.into(), tool.clone());
                }
                Some("tool-removal") => {
                    active.shift_remove(name);
                }
                _ => continue,
            }
            states.push(active.get("search").cloned());
        }
    }
    states
}
fn add(
    session: &Session,
    turn: u64,
    tool: Value,
    id: &str,
) -> hoh_session::Result<Arc<SessionEvent>> {
    let header_seq=append(session,"request/header",json!({"reason":"change","header":{"config":{"provider":"test","model":"test"},"tools":[tool]}}),false)?.seq();
    append(
        session,
        "developer/message",
        json!({"turn":turn,"step":1,"headerSeq":header_seq,"message":{"id":id,"role":"developer","source":{"kind":"tool-registry"},"content":[{"type":"tool-addition","toolName":tool["name"]}]}}),
        true,
    )
}
fn restore_session(session: &Session) -> Session {
    let header = obj((*session.header()).clone());
    let events = session
        .snapshot_events(None, None)
        .iter()
        .map(|e| obj(e.as_value().clone()))
        .collect();
    let a = artifact(events, header, session.inherited_event_count().get());
    let a = reopen(&a).unwrap();
    detached(&a, false).unwrap()
}
#[test]
fn reconstructs_remove_re_add_and_replacement_from_historical_headers_after_restart_and_fork() {
    let parent = Session::create(
        "tool-history".into(),
        None,
        Some(h(4, "tool-history").into()),
        None,
        None,
    )
    .unwrap();
    let mut initial = json!({"name":"search","description":"Initial","parameters":{}});
    let mut first = json!({"name":"search","description":"First","parameters":{"properties":{"query":{"type":"string"}}},"deferLoading":true});
    let mut second = json!({"name":"search","description":"Second","parameters":{"properties":{"limit":{"type":"number"}}}});
    let third = json!({"name":"search","description":"Third","parameters":{"properties":{"exact":{"type":"boolean"}}}});
    let expected = vec![
        Some(initial.clone()),
        Some(first.clone()),
        None,
        Some(second.clone()),
        Some(third.clone()),
    ];
    append(&parent, "turn/start", json!({"turn":1}), false).unwrap();
    append(&parent, "step/start", json!({"turn":1,"step":1}), false).unwrap();
    append(&parent,"request/header",json!({"reason":"initial","header":{"config":{"provider":"test","model":"test"},"tools":[initial]}}),false).unwrap();
    add(&parent, 1, first.clone(), "first").unwrap();
    append(&parent,"developer/message",json!({"turn":1,"step":1,"message":{"id":"remove","role":"developer","source":{"kind":"tool-registry"},"content":[{"type":"tool-removal","toolName":"search"}]}}),true).unwrap();
    add(&parent, 1, second.clone(), "second").unwrap();
    append(&parent, "step/end", json!({"turn":1,"step":1}), false).unwrap();
    let boundary = append(
        &parent,
        "turn/end",
        json!({"turn":1,"reason":{"kind":"completed"}}),
        false,
    )
    .unwrap()
    .seq();
    append(&parent, "turn/start", json!({"turn":2}), false).unwrap();
    append(&parent, "step/start", json!({"turn":2,"step":1}), false).unwrap();
    add(&parent, 2, third, "third").unwrap();
    append(&parent, "step/end", json!({"turn":2,"step":1}), false).unwrap();
    append(
        &parent,
        "turn/end",
        json!({"turn":2,"reason":{"kind":"completed"}}),
        false,
    )
    .unwrap();
    initial["description"] = "mutated initial definition".into();
    first["description"] = "mutated producer definition".into();
    first["parameters"]["properties"] = json!({});
    second["description"] = "mutated replacement".into();
    assert_eq!(tool_states(&parent), expected);
    assert_eq!(tool_states(&restore_session(&parent)), expected);
    let seed = hoh_session::build_fork_seed(&parent.snapshot_events(None, None), boundary).unwrap();
    let mut header = h(4, "tool-history-fork");
    header.insert("parentSession".into(), "tool-history".into());
    header.insert("isSeeded".into(), true.into());
    let child = Session::create(
        "tool-history-fork".into(),
        Some(&seed),
        Some(header.into()),
        Some(SessionLogOffset::new(boundary.get() + 1).unwrap()),
        None,
    )
    .unwrap();
    assert_eq!(tool_states(&restore_session(&child)), expected[..4]);
}
