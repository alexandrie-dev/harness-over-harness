mod common;
use common::*;
use hoh_session_format::*;
use hoh_session_format_v0_to_v1::{
    RELEASED_V0_SESSION_FORMAT_CODEC, ReleasedAssistantChunkRun, SESSION_FORMAT_V0_TO_V1,
};
use hoh_session_format_v1_to_v2::{testing::assert_released_v2_artifact, *};
use serde_json::{Value, json};
use std::sync::Arc;
fn message() -> Value {
    json!({"id":"assistant-1","role":"assistant","content":[{"type":"text","text":"hello"}],"source":{"kind":"model","provider":"mock","model":"mock"}})
}
fn source(events: Vec<SessionFormatEvent>) -> SessionFormatArtifact {
    SessionFormatArtifact {
        header: header(1, "migration"),
        inherited_event_count: 0,
        events,
    }
}
fn seeded(mut a: SessionFormatArtifact, cut: u64) -> SessionFormatArtifact {
    a.header.insert("parentSession".into(), "parent".into());
    a.header.insert("isSeeded".into(), true.into());
    a.inherited_event_count = cut;
    a
}
fn migrate(a: SessionFormatArtifact) -> Result<SessionFormatArtifact, SessionFormatError> {
    let restore: SessionFormatArtifactRestorer = Arc::new(|a| {
        assert_released_v2_artifact(&a)?;
        Ok(a)
    });
    let catalog = create_session_format_catalog(SessionFormatCatalogOptions {
        current_version: 2,
        migrations: vec![
            Arc::new(SESSION_FORMAT_V0_TO_V1),
            Arc::new(SESSION_FORMAT_V1_TO_V2),
        ],
        restore_current_header: Arc::new(|h| {
            assert_released_v2_header(&h)?;
            Ok(h)
        }),
        codecs: vec![
            Arc::new(RELEASED_V0_SESSION_FORMAT_CODEC),
            Arc::new(RELEASED_V1_SESSION_FORMAT_CODEC),
            Arc::new(RELEASED_V2_SESSION_FORMAT_CODEC),
        ],
        restore_current: restore.clone(),
        current_encoder: Arc::new(RELEASED_V2_SESSION_FORMAT_CODEC),
        restore_transformed_current: restore,
    })?;
    let mut physical = a.header.clone();
    physical.shift_remove("isSeeded");
    physical.insert("type".into(), "session".into());
    if a.header["isSeeded"] == true {
        physical.insert("seedLength".into(), a.inherited_event_count.into());
    }
    let mut restore = catalog.create_restore(
        &Value::Object(physical),
        SessionFormatRestoreOptions {
            recovery: SessionFormatRecovery::Strict,
            validation: SessionFormatValidation::Current,
        },
    )?;
    for row in a.events {
        restore.decode_row(Value::Object(row))?;
    }
    restore.finish()
}
fn harness(
    cut: u64,
    seeded: bool,
    transformed: bool,
) -> (
    Box<dyn SessionFormatMigrationStage>,
    SessionFormatEventCollector,
) {
    let mut h = header(1, "child");
    if seeded {
        h.insert("parentSession".into(), "parent".into());
        h.insert("isSeeded".into(), true.into());
    }
    let target = SESSION_FORMAT_V1_TO_V2.migrate_header(h.clone()).unwrap();
    (
        SESSION_FORMAT_V1_TO_V2
            .create_stage(SessionFormatMigrationStageInput {
                source_header: &h,
                target_header: &target,
                source_inherited_event_count: Some(cut),
                source_kind: if transformed {
                    SessionFormatSourceKind::Transformed
                } else {
                    SessionFormatSourceKind::Decoded
                },
            })
            .unwrap(),
        SessionFormatEventCollector::new(),
    )
}
fn packed(
    first: u64,
    count: u64,
    step: u64,
    last: i64,
    stream: Value,
) -> Box<dyn SessionFormatEventRun> {
    Box::new(ReleasedAssistantChunkRun {
        first_seq: first,
        event_count: count,
        turn: 1,
        step,
        last_seq: first + count - 1,
        last_time: last,
        stream: obj(stream),
    })
}
fn text_record(time: i64, index: u64, text: &str) -> Value {
    json!({"type":"text-chunks","time0":time,"index":index,"dt":[],"texts":[text]})
}
fn chunk(seq: u64, time: i64, value: Value) -> SessionFormatEvent {
    event(
        "assistant/chunk",
        seq,
        time,
        json!({"turn":1,"step":1,"chunk":value}),
    )
}
fn delta(seq: u64, time: i64, text: &str) -> SessionFormatEvent {
    chunk(
        seq,
        time,
        json!({"type":"text-delta","index":0,"text":text}),
    )
}
fn finish(seq: u64, time: i64) -> SessionFormatEvent {
    chunk(seq, time, json!({"type":"finish","reason":{"kind":"stop"}}))
}
fn assistant(seq: u64, time: i64, sources: Option<Value>) -> SessionFormatEvent {
    let mut e = event(
        "assistant/message",
        seq,
        time,
        json!({"turn":1,"step":1,"message":message()}),
    );
    e.insert("surfaceOp".into(), "append".into());
    if let Some(v) = sources {
        e.insert("sourceEventSeqs".into(), v);
    }
    e
}
fn success() -> SessionFormatArtifact {
    source(vec![
        event("turn/start", 0, 100, json!({"turn":1})),
        event("step/start", 1, 101, json!({"turn":1,"step":1})),
        delta(2, 110, "hello"),
        finish(3, 120),
        assistant(4, 121, Some(json!([2, 3]))),
        event("step/end", 5, 122, json!({"turn":1,"step":1})),
        event(
            "turn/end",
            6,
            123,
            json!({"turn":1,"reason":{"kind":"completed"}}),
        ),
    ])
}
fn failure() -> Value {
    json!({"message":"provider failed","code":"PROVIDER_ERROR"})
}
fn failed() -> SessionFormatArtifact {
    source(vec![
        event("turn/start", 0, 100, json!({"turn":1})),
        event("step/start", 1, 101, json!({"turn":1,"step":1})),
        delta(2, 110, "partial"),
        chunk(
            3,
            120,
            json!({"type":"finish","reason":{"kind":"error","failure":failure()}}),
        ),
        event("step/end", 4, 121, json!({"turn":1,"step":1})),
        event(
            "turn/end",
            5,
            122,
            json!({"turn":1,"reason":{"kind":"error","error":failure()}}),
        ),
    ])
}
fn stream(events: &[SessionFormatEvent], index: usize) -> &Value {
    &events[index]["data"]["stream"]
}

