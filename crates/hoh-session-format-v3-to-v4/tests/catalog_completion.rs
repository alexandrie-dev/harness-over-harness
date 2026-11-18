mod common;
use common::*;
use hoh_session_format_v3_to_v4::facts::*;
fn child() -> Value {
    json!({"version":0,"childId":"child","childCreatedAt":2,"mode":"continuable","label":"child task"})
}
fn evidence(value: Value) -> R<Value> {
    if value.get("version").is_none() {
        return Ok(value);
    }
    let fact = catalog_fact(&value)?;
    let mut descriptor = json!({"version":3,"provider":"spawn","mode":fact["mode"]});
    if let Some(label) = fact.get("label") {
        descriptor["label"] = label.clone();
    }
    Ok(
        json!({"childId":fact["childId"],"childCreatedAt":fact["childCreatedAt"],"descriptorCount":1,"descriptor":descriptor}),
    )
}
fn run(
    events: Vec<SessionFormatEvent>,
    facts: Vec<Value>,
    seeded: bool,
    cut: Option<u64>,
    as_run: bool,
) -> R<SessionFormatArtifact> {
    let facts = facts.into_iter().map(evidence).collect::<R<Vec<_>>>()?;
    let mut header = h(3, "parent");
    header.insert("isSeeded".into(), seeded.into());
    if seeded {
        header.insert("parentSession".into(), "ancestor".into());
    }
    let mut s = stage(&header, cut.or(if seeded { None } else { Some(0) }), facts)?;
    let mut out = SessionFormatEventCollector::new();
    if as_run {
        s.transform_run(Box::new(Run(events)), &mut out)?;
    } else {
        for event in events {
            s.transform_event(event, &mut out)?;
        }
    }
    let cut = s.finish(&mut out)?;
    header.insert("version".into(), 4.into());
    Ok(artifact(out.values, header, cut))
}
#[test]
fn interprets_v1_descriptors_and_includes_an_evidence_source_path_in_diagnostics() {
    assert_eq!(child_catalog_fact(&obj(json!({"childId":"child","childCreatedAt":2,"descriptorCount":1,"descriptor":{"version":1,"provider":"spawn","label":"legacy child"}}))).unwrap(),Some(obj(json!({"version":0,"childId":"child","childCreatedAt":2,"mode":"continuable","label":"legacy child"}))));
    assert_eq!(
        child_catalog_subject(&obj(
            json!({"childId":"child","sourcePath":"/tmp/session.v3.jsonl"})
        )),
        "Session child (raw log: /tmp/session.v3.jsonl)"
    );
}
#[test]
fn appends_deterministic_child_facts_without_changing_source_events() {
    let mut row = ev("feedback/record", json!({"text":"retained"}));
    row.insert("time".into(), 10.into());
    let source = vec![row];
    let result = run(source.clone(), vec![child()], false, None, false).unwrap();
    assert_eq!(result.events[0], source[0]);
    assert_eq!(
        result.events[1],
        obj(json!({"type":"subagent/catalog","seq":1,"time":10,"data":child()}))
    );
    assert_eq!(
        run(source.clone(), vec![child()], false, None, false).unwrap(),
        result
    );
    assert_eq!(source.len(), 1);
}
#[test]
fn preserves_existing_facts_and_adds_only_missing_children() {
    let source = vec![ev("subagent/catalog", child())];
    assert_eq!(
        run(source.clone(), vec![child(), child()], false, None, false)
            .unwrap()
            .events,
        source
    );
    let mut second = child();
    second["childId"] = "second".into();
    assert_eq!(
        run(source, vec![child(), second], false, None, false)
            .unwrap()
            .events
            .len(),
        2
    );
}
#[test]
fn does_not_treat_inherited_catalog_membership_as_owned() {
    let source = vec![
        ev("subagent/catalog", child()),
        e("session/end-seed", 1, json!({"inherited":true})),
    ];
    let result = run(source, vec![child()], true, None, false).unwrap();
    assert_eq!(result.inherited_event_count, 1);
    assert_eq!(result.events[2]["type"], "subagent/catalog");
    assert_eq!(result.events[2]["seq"], 2);
    assert_eq!(result.events[2]["data"], child());
}
#[test]
fn validates_catalog_payloads_only_after_the_final_inherited_seed_marker() {
    let opaque = ev("subagent/catalog", json!({"version":99}));
    let mut seed = e("session/end-seed", 1, json!({"inherited":true}));
    seed.insert("time".into(), 2.into());
    let mut source = vec![opaque.clone(), seed.clone(), opaque.clone(), seed];
    source = dense(source);
    let mut expected = source.clone();
    expected.push(obj(
        json!({"type":"subagent/catalog","seq":4,"time":2,"data":child()}),
    ));
    assert_eq!(
        run(source.clone(), vec![child()], true, None, false)
            .unwrap()
            .events,
        expected
    );
    let mut late = opaque.clone();
    late.insert("seq".into(), 4.into());
    source.push(late);
    err(
        run(source, vec![], true, None, false),
        "supported versioned",
    );
    err(
        run(vec![opaque], vec![], false, None, false),
        "supported versioned",
    );
}
#[test]
fn requires_explicit_corpus_facts_and_refuses_conflicting_identities() {
    let header = h(3, "parent");
    let target = h(4, "parent");
    err(
        SESSION_FORMAT_V3_TO_V4.create_stage(SessionFormatMigrationStageInput {
            source_header: &header,
            target_header: &target,
            source_kind: SessionFormatSourceKind::Decoded,
            source_inherited_event_count: Some(0),
        }),
        "explicit historical child facts",
    );
    let mut invalid = child();
    invalid["mode"] = "invalid".into();
    err(
        run(vec![], vec![invalid], false, None, false),
        "supported versioned",
    );
    let mut conflict = child();
    conflict["childCreatedAt"] = 3.into();
    err(
        run(
            vec![ev("subagent/catalog", child())],
            vec![conflict],
            false,
            None,
            false,
        ),
        "conflicts",
    );
}
#[test]
fn extracts_the_own_descriptor_after_a_fork_prefix() {
    let mut header = h(3, "child");
    header.insert("createdAt".into(), 2.into());
    header.insert("origin".into(), "subagent".into());
    header.insert("parentSession".into(), "parent".into());
    header.insert("isSeeded".into(), true.into());
    let mut a = artifact(
        vec![
            ev(
                "subagent/descriptor",
                json!({"version":3,"mode":"one-shot","provider":"fork"}),
            ),
            e(
                "subagent/descriptor",
                1,
                json!({"version":3,"mode":"continuable","provider":"fork","label":"child task"}),
            ),
        ],
        header,
        1,
    );
    assert_eq!(
        child_catalog_fact(&historical_child_catalog_source(&a).unwrap()).unwrap(),
        Some(obj(child()))
    );
    a.inherited_event_count = 0;
    assert_eq!(
        run(
            vec![],
            vec![historical_child_catalog_source(&a).unwrap().into()],
            false,
            None,
            false
        )
        .unwrap()
        .events[0]["data"]["mode"],
        "unknown"
    );
    a.events.clear();
    assert_eq!(
        run(
            vec![],
            vec![historical_child_catalog_source(&a).unwrap().into()],
            false,
            None,
            false
        )
        .unwrap()
        .events[0]["data"]["mode"],
        "unknown"
    );
}
#[test]
fn orders_tied_and_differently_dated_children_independently_of_directory_enumeration() {
    let mut children = vec![child(), child(), child()];
    children[0]["childId"] = "z".into();
    children[1]["childId"] = "a".into();
    children[2]["childId"] = "first".into();
    children[2]["childCreatedAt"] = 1.into();
    let out = run(vec![], children.clone(), false, None, false).unwrap();
    assert_eq!(
        out.events
            .iter()
            .map(|e| e["data"].clone())
            .collect::<Vec<_>>(),
        vec![
            children[2].clone(),
            children[1].clone(),
            children[0].clone()
        ]
    );
    assert_eq!(out.events[0]["time"], 1);
}
#[test]
fn rejects_duplicates_sparse_events_and_missing_or_inconsistent_seed_cuts() {
    let c = ev("subagent/catalog", child());
    err(
        run(
            dense(vec![c.clone(), c.clone()]),
            vec![],
            false,
            None,
            false,
        ),
        "duplicate",
    );
    let mut bad = c.clone();
    bad.insert("seq".into(), 1.into());
    err(run(vec![bad], vec![], false, None, false), "dense");
    err(
        run(vec![], vec![], true, None, false),
        "inherited event count",
    );
    err(run(vec![], vec![], false, Some(1), false), "disagrees");
    assert_eq!(
        run(vec![c.clone()], vec![], false, None, true)
            .unwrap()
            .events,
        vec![c]
    );
}
#[test]
fn refuses_target_delivery_activation_and_invalid_current_delivery_ownership() {
    let prior = ev("feedback/record", json!({"text":"prior"}));
    for (version, message) in [(4, "claims target format v4"), (3, "wrong Session")] {
        err(
            run(
                vec![
                    prior.clone(),
                    e(
                        "session-log-deepseek/delivery-accepted",
                        1,
                        json!({"sessionId":"other","throughSeq":0,"sessionFormatVersion":version}),
                    ),
                ],
                vec![],
                false,
                None,
                false,
            ),
            message,
        );
    }
}
#[test]
fn retains_unknown_children_while_validating_available_descriptor_fields() {
    let mut header = h(3, "parent");
    header.insert("origin".into(), "subagent".into());
    header.insert("parentSession".into(), "parent".into());
    let mut a = artifact(vec![], header, 0);
    let mut bad = a.clone();
    bad.header = h(3, "parent");
    err(historical_child_catalog_source(&bad), "direct parent");
    for data in [
        Value::Null,
        json!({}),
        json!({"version":4,"mode":"one-shot","provider":"spawn"}),
    ] {
        a.events = vec![ev("subagent/descriptor", data)];
        let out = run(
            vec![],
            vec![historical_child_catalog_source(&a).unwrap().into()],
            false,
            None,
            false,
        )
        .unwrap();
        assert_eq!(out.events[0]["data"]["childId"], "parent");
        assert_eq!(out.events[0]["data"]["mode"], "unknown");
    }
    let mut invalids = vec![Value::Null];
    for (key, value) in [
        ("version", json!(2)),
        ("childId", Value::Null),
        ("label", Value::Null),
        ("childCreatedAt", json!(-1)),
    ] {
        let mut c = child();
        c[key] = value;
        invalids.push(c);
    }
    for c in invalids {
        assert!(run(vec![], vec![c], false, None, false).is_err());
    }
    assert_eq!(
        run(
            vec![],
            vec![json!({"version":0,"childId":"one-shot","childCreatedAt":2,"mode":"one-shot"})],
            false,
            None,
            false
        )
        .unwrap()
        .events
        .len(),
        1
    );
}
#[test]
fn retains_existing_catalog_extensions_without_interpreting_unavailable_descriptors() {
    for descriptor in [Value::Null, json!({"version":99})] {
        let mut data = child();
        data["plugin"] = "retained".into();
        let source = vec![ev("subagent/catalog", data)];
        assert_eq!(run(source.clone(),vec![json!({"childId":"child","childCreatedAt":2,"descriptorCount":if descriptor.is_null(){0}else{1},"descriptor":descriptor})],false,None,false).unwrap().events,source);
    }
}
#[test]
fn keeps_parent_facts_authoritative_when_a_child_has_multiple_own_descriptors() {
    let mut c = child();
    c["extension"] = json!({"retained":true});
    let source = vec![ev("subagent/catalog", c)];
    let mut evidence = json!({"childId":"child","childCreatedAt":2,"descriptorCount":2,"descriptor":{"version":3,"mode":"one-shot","provider":"spawn","label":"ambiguous child identity"}});
    assert_eq!(
        run(source.clone(), vec![evidence.clone()], false, None, false)
            .unwrap()
            .events,
        source
    );
    evidence["childCreatedAt"] = 3.into();
    err(run(source, vec![evidence], false, None, false), "conflicts");
}
#[test]
fn continues_catalog_backfill_after_unavailable_child_evidence() {
    for kind in ["missing", "unknown", "multiple"] {
        let evidence = json!({"childId":"unavailable","childCreatedAt":1,"descriptorCount":if kind=="missing"{0}else if kind=="multiple"{2}else{1},"descriptor":if kind=="missing"{Value::Null}else if kind=="unknown"{json!({"version":99})}else{json!({"version":3,"mode":"one-shot","provider":"spawn"})}});
        let out = run(vec![], vec![child(), evidence], false, None, false).unwrap();
        assert_eq!(
            out.events,
            vec![
                obj(
                    json!({"type":"subagent/catalog","seq":0,"time":1,"data":{"version":1,"childId":"unavailable","childCreatedAt":1,"mode":"unknown"}})
                ),
                obj(json!({"type":"subagent/catalog","seq":1,"time":1,"data":child()}))
            ]
        );
    }
}
#[test]
fn refuses_malformed_supplemental_evidence_and_known_descriptor_conflicts() {
    for source in [
        Value::Null,
        json!({}),
        json!({"childId":"child","childCreatedAt":2,"descriptor":null,"descriptorCount":-1}),
    ] {
        assert!(run(vec![], vec![source], false, None, false).is_err());
    }
    let mut evidence = json!({"childId":"child","childCreatedAt":2,"descriptorCount":1,"descriptor":{"version":3,"mode":"continuable","label":"child task"}});
    err(
        run(vec![], vec![evidence.clone()], false, None, false),
        "invalid subagent descriptor provider",
    );
    evidence["descriptor"]["provider"] = "spawn".into();
    evidence["descriptor"]["label"] = "different".into();
    err(
        run(
            vec![ev("subagent/catalog", child())],
            vec![evidence],
            false,
            None,
            false,
        ),
        "conflicts",
    );
}
#[test]
fn validates_predecessor_delivery_before_preserving_its_generation_identity() {
    let prior = ev("feedback/record", json!({"text":"prior"}));
    let marker = e(
        "session-log-deepseek/delivery-accepted",
        1,
        json!({"sessionId":"ancestor","throughSeq":0,"sessionFormatVersion":3}),
    );
    let seed = e("session/end-seed", 2, json!({"inherited":true}));
    let rows = vec![prior.clone(), marker.clone(), seed.clone()];
    assert_eq!(
        run(rows.clone(), vec![], true, None, false).unwrap().events,
        rows
    );
    err(
        run(
            dense(vec![prior.clone(), seed, marker.clone()]),
            vec![],
            true,
            None,
            false,
        ),
        "wrong Session",
    );
    for (key, value) in [
        ("sessionFormatVersion", json!(2)),
        ("sessionId", json!("parent")),
    ] {
        let mut m = marker.clone();
        m["data"][key] = value;
        assert_eq!(
            run(vec![prior.clone(), m], vec![], false, None, false)
                .unwrap()
                .events
                .len(),
            2
        );
    }
    let mut header = h(3, "parent");
    header.insert("origin".into(), "subagent".into());
    header.insert("parentSession".into(), "ancestor".into());
    let a = artifact(
        vec![ev(
            "subagent/descriptor",
            json!({"version":3,"mode":"one-shot","provider":"spawn"}),
        )],
        header,
        0,
    );
    assert_eq!(
        child_catalog_fact(&historical_child_catalog_source(&a).unwrap()).unwrap(),
        Some(obj(
            json!({"version":0,"childId":"parent","childCreatedAt":1,"mode":"one-shot"})
        ))
    );
}
