mod common;
use common::*;
use hoh_session_format_v3_to_v4::references::remap_v3_references;
fn start(n: u64) -> SessionFormatEvent {
    ev("turn/start", json!({"turn":n}))
}
fn end(n: u64) -> SessionFormatEvent {
    ev("turn/end", json!({"turn":n,"reason":{"kind":"completed"}}))
}
fn splice() -> SessionFormatEvent {
    ev(
        "agent/inbox/spliced",
        json!({"target":"next-turn","inserted":[user_data("next")]}),
    )
}
fn prefix() -> Vec<SessionFormatEvent> {
    vec![
        start(1),
        ev("step/start", json!({"turn":1,"step":1})),
        ev("step/end", json!({"turn":1,"step":1})),
        splice(),
    ]
}
fn joined(parts: Vec<Vec<SessionFormatEvent>>) -> Vec<SessionFormatEvent> {
    parts.into_iter().flatten().collect()
}
fn restore(rows: Vec<SessionFormatEvent>) -> R<SessionFormatArtifact> {
    restore_v3(
        &numbered(rows, 10),
        h(3, "restart"),
        SessionFormatRecovery::Strict,
    )
}
#[test]
fn closes_an_interrupted_turn_at_the_next_turn_restart_and_reopens_the_resulting_v4_unchanged() {
    let rows = numbered(joined(vec![prefix(), vec![start(2), end(2)]]), 10);
    let before = rows.clone();
    let a = restore_v3(&rows, h(3, "restart"), SessionFormatRecovery::Strict).unwrap();
    let mut expected = rows[..4].to_vec();
    expected.push(obj(json!({"type":"turn/end","seq":4,"time":14,"data":{"turn":1,"reason":{"kind":"interrupted"}}})));
    for mut event in rows[4..].iter().cloned() {
        event.insert("seq".into(), (event["seq"].as_u64().unwrap() + 1).into());
        expected.push(event);
    }
    assert_eq!(a.events, expected);
    assert_eq!(
        round_trip(&a.events, &a.header, a.inherited_event_count).unwrap(),
        (a.events.clone(), 0)
    );
    assert_eq!(rows, before);
    err(native(rows), "turn/start does not open the expected turn");
}
#[test]
fn retains_open_tails_and_balanced_turns_and_repairs_each_independently_evidenced_restart() {
    let tail = numbered(prefix(), 10);
    assert_eq!(
        restore_v3(&tail, h(3, "restart"), SessionFormatRecovery::Strict)
            .unwrap()
            .events,
        tail
    );
    let balanced = numbered(joined(vec![prefix(), vec![end(1), start(2), end(2)]]), 10);
    assert_eq!(
        restore_v3(&balanced, h(3, "restart"), SessionFormatRecovery::Strict)
            .unwrap()
            .events,
        balanced
    );
    let twice = restore(joined(vec![
        prefix(),
        vec![start(2), splice(), start(3), end(3)],
    ]))
    .unwrap();
    assert_eq!(
        twice
            .events
            .iter()
            .filter(|e| e["type"] == "turn/end")
            .map(|e| e["data"].clone())
            .collect::<Vec<_>>(),
        vec![
            json!({"turn":1,"reason":{"kind":"interrupted"}}),
            json!({"turn":2,"reason":{"kind":"interrupted"}}),
            json!({"turn":3,"reason":{"kind":"completed"}})
        ]
    );
    for (i, e) in twice.events.iter().enumerate() {
        assert_eq!(e["seq"], i);
    }
}
#[test]
fn refuses_unevidenced_or_unsettled_restarts() {
    let mut empty_splice = splice();
    empty_splice["data"]["inserted"] = json!([]);
    let mut step_splice = splice();
    step_splice["data"]["target"] = "next-step".into();
    let cases = vec![
        ("repeated turn", joined(vec![prefix(), vec![start(1)]])),
        ("skipped turn", joined(vec![prefix(), vec![start(3)]])),
        (
            "empty splice",
            joined(vec![prefix()[..3].to_vec(), vec![empty_splice, start(2)]]),
        ),
        (
            "step splice",
            joined(vec![prefix()[..3].to_vec(), vec![step_splice, start(2)]]),
        ),
        (
            "nonadjacent splice",
            joined(vec![
                prefix(),
                vec![ev("feedback/record", json!({})), start(2)],
            ]),
        ),
        (
            "open step",
            joined(vec![prefix()[..2].to_vec(), vec![splice(), start(2)]]),
        ),
        (
            "unsettled compaction",
            vec![
                start(1),
                ev("compaction/start", json!({"compactionId":"c","turn":1})),
                splice(),
                start(2),
            ],
        ),
    ];
    for (name, rows) in cases {
        assert!(restore(rows).is_err(), "{name}");
    }
}
#[test]
fn remaps_both_sides_of_a_restart_through_compaction_surface_replacement_command_completion_and_titles()
 {
    let (rows, title_message) = complex_restart_source();
    let before = rows.clone();
    let a = restore_v3(&rows, h(3, "restart"), SessionFormatRecovery::Strict).unwrap();
    assert_eq!(
        a.events[7]["data"]["shadowedRange"],
        json!({"start":1,"end":5})
    );
    assert_eq!(a.events[7]["data"]["shadowedSeqs"], json!([1, 5]));
    assert_eq!(a.events[8]["sourceEventSeqs"], json!([1, 5, 6, 7]));
    assert_eq!(
        a.events[8]["surfaceOp"],
        json!({"op":"replace","startSeq":1,"endSeq":5})
    );
    assert_eq!(a.events[11]["data"]["sourceEventSeq"], 8);
    assert_eq!(a.events[12]["data"]["messageSeqs"], json!([1, 5]));
    assert_eq!(
        a.events[13]["data"],
        json!({"messageSeqs":[1,5],"messages":[title_message]})
    );
    assert_eq!(a.events[14]["data"], rows[13]["data"]);
    assert_eq!(
        a.events[15]["data"]["source"],
        json!({"kind":"session-reference","sessionId":"other","seq":4,"sessionFormatVersion":3})
    );
    let mut expected_opaque = rows[15].clone();
    expected_opaque.insert("type".into(), "plugin:external/opaque".into());
    expected_opaque.insert("seq".into(), 16.into());
    assert_eq!(a.events[16], expected_opaque);
    assert_eq!(a.header, h(4, "restart"));
    assert_eq!(rows, before);
}
#[test]
fn remaps_image_offload_targets_while_retaining_occurrence_indexes_and_image_identities() {
    let image = json!({"type":"image","attachment":{"attachmentId":"image","mediaType":"image/png","bytes":1,"width":1,"height":1}});
    let mut picture = user("picture");
    picture["data"]["content"] = json!([image]);
    let a = restore(joined(vec![
        prefix(),
        vec![
            start(2),
            picture,
            ev(
                "image/offload",
                json!({"targets":[{"seq":5,"imageIndexes":[0]}]}),
            ),
            end(2),
        ],
    ]))
    .unwrap();
    assert_eq!(
        a.events[7]["data"]["targets"],
        json!([{"seq":6,"imageIndexes":[0]}])
    );
    assert_eq!(a.events[6]["data"]["content"][0], image);
    assert_eq!(a.header, h(4, "restart"));
}
#[test]
fn remaps_the_final_inherited_cut_and_validates_delivery_ownership_in_source_coordinates() {
    let mut header = h(3, "restart");
    header.insert("isSeeded".into(), true.into());
    header.insert("parentSession".into(), "parent".into());
    let marker = ev("session/end-seed", json!({"inherited":true}));
    let foreign = ev(
        "session-log-deepseek/delivery-accepted",
        json!({"sessionId":"parent","throughSeq":4,"sessionFormatVersion":3}),
    );
    let source = numbered(
        joined(vec![
            prefix(),
            vec![start(2), foreign.clone(), end(2), marker.clone()],
        ]),
        10,
    );
    assert_eq!(
        restore_v3(&source, header.clone(), SessionFormatRecovery::Strict)
            .unwrap()
            .inherited_event_count,
        8
    );
    let twice = numbered(
        joined(vec![
            prefix(),
            vec![start(2), marker.clone(), foreign.clone(), marker.clone()],
        ]),
        10,
    );
    assert_eq!(
        restore_v3(&twice, header.clone(), SessionFormatRecovery::Strict)
            .unwrap()
            .inherited_event_count,
        8
    );
    let earlier = numbered(
        joined(vec![
            prefix(),
            vec![end(1), marker.clone(), start(2), splice(), start(3)],
        ]),
        10,
    );
    assert_eq!(
        restore_v3(&earlier, header.clone(), SessionFormatRecovery::Strict)
            .unwrap()
            .inherited_event_count,
        5
    );
    let bad = numbered(joined(vec![prefix(), vec![start(2), marker, foreign]]), 10);
    err(
        restore_v3(&bad, header.clone(), SessionFormatRecovery::Strict),
        "wrong Session",
    );
    let mut current = stage(&header, Some(7), vec![]).unwrap();
    let mut out = SessionFormatEventCollector::new();
    current
        .transform_run(Box::new(Run(source.clone())), &mut out)
        .unwrap();
    assert_eq!(current.finish(&mut out).unwrap(), 8);
    assert_eq!(
        out.values,
        restore_v3(&source, header, SessionFormatRecovery::Strict)
            .unwrap()
            .events
    );
}
#[test]
fn refuses_unresolved_tool_advertisements_even_when_a_restart_has_the_expected_splice() {
    let rows = joined(vec![
        prefix()[..2].to_vec(),
        vec![assistant(
            json!([{"type":"tool-call","id":"call","name":"test","arguments":"{}"}]),
        )],
        prefix()[2..].to_vec(),
        vec![start(2)],
    ]);
    err(restore(rows), "unresolved");
}
#[test]
fn remaps_prune_ranges_and_optional_command_references_without_interpreting_unrelated_data() {
    assert_eq!(
        remap_v3_references(
            e(
                "compaction/prune",
                2,
                json!({"shadowedRange":{"start":0,"end":1},"shadowedSeqs":[0,1]})
            ),
            3,
            &[0, 2]
        )
        .unwrap()["data"],
        json!({"shadowedRange":{"start":0,"end":2},"shadowedSeqs":[0,2]})
    );
    assert_eq!(
        remap_v3_references(
            e("command/done", 1, json!({"commandId":"c","kind":"error"})),
            2,
            &[0]
        )
        .unwrap()["data"],
        json!({"commandId":"c","kind":"error"})
    );
    assert_eq!(
        remap_v3_references(e("feedback/record", 1, Value::Null), 2, &[0]).unwrap()["data"],
        Value::Null
    );
}
#[test]
fn refuses_malformed_source_references_after_insertion() {
    for (tag, data) in [
        ("command/done", Value::Null),
        ("command/done", json!({"sourceEventSeq":2})),
        ("command/done", json!({"sourceEventSeq":-1})),
        ("session/title", json!({"messageSeqs":null})),
        ("image/offload", json!({"targets":null})),
        ("image/offload", json!({"targets":[null]})),
    ] {
        assert!(remap_v3_references(e(tag, 2, data), 3, &[0, 2]).is_err());
    }
}