#[test]
fn migrates_one_exact_released_v1_header_without_reading_events() {
    let h = header(1, "header-only");
    let mut expected = h.clone();
    expected.insert("version".into(), 2.into());
    assert_eq!(SESSION_FORMAT_V1_TO_V2.migrate_header(h).unwrap(), expected);
    expected.insert("version".into(), 0.into());
    err(
        SESSION_FORMAT_V1_TO_V2.migrate_header(expected),
        "v1 header",
    );
}
#[test]
fn validates_a_directly_decoded_v1_source_before_transforming_it() {
    err(
        migrate(source(vec![ev("external/post-v1", 0, Value::Null)])),
        "unknown event type",
    );
    err(
        migrate(source(vec![ev(
            "turn/start",
            0,
            json!({"turn":1,"postReleaseMember":true}),
        )])),
        "unexpected member",
    );
}
#[test]
fn refuses_an_assistant_chunk_envelope_with_an_invalid_member() {
    let mut extra = ev("assistant/chunk", 0, json!({}));
    extra.insert("extra".into(), true.into());
    let mut missing = ev("assistant/chunk", 0, json!({}));
    missing.shift_remove("data");
    let mut ignorable = ev("assistant/chunk", 0, json!({}));
    ignorable.insert("ignorable".into(), false.into());
    for (e, msg) in [
        (extra, "unexpected member extra"),
        (missing, "lacks required member data"),
        (ignorable, "ignorable must be true"),
    ] {
        let (mut stage, mut out) = harness(0, false, true);
        err(stage.transform_event(e, &mut out), msg);
    }
}
#[test]
fn checks_own_generation_delivery_markers_but_accepts_inherited_markers() {
    let marker = |id: &str| {
        ev(
            "session-log-deepseek/delivery-accepted",
            1,
            json!({"sessionId":id,"throughSeq":0,"sessionFormatVersion":1}),
        )
    };
    let (mut stage, mut out) = harness(0, false, false);
    err(
        stage.transform_event(marker("other"), &mut out),
        "wrong Session",
    );
    let (mut stage, mut out) = harness(2, true, false);
    stage.transform_event(marker("parent"), &mut out).unwrap();
    let mut expected = marker("parent");
    expected.insert("seq".into(), 0.into());
    assert_eq!(out.values, vec![expected]);
}
struct GenericRun(SessionFormatEvent);
impl SessionFormatEventRun for GenericRun {
    fn run_type(&self) -> &str {
        "test-run"
    }
    fn first_seq(&self) -> u64 {
        0
    }
    fn event_count(&self) -> u64 {
        1
    }
    fn expand(self: Box<Self>) -> Box<dyn Iterator<Item = SessionFormatEvent>> {
        Box::new(std::iter::once(self.0))
    }
}
#[test]
fn expands_generic_runs_and_refuses_a_packed_run_split_by_the_inherited_cut() {
    let (mut stage, mut out) = harness(0, false, true);
    let retained = ev("feedback/record", 0, json!({"text":"retained"}));
    stage
        .transform_run(Box::new(GenericRun(retained.clone())), &mut out)
        .unwrap();
    assert_eq!(out.values, vec![retained]);
    let (mut stage, mut out) = harness(1, true, false);
    err(
        stage.transform_run(
            packed(
                0,
                2,
                1,
                2,
                json!({"type":"text-chunks","time0":1,"index":0,"dt":[1],"texts":["a","b"]}),
            ),
            &mut out,
        ),
        "splits one Assistant attempt",
    );
}
#[test]
fn coalesces_adjacent_packed_text_and_tool_call_runs_without_expanding_them() {
    let (mut stage, mut out) = harness(0, false, false);
    stage
        .transform_run(packed(0, 1, 1, 1, text_record(1, 0, "a")), &mut out)
        .unwrap();
    stage
        .transform_run(
            packed(
                1,
                2,
                1,
                4,
                json!({"type":"text-chunks","time0":3,"index":0,"dt":[1],"texts":["b","c"]}),
            ),
            &mut out,
        )
        .unwrap();
    stage.finish(&mut out).unwrap();
    assert_eq!(
        *stream(&out.values, 0),
        json!([{"type":"text-chunks","time0":1,"index":0,"dt":[2,1],"texts":["a","b","c"]}])
    );
    let (mut stage, mut out) = harness(0, false, false);
    stage.transform_event(chunk(0,1,json!({"type":"tool-call-delta","index":0,"id":"call","name":"read","argumentsDelta":"{"})),&mut out).unwrap();
    stage.transform_run(packed(1,2,1,3,json!({"type":"tool-call-chunks","time0":2,"index":0,"id":"call","name":"read","dt":[1],"args":["}","!"]})),&mut out).unwrap();
    stage.finish(&mut out).unwrap();
    assert_eq!(
        *stream(&out.values, 0),
        json!([{"type":"tool-call-chunks","time0":1,"index":0,"id":"call","name":"read","dt":[1,1],"args":["{","}","!"]}])
    );
}
#[test]
fn keeps_incompatible_packed_records_and_attempt_groups_separate() {
    for (index, id, name, time0, previous) in [
        (1, "call", "read", 2, 1),
        (0, "other", "read", 2, 1),
        (0, "call", "write", 2, 1),
        (
            0,
            "call",
            "read",
            9_007_199_254_740_991,
            -9_007_199_254_740_991,
        ),
    ] {
        let (mut stage, mut out) = harness(0, false, false);
        stage.transform_run(packed(0,1,1,previous,json!({"type":"tool-call-chunks","time0":previous,"index":0,"id":"call","name":"read","dt":[],"args":["a"]})),&mut out).unwrap();
        stage.transform_run(packed(1,1,1,time0,json!({"type":"tool-call-chunks","time0":time0,"index":index,"id":id,"name":name,"dt":[],"args":["b"]})),&mut out).unwrap();
        stage.finish(&mut out).unwrap();
        assert_eq!(stream(&out.values, 0).as_array().unwrap().len(), 2);
    }
    let (mut stage, mut out) = harness(0, false, false);
    stage
        .transform_run(packed(0, 1, 1, 1, text_record(1, 0, "a")), &mut out)
        .unwrap();
    stage
        .transform_run(packed(1, 1, 2, 2, text_record(2, 0, "b")), &mut out)
        .unwrap();
    stage.finish(&mut out).unwrap();
    assert_eq!(out.values.len(), 2);
}
#[test]
fn copies_incompatible_accumulator_records_behind_an_owned_packed_prefix() {
    let (mut stage, mut out) = harness(0, false, false);
    stage
        .transform_run(packed(0, 1, 1, 1, text_record(1, 0, "packed")), &mut out)
        .unwrap();
    for (seq, text) in [(1, "a"), (2, "b")] {
        stage
            .transform_event(
                chunk(
                    seq,
                    seq as i64 + 1,
                    json!({"type":"text-delta","index":1,"text":text}),
                ),
                &mut out,
            )
            .unwrap();
    }
    stage
        .transform_run(
            packed(
                3,
                1,
                1,
                4,
                json!({"type":"reasoning-chunks","time0":4,"index":0,"dt":[],"texts":["flush"]}),
            ),
            &mut out,
        )
        .unwrap();
    stage.finish(&mut out).unwrap();
    assert_eq!(stream(&out.values, 0).as_array().unwrap().len(), 3);
    let (mut stage, mut out) = harness(0, false, false);
    stage.transform_run(packed(0,1,1,1,json!({"type":"tool-call-chunks","time0":1,"index":0,"id":"call","dt":[],"args":["a"]})),&mut out).unwrap();
    stage
        .transform_event(
            chunk(
                1,
                2,
                json!({"type":"tool-call-delta","index":0,"id":"other","argumentsDelta":"b"}),
            ),
            &mut out,
        )
        .unwrap();
    stage
        .transform_run(packed(2, 1, 1, 3, text_record(3, 0, "flush")), &mut out)
        .unwrap();
    stage.finish(&mut out).unwrap();
    assert_eq!(stream(&out.values, 0).as_array().unwrap().len(), 3);
}
#[test]
fn separates_a_terminal_packed_successor_and_a_message_from_another_attempt() {
    let (mut stage, mut out) = harness(0, false, false);
    stage.transform_event(finish(0, 1), &mut out).unwrap();
    stage
        .transform_run(packed(1, 1, 1, 2, text_record(2, 0, "next")), &mut out)
        .unwrap();
    stage.finish(&mut out).unwrap();
    assert_eq!(out.values.len(), 2);
    let (mut stage, mut out) = harness(0, false, false);
    stage
        .transform_event(delta(0, 1, "partial"), &mut out)
        .unwrap();
    let mut e = assistant(1, 2, None);
    e["data"]["turn"] = 2.into();
    stage.transform_event(e, &mut out).unwrap();
    assert_eq!(out.values.len(), 2);
    assert_eq!(*stream(&out.values, 1), json!([]));
}
#[test]
fn settles_incremental_attempts_and_rejects_ambiguous_streamed_input() {
    let events = vec![
        ev("turn/start", 0, json!({"turn":1})),
        ev("step/start", 1, json!({"turn":1,"step":1})),
        delta(2, 3, "first"),
        finish(3, 4),
        delta(4, 5, "second"),
        ev("step/end", 5, json!({"turn":1,"step":1})),
        ev(
            "turn/end",
            6,
            json!({"turn":1,"reason":{"kind":"completed"}}),
        ),
    ];
    assert_eq!(
        migrate(source(events))
            .unwrap()
            .events
            .iter()
            .filter(|e| e["type"] == "assistant/attempt")
            .count(),
        2
    );
    err(
        migrate(source(vec![delta(0, 1, "partial"), assistant(1, 2, None)])),
        "does not cite",
    );
    err(
        migrate(source(vec![
            delta(0, 1, "partial"),
            assistant(1, 2, Some(json!([1]))),
        ])),
        "complete ordered attempt",
    );
    err(
        migrate(source(vec![ev("external/unknown", 0, Value::Null)])),
        "unknown event type",
    );
    assert_eq!(
        migrate(seeded(source(vec![]), 0)).unwrap().events,
        vec![ev("session/end-seed", 0, json!({"inherited":true}))]
    );
    assert_eq!(
        migrate(seeded(
            source(vec![event("turn/start", 0, 2, json!({"turn":1}))]),
            0
        ))
        .unwrap()
        .events[0],
        event("session/end-seed", 0, 2, json!({"inherited":true}))
    );
    err(
        migrate(seeded(
            source(vec![
                delta(0, 1, "partial"),
                assistant(1, 2, Some(json!([0]))),
            ]),
            1,
        )),
        "splits one Assistant attempt",
    );
}
#[test]
fn splits_a_legacy_goal_mutation_from_its_preserved_model_visible_message() {
    let change = json!({"kind":"goal/change","version":1,"operation":"create","goal":{"id":"goal-1","revision":1,"objective":"finish","phase":"active","maxGoalRounds":2},"roundsStarted":0,"createdAt":1,"updatedAt":1});
    let payload = json!({"goal":change["goal"],"roundsStarted":0,"createdAt":1,"updatedAt":1});
    let text = format!(
        "<goal_state>{}</goal_state>",
        hoh_util_values::js_json_stringify(&payload)
    );
    let mut e = ev(
        "user/message",
        0,
        json!({"id":"goal-message","role":"user","content":[{"type":"text","text":text}],"source":{"kind":"goal","goalId":"goal-1","revision":1,"round":0,"change":change}}),
    );
    e.insert("surfaceOp".into(), "append".into());
    let result = migrate(source(vec![e])).unwrap();
    assert_eq!(result.events[0], ev("goal/change", 0, change));
    assert_eq!(result.events[1]["seq"], 1);
    assert_eq!(
        result.events[1]["data"]["source"],
        json!({"kind":"plugin","plugin":"goal"})
    );
    assert_eq!(result.events[1]["data"]["content"][0]["text"], text);
}
#[test]
fn closes_the_prior_turn_for_the_bounded_legacy_next_turn_resume_pattern() {
    let events = vec![
        ev("turn/start", 0, json!({"turn":1})),
        ev("step/start", 1, json!({"turn":1,"step":1})),
        ev("step/end", 2, json!({"turn":1,"step":1})),
        ev(
            "agent/inbox/spliced",
            3,
            json!({"target":"next-turn","start":0,"inserted":[user_data("user-1","question")]}),
        ),
        ev("turn/start", 4, json!({"turn":2})),
        ev(
            "turn/end",
            5,
            json!({"turn":2,"reason":{"kind":"completed"}}),
        ),
    ];
    let result = migrate(source(events.clone())).unwrap();
    assert_eq!(
        result
            .events
            .iter()
            .map(|e| e["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "turn/start",
            "step/start",
            "step/end",
            "agent/inbox/spliced",
            "turn/end",
            "turn/start",
            "turn/end"
        ]
    );
    assert_eq!(
        result.events[4]["data"],
        json!({"turn":1,"reason":{"kind":"interrupted"}})
    );
    let mut invalid = events;
    invalid[3]["data"]["inserted"] = json!([]);
    err(migrate(source(invalid)), "turn/start 2");
}
#[test]
fn embeds_an_interleaved_successful_stream_and_densely_remaps_survivors() {
    let mut a = success();
    a.events.insert(
        3,
        event("feedback/record", 3, 111, json!({"text":"interleaved"})),
    );
    for (i, e) in a.events.iter_mut().enumerate() {
        e.insert("seq".into(), i.into());
    }
    a.events[5].insert("sourceEventSeqs".into(), json!([2, 4]));
    a.events.push(event(
        "command/run",
        8,
        124,
        json!({"commandId":"command-1","name":"inspect","source":{"kind":"user"}}),
    ));
    a.events.push(event(
        "command/done",
        9,
        125,
        json!({"commandId":"command-1","kind":"success","sourceEventSeq":5}),
    ));
    let mut h = a.header.clone();
    h.insert("version".into(), 2.into());
    let mut committed = event(
        "assistant/message",
        3,
        121,
        json!({"turn":1,"step":1,"message":message(),"stream":[text_record(110,0,"hello"),{"type":"chunk","time":120,"chunk":{"type":"finish","reason":{"kind":"stop"}}}]}),
    );
    committed.insert("surfaceOp".into(), "append".into());
    let expected = SessionFormatArtifact {
        header: h,
        inherited_event_count: 0,
        events: vec![
            a.events[0].clone(),
            a.events[1].clone(),
            event("feedback/record", 2, 111, json!({"text":"interleaved"})),
            committed,
            event("step/end", 4, 122, json!({"turn":1,"step":1})),
            event(
                "turn/end",
                5,
                123,
                json!({"turn":1,"reason":{"kind":"completed"}}),
            ),
            event(
                "command/run",
                6,
                124,
                json!({"commandId":"command-1","name":"inspect","source":{"kind":"user"}}),
            ),
            event(
                "command/done",
                7,
                125,
                json!({"commandId":"command-1","kind":"success","sourceEventSeq":3}),
            ),
        ],
    };
    assert_eq!(migrate(a).unwrap(), expected);
}
#[test]
fn retains_a_failed_no_output_attempt_without_fabricating_a_surface_message() {
    let a = failed();
    let mut h = a.header.clone();
    h.insert("version".into(), 2.into());
    assert_eq!(
        migrate(a.clone()).unwrap(),
        SessionFormatArtifact {
            header: h,
            inherited_event_count: 0,
            events: vec![
                a.events[0].clone(),
                a.events[1].clone(),
                event(
                    "assistant/attempt",
                    2,
                    120,
                    json!({"turn":1,"step":1,"stream":[text_record(110,0,"partial"),{"type":"chunk","time":120,"chunk":{"type":"finish","reason":{"kind":"error","failure":failure()}}}]})
                ),
                event("step/end", 3, 121, json!({"turn":1,"step":1})),
                event(
                    "turn/end",
                    4,
                    122,
                    json!({"turn":1,"reason":{"kind":"error","error":failure()}})
                )
            ]
        }
    );
}
#[test]
fn separates_an_unterminated_failed_prefix_from_a_retried_attempt_in_the_same_step() {
    let a = source(vec![
        event("turn/start", 0, 100, json!({"turn":1})),
        event("step/start", 1, 101, json!({"turn":1,"step":1})),
        event(
            "request/header",
            2,
            102,
            json!({"header":{"config":{"provider":"mock","model":"mock"}},"reason":"initial"}),
        ),
        delta(3, 110, "partial"),
        event(
            "llm/retry",
            4,
            120,
            json!({"retryId":"retry-1","turn":1,"step":1,"provider":"mock","mode":"normal","policyKey":"default","retry":1,"maxRetries":1,"delayMs":0,"failure":{"code":"SERVER","message":"retry"}}),
        ),
        event(
            "llm/retry-started",
            5,
            121,
            json!({"retryId":"retry-1","turn":1,"step":1,"retry":1}),
        ),
        delta(6, 130, "hello"),
        finish(7, 140),
        assistant(8, 141, Some(json!([6, 7]))),
        event("step/end", 9, 142, json!({"turn":1,"step":1})),
        event(
            "turn/end",
            10,
            143,
            json!({"turn":1,"reason":{"kind":"completed"}}),
        ),
    ]);
    let result = migrate(a).unwrap();
    let assistant: Vec<_> = result
        .events
        .iter()
        .filter(|e| e["type"].as_str().unwrap().starts_with("assistant/"))
        .collect();
    assert_eq!(assistant.len(), 2);
    assert_eq!(
        *assistant[0],
        event(
            "assistant/attempt",
            3,
            110,
            json!({"turn":1,"step":1,"stream":[text_record(110,0,"partial")]})
        )
    );
    let mut expected = event(
        "assistant/message",
        6,
        141,
        json!({"turn":1,"step":1,"message":message(),"stream":[text_record(130,0,"hello"),{"type":"chunk","time":140,"chunk":{"type":"finish","reason":{"kind":"stop"}}}]}),
    );
    expected.insert("surfaceOp".into(), "append".into());
    assert_eq!(*assistant[1], expected);
}
#[test]
fn moves_and_tags_the_seeded_child_marker_after_collapsing_an_inherited_attempt() {
    let mut a = seeded(success(), 7);
    a.events.push(event("session/end-seed", 7, 124, json!({})));
    let result = migrate(a).unwrap();
    assert_eq!(result.inherited_event_count, 5);
    assert_eq!(
        result.events[5],
        event("session/end-seed", 5, 124, json!({"inherited":true}))
    );
}
#[test]
fn inserts_the_tagged_marker_for_an_explicitly_empty_inherited_seed() {
    let mut a = seeded(source(vec![]), 0);
    a.header.insert("createdAt".into(), 42.into());
    let mut h = a.header.clone();
    h.insert("version".into(), 2.into());
    assert_eq!(
        migrate(a).unwrap(),
        SessionFormatArtifact {
            header: h,
            inherited_event_count: 0,
            events: vec![event("session/end-seed", 0, 42, json!({"inherited":true}))]
        }
    );
}
#[test]
fn inserts_a_tagged_marker_after_a_retained_non_empty_inherited_prefix() {
    let retained = event("feedback/record", 0, 9, json!({"text":"inherited"}));
    let result = migrate(seeded(source(vec![retained.clone()]), 1)).unwrap();
    assert_eq!(result.inherited_event_count, 1);
    assert_eq!(
        result.events,
        vec![
            retained,
            event("session/end-seed", 1, 9, json!({"inherited":true}))
        ]
    );
}
#[test]
fn refuses_a_v1_message_whose_content_disagrees_with_its_cited_stream() {
    let mut a = success();
    a.events[2]["data"]["chunk"]["text"] = json!("actual");
    a.events[4]["data"]["message"]["content"][0]["text"] = json!("different");
    err(
        migrate(a),
        "message content disagrees with its embedded stream",
    );
}
#[test]
fn refuses_an_undeclared_reference_into_the_last_consumed_chunk_of_a_failed_attempt() {
    let mut a = failed();
    a.events.push(event(
        "command/run",
        6,
        123,
        json!({"commandId":"command-1","name":"inspect","source":{"kind":"user"}}),
    ));
    a.events.push(event(
        "command/done",
        7,
        124,
        json!({"commandId":"command-1","kind":"success","sourceEventSeq":3}),
    ));
    err(
        migrate(a),
        "command/done 7 sourceEventSeq targets consumed assistant/chunk 3",
    );
}
#[test]
fn refuses_an_undeclared_v1_event_even_when_its_envelope_says_ignorable() {
    let mut e = event("external/info", 0, 100, json!({"text":"unknown"}));
    e.insert("ignorable".into(), true.into());
    err(
        migrate(source(vec![e])),
        "format v1 contains unknown event type \"external/info\" at seq 0",
    );
}
#[test]
fn retains_a_legacy_message_with_source_event_seqs_as_an_empty_embedded_stream() {
    for refs in [None, Some(json!([]))] {
        let mut a = success();
        a.events = vec![
            ev("turn/start", 0, json!({"turn":1})),
            ev("step/start", 1, json!({"turn":1,"step":1})),
            assistant(2, 3, refs),
            ev("step/end", 3, json!({"turn":1,"step":1})),
            ev(
                "turn/end",
                4,
                json!({"turn":1,"reason":{"kind":"completed"}}),
            ),
        ];
        let result = migrate(a).unwrap();
        assert_eq!(result.events[2]["type"], "assistant/message");
        assert_eq!(*stream(&result.events, 2), json!([]));
        assert_eq!(result.events[2]["surfaceOp"], "append");
    }
}
#[test]
fn refuses_missing_partial_or_reordered_source_event_references_for_a_present_v1_attempt() {
    for (refs, msg) in [
        (None, "does not cite"),
        (Some(json!([2])), "complete ordered attempt"),
        (Some(json!([3, 2])), "complete ordered attempt"),
    ] {
        let mut a = success();
        a.events[4].shift_remove("sourceEventSeqs");
        if let Some(refs) = refs {
            a.events[4].insert("sourceEventSeqs".into(), refs);
        }
        err(migrate(a), msg);
    }
}
#[test]
fn rejects_invalid_messages_during_complete_target_validation() {
    let mut closed = success();
    closed.events.swap(4, 5);
    for (i, e) in closed.events.iter_mut().enumerate() {
        e.insert("seq".into(), i.into());
    }
    closed.events[5].shift_remove("sourceEventSeqs");
    err(migrate(closed), "does not match an open turn and step");
    let mut other = success();
    other.events[4]["data"]["step"] = 2.into();
    err(migrate(other), "does not match an open turn and step");
}
#[test]
fn refuses_a_lineage_cut_between_members_of_one_interleaved_stream() {
    let mut a = success();
    a.events.insert(3, ev("session/end-seed", 3, json!({})));
    for (i, e) in a.events.iter_mut().enumerate() {
        e.insert("seq".into(), i.into());
    }
    a.events[5].insert("sourceEventSeqs".into(), json!([2, 4]));
    err(migrate(seeded(a, 3)), "cut 3 splits one Assistant attempt");
}
#[test]
fn refuses_a_lineage_cut_between_a_complete_stream_and_its_committed_message() {
    let mut a = success();
    a.events.insert(4, ev("session/end-seed", 4, json!({})));
    for (i, e) in a.events.iter_mut().enumerate() {
        e.insert("seq".into(), i.into());
    }
    err(migrate(seeded(a, 4)), "cut 4 splits one Assistant attempt");
}
#[test]
fn keeps_referenced_session_generation_metadata_frozen() {
    let mut data = user_data("reference", "question");
    data["source"] = json!({"kind":"session-reference","form":"recall","version":1,"references":[{"sessionId":"source","label":"Source","capturedFormatVersion":1,"capturedThroughSeq":7,"compacted":false,"originalMessages":1,"retainedMessages":1,"omittedMessages":0,"omittedBytes":0,"truncated":false,"inputIndex":0}]});
    let mut e = ev("user/message", 0, data.clone());
    e.insert("surfaceOp".into(), "append".into());
    assert_eq!(migrate(source(vec![e])).unwrap().events[0]["data"], data);
}
fn title_request(seq: u64, message_seq: u64) -> SessionFormatEvent {
    ev(
        "session/title-llm-request",
        seq,
        json!({"titleProvider":"title-1","messageSeqs":[message_seq],"route":{"provider":"mock","model":"mock"},"system":"title","messages":[{"id":"title-request","role":"user","content":[{"type":"text","text":format!("Generate the session title from this JSON array of human messages:\n{}",hoh_util_values::js_json_stringify(&json!([{"seq":message_seq,"text":"question"}])))}],"source":{"kind":"plugin","plugin":"dsh-session-title-llm"}}],"maxTokens":20}),
    )
}
#[test]
fn remaps_surface_compaction_title_and_optional_command_references_explicitly() {
    let mut checkpoint = ev("user/message", 2, user_data("checkpoint", "question"));
    checkpoint["data"]["source"] = json!({"kind":"plugin","plugin":"test"});
    checkpoint.insert("sourceEventSeqs".into(), json!([1]));
    checkpoint.insert(
        "surfaceOp".into(),
        json!({"op":"replace","start":1,"end":1}),
    );
    let mut user = ev("user/message", 1, user_data("user-1", "question"));
    user.insert("surfaceOp".into(), "append".into());
    let a = source(vec![
        ev("turn/start", 0, json!({"turn":1})),
        user,
        checkpoint,
        ev("step/start", 3, json!({"turn":1,"step":1})),
        delta(4, 5, "hello"),
        finish(5, 6),
        assistant(6, 7, Some(json!([4, 5]))),
        ev("step/end", 7, json!({"turn":1,"step":1})),
        ev(
            "compaction/prune",
            8,
            json!({"shadowedRange":{"start":2,"end":2},"shadowedSeqs":[2],"shadowedTokenCount":1}),
        ),
        ev(
            "session/title",
            9,
            json!({"title":"Question","messageSeqs":[1],"source":{"kind":"fallback"}}),
        ),
        title_request(10, 1),
        ev(
            "command/run",
            11,
            json!({"commandId":"without-source","name":"inspect","source":{"kind":"user"}}),
        ),
        ev(
            "command/done",
            12,
            json!({"commandId":"without-source","kind":"success"}),
        ),
        ev(
            "command/run",
            13,
            json!({"commandId":"with-source","name":"inspect","source":{"kind":"user"}}),
        ),
        ev(
            "command/done",
            14,
            json!({"commandId":"with-source","kind":"success","sourceEventSeq":8}),
        ),
        ev(
            "turn/end",
            15,
            json!({"turn":1,"reason":{"kind":"completed"}}),
        ),
    ]);
    let result = migrate(a).unwrap();
    assert_eq!(
        result.events[6]["data"]["shadowedRange"],
        json!({"start":2,"end":2})
    );
    assert_eq!(result.events[6]["data"]["shadowedSeqs"], json!([2]));
    for e in result
        .events
        .iter()
        .filter(|e| e["type"].as_str().unwrap().starts_with("session/title"))
    {
        assert_eq!(e["data"]["messageSeqs"], json!([1]));
    }
    let done: Vec<_> = result
        .events
        .iter()
        .filter(|e| e["type"] == "command/done")
        .map(|e| e["data"].clone())
        .collect();
    assert_eq!(
        done,
        vec![
            json!({"commandId":"without-source","kind":"success"}),
            json!({"commandId":"with-source","kind":"success","sourceEventSeq":6})
        ]
    );
    assert_eq!(result.events[2]["sourceEventSeqs"], json!([1]));
    assert_eq!(
        result.events[2]["surfaceOp"],
        json!({"op":"replace","start":1,"end":1})
    );
}
#[test]
fn preserves_source_sequence_text_in_a_title_request_while_remapping_its_references() {
    let mut a = success();
    a.events.pop();
    let mut user = ev("user/message", 6, user_data("user-1", "question"));
    user.insert("surfaceOp".into(), "append".into());
    let request = title_request(7, 6);
    let framed = request["data"]["messages"][0]["content"][0]["text"].clone();
    a.events.extend([
        user,
        request,
        ev(
            "turn/end",
            8,
            json!({"turn":1,"reason":{"kind":"completed"}}),
        ),
    ]);
    let result = migrate(a).unwrap();
    let title = result
        .events
        .iter()
        .find(|e| e["type"] == "session/title-llm-request")
        .unwrap();
    assert_eq!(title["data"]["messageSeqs"], json!([4]));
    assert_eq!(title["data"]["messages"][0]["content"][0]["text"], framed);
}
#[test]
fn remaps_compaction_summaries_and_closes_prior_turn_groups_independently() {
    let mut user = ev("user/message", 1, user_data("user-1", "question"));
    user.insert("surfaceOp".into(), "append".into());
    let mut compact = ev("user/message", 4, user_data("compact", "question"));
    compact["data"]["source"] = json!({"kind":"plugin","plugin":"compact","compactionId":"c"});
    compact.insert("sourceEventSeqs".into(), json!([1]));
    compact.insert(
        "surfaceOp".into(),
        json!({"op":"replace","start":1,"end":1}),
    );
    let a = source(vec![
        ev("turn/start", 0, json!({"turn":1})),
        user,
        ev("compaction/start", 2, json!({"compactionId":"c","turn":1})),
        ev(
            "compaction/summary",
            3,
            json!({"compactionId":"c","summary":[{"type":"text","text":"summary"}],"shadowedRange":{"start":1,"end":1},"shadowedSeqs":[1],"shadowedTokenCount":1,"provider":"mock","model":"mock"}),
        ),
        compact,
        ev("compaction/end", 5, json!({"compactionId":"c","turn":1})),
        ev(
            "turn/end",
            6,
            json!({"turn":1,"reason":{"kind":"completed"}}),
        ),
        ev("turn/start", 7, json!({"turn":2})),
        ev("step/start", 8, json!({"turn":2,"step":1})),
        ev("step/end", 9, json!({"turn":2,"step":1})),
        ev(
            "turn/end",
            10,
            json!({"turn":2,"reason":{"kind":"completed"}}),
        ),
    ]);
    let result = migrate(a).unwrap();
    assert_eq!(
        result.events[3]["data"]["shadowedRange"],
        json!({"start":1,"end":1})
    );
    assert_eq!(result.events[3]["data"]["shadowedSeqs"], json!([1]));
}
#[test]
fn keeps_attempt_delimiters_isolated_across_consecutive_turns() {
    let failure = json!({"code":"UNKNOWN","message":"failed"});
    let mut events = Vec::new();
    for turn in 1..=2 {
        let start = events.len() as u64;
        events.extend([ev("turn/start",start,json!({"turn":turn})),ev("step/start",start+1,json!({"turn":turn,"step":1})),ev("assistant/chunk",start+2,json!({"turn":turn,"step":1,"chunk":{"type":"finish","reason":{"kind":"error","failure":failure}}})),ev("step/end",start+3,json!({"turn":turn,"step":1})),ev("turn/end",start+4,json!({"turn":turn,"reason":{"kind":"error","error":failure}}))]);
    }
    assert_eq!(
        migrate(source(events))
            .unwrap()
            .events
            .iter()
            .filter(|e| e["type"] == "assistant/attempt")
            .count(),
        2
    );
}

#[test]
fn preserves_raw_chunk_fields_and_stream_record_property_order() {
    let (mut stage, mut out) = harness(0, false, false);
    let raw = hoh_util_values::js_json_parse(
        r#"{"reason":{"kind":"stop"},"extension":{"2":2,"1":1},"type":"finish"}"#,
    )
    .unwrap();
    stage.transform_event(chunk(0, 1, raw), &mut out).unwrap();
    stage.finish(&mut out).unwrap();
    assert_eq!(
        hoh_util_values::js_json_stringify(&Value::Object(out.values[0].clone())),
        r#"{"type":"assistant/attempt","seq":0,"time":1,"data":{"turn":1,"step":1,"stream":[{"type":"chunk","time":1,"chunk":{"reason":{"kind":"stop"},"extension":{"1":1,"2":2},"type":"finish"}}]}}"#
    );
}

fn malformed_raw_chunk_is_rejected_only_by_target_validation(raw: Value) {
    let events = vec![
        ev("turn/start", 0, json!({"turn":1})),
        ev("step/start", 1, json!({"turn":1,"step":1})),
        chunk(2, 3, raw.clone()),
    ];
    let (mut stage, mut out) = harness(0, false, false);
    for event in &events {
        stage.transform_event(event.clone(), &mut out).unwrap();
    }
    assert_eq!(stage.finish(&mut out).unwrap(), 0);
    assert_eq!(
        out.values[2]["data"]["stream"],
        json!([{"type":"chunk","time":3,"chunk":raw}])
    );
    let failure = migrate(source(events)).unwrap_err();
    assert!(
        matches!(
            failure,
            SessionFormatError::Format(_) | SessionFormatError::FormatWithCause { .. }
        ),
        "{failure:?}"
    );
    assert_eq!(
        failure.to_string(),
        "assistant/attempt 2 has an invalid embedded stream"
    );
    assert!(std::error::Error::source(&failure).is_some());
}

#[test]
fn defers_a_finish_without_a_reason_until_catalog_target_validation() {
    malformed_raw_chunk_is_rejected_only_by_target_validation(json!({"type":"finish"}));
}

#[test]
fn defers_a_usage_without_usage_until_catalog_target_validation() {
    malformed_raw_chunk_is_rejected_only_by_target_validation(json!({"type":"usage"}));
}

#[test]
fn preserves_native_failures_for_malformed_chunk_payloads_through_the_catalog() {
    for (data, expected) in [
        (
            Value::Null,
            "Cannot read properties of null (reading 'turn')",
        ),
        (
            json!({"turn":1,"step":1,"chunk":null}),
            "Cannot read properties of null (reading 'type')",
        ),
        (
            json!({"turn":1,"step":1,"chunk":false}),
            "unreachable variant in AssistantStreamAccumulator.push: false",
        ),
    ] {
        let event = ev("assistant/chunk", 0, data);
        let (mut stage, mut out) = harness(0, false, false);
        let failure = stage.transform_event(event.clone(), &mut out).unwrap_err();
        assert!(
            matches!(failure, SessionFormatError::Other(_)),
            "{failure:?}"
        );
        assert_eq!(failure.to_string(), expected);
        let failure = migrate(source(vec![event])).unwrap_err();
        assert_eq!(
            failure.to_string(),
            format!(
                "@deepseek-ai/dsh-session-format-v1-to-v2 refuses this format v1 Session: {expected}"
            )
        );
        let SessionFormatError::UnsupportedMigration {
            cause: Some(cause), ..
        } = failure
        else {
            panic!("expected a migration refusal retaining its cause, got {failure:?}");
        };
        assert!(matches!(*cause, SessionFormatError::Other(_)), "{cause:?}");
        assert_eq!(cause.to_string(), expected);
    }
}

#[test]
fn distinct_object_coordinates_do_not_merge_attempts_across_the_inherited_cut() {
    for key in ["turn", "step"] {
        let mut events = vec![delta(0, 1, "first"), delta(1, 2, "second")];
        for event in &mut events {
            event["data"][key] = json!({});
        }
        let failure = migrate(seeded(source(events), 1)).unwrap_err();
        assert!(
            matches!(
                failure,
                SessionFormatError::Format(_) | SessionFormatError::FormatWithCause { .. }
            ),
            "{failure:?}"
        );
        assert_eq!(
            failure.to_string(),
            format!("assistant/attempt 0 {key} must be a non-negative safe integer")
        );
    }
}

#[test]
fn refuses_invalid_reference_indices_without_aliasing_a_survivor() {
    for index in [json!(-1), json!(2.5), json!(99)] {
        let (mut stage, mut out) = harness(0, false, true);
        for seq in 0..3 {
            stage.transform_event(feedback(seq), &mut out).unwrap();
        }
        let expected = format!(
            "command/done 3 sourceEventSeq targets consumed assistant/chunk {}",
            hoh_util_values::js_string(&index)
        );
        err(
            stage.transform_event(
                ev(
                    "command/done",
                    3,
                    json!({"commandId":"command","kind":"success","sourceEventSeq":index}),
                ),
                &mut out,
            ),
            &expected,
        );
    }
}

#[test]
fn keeps_chunk_coordinates_for_target_validation() {
    let (mut stage, mut out) = harness(0, false, false);
    let mut chunk = delta(0, 1, "partial");
    chunk["data"]["turn"] = json!(2.5);
    stage.transform_event(chunk, &mut out).unwrap();
    stage.finish(&mut out).unwrap();
    assert_eq!(out.values[0]["data"]["turn"], json!(2.5));
    err(
        assert_released_v2_artifact(&artifact(out.values)),
        "assistant/attempt 0 turn must be a non-negative safe integer",
    );
}

#[test]
fn defers_log_time_validation_and_uses_accumulator_chunk_time_errors() {
    let (mut stage, mut out) = harness(0, false, true);
    stage
        .transform_event(ev("feedback/record", 0, Value::Null), &mut out)
        .unwrap();
    assert_eq!(out.values[0]["data"], Value::Null);
    err(
        assert_released_v2_artifact(&artifact(out.values)),
        "feedback/record 0 data must be an object",
    );
    let mut log = feedback(0);
    log.insert("time".into(), json!(0.5));
    err(
        migrate(source(vec![log.clone()])),
        "format v2 event 0 time must be a safe integer",
    );
    err(
        migrate(seeded(source(vec![log]), 0)),
        "format v2 event 0 time must be a safe integer",
    );
    let mut chunk = delta(0, 1, "partial");
    chunk.insert("time".into(), json!(0.5));
    err(
        migrate(source(vec![chunk])),
        "Assistant stream time must be a safe integer, got 0.5",
    );
}

#[test]
fn distinguishes_missing_and_null_surface_replacement_coordinates() {
    for (operation, expected) in [
        (
            json!({}),
            "surface start targets consumed assistant/chunk undefined",
        ),
        (
            json!({"start":null,"end":0}),
            "surface start targets consumed assistant/chunk null",
        ),
        (
            json!({"start":0}),
            "surface end targets consumed assistant/chunk undefined",
        ),
        (
            json!(true),
            "surface start targets consumed assistant/chunk undefined",
        ),
        (
            Value::Null,
            "Cannot read properties of null (reading 'start')",
        ),
    ] {
        let (mut stage, mut out) = harness(0, false, true);
        stage.transform_event(feedback(0), &mut out).unwrap();
        let mut event = user(1, None);
        event.insert("surfaceOp".into(), operation.clone());
        let failure = stage.transform_event(event, &mut out).unwrap_err();
        assert!(failure.to_string().contains(expected), "{failure}");
        assert_eq!(failure.is_unsupported_migration(), !operation.is_null());
    }
}
