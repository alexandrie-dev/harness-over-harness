mod common;
use common::*;
use hoh_session_format::*;
use hoh_session_format_v2_to_v3::*;
use serde_json::{Value, json};
use std::collections::HashSet;
fn with(
    mut a: Vec<SessionFormatEvent>,
    b: impl IntoIterator<Item = SessionFormatEvent>,
) -> Vec<SessionFormatEvent> {
    a.extend(b);
    a
}

#[test]
fn pins_the_generated_system_message_id_to_the_catalog_fixture() {
    let artifact = raw(dense(opening()), h(2, "v2-identity"), Some(0)).unwrap();
    assert_eq!(
        systems(&artifact)[0]["data"]["message"]["id"],
        "v2-to-v3-system-9673c4ed630de6c21ea6bd6b573094ea8e5e216843a1b572a68657499ad9667b"
    );
}

#[test]
fn hashes_a_non_ascii_header_id_as_compact_utf8_json() {
    // node -e 'const {createHash}=require("node:crypto"); console.log("v2-to-v3-system-"+createHash("sha256").update(JSON.stringify(["session-format-v2-to-v3","сессия-猫-🦀",1,"step/start"])).digest("hex"))'
    let artifact = raw(dense(opening()), h(2, "сессия-猫-🦀"), Some(0)).unwrap();
    assert_eq!(
        systems(&artifact)[0]["data"]["message"]["id"],
        "v2-to-v3-system-839148ff4c9e493a1990f9e5468f22c5b7e290b412e7df52843c7ff443e7c807"
    );
}
#[test]
fn emits_an_empty_head_immediately_after_first_step_preserves_chronology_and_captures_changed_and_cleared_prompts()
 {
    let input = dense(with(
        opening(),
        [
            user("user"),
            request(Some("first")),
            request(Some("first")),
            request(Some("changed")),
            request(None),
            request(Some("")),
        ],
    ));
    let mut s = stage(&h(2, "identity"), None, SessionFormatSourceKind::Decoded).unwrap();
    let mut out = SessionFormatEventCollector::new();
    s.transform_event(input[0].clone(), &mut out).unwrap();
    assert_eq!(out.values.len(), 1);
    s.transform_event(input[1].clone(), &mut out).unwrap();
    assert_eq!(
        out.values
            .iter()
            .map(|e| e["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["turn/start", "step/start", "system/message"]
    );
    for e in &input[2..] {
        s.transform_event(e.clone(), &mut out).unwrap();
    }
    assert_eq!(s.finish(&mut out).unwrap(), 0);
    let a = migrate(input.clone()).unwrap();
    assert_eq!(systems(&a).len(), 4);
    assert_eq!(requests(&a.events, 3), requests(&input, 2));
    assert_eq!(
        a.events
            .iter()
            .filter(|e| e["type"] != "system/message")
            .map(|e| (&e["type"], &e["time"]))
            .collect::<Vec<_>>(),
        input
            .iter()
            .map(|e| (&e["type"], &e["time"]))
            .collect::<Vec<_>>()
    );
    assert!(
        a.events
            .iter()
            .filter(|e| e["type"] == "request/header")
            .all(|e| e["data"]["header"].get("system").is_none())
    );
    assert_eq!(migrate(input.clone()).unwrap(), a);
    assert_eq!(input[3]["data"], request(Some("first"))["data"]);
}
#[test]
fn remaps_exact_local_ranges_and_lists_without_putting_the_head_inside_compaction() {
    let mut replacement = user("replacement");
    replacement.insert(
        "surfaceOp".into(),
        json!({"op":"replace","start":2,"end":4}),
    );
    replacement.insert("sourceEventSeqs".into(), json!([2, 4]));
    let source = with(
        opening(),
        [
            user("a"),
            request(Some("sys")),
            user("b"),
            ev(
                "compaction/prune",
                json!({"shadowedRange":{"start":2,"end":4},"shadowedSeqs":[2,4],"shadowedTokenCount":20}),
            ),
            replacement,
            ev(
                "command/run",
                json!({"commandId":"c","name":"x","source":{"kind":"user"}}),
            ),
            ev(
                "command/done",
                json!({"commandId":"c","kind":"success","sourceEventSeq":6}),
            ),
            ev(
                "session/title",
                json!({"title":"title","messageSeqs":[2,4],"source":{"kind":"fallback"}}),
            ),
        ],
    );
    let target = migrate(source).unwrap();
    let rows: Vec<_> = target
        .events
        .iter()
        .filter(|e| e["type"] != "system/message")
        .collect();
    let refs = json!([rows[2]["seq"], rows[4]["seq"]]);
    assert_eq!(rows[5]["data"]["shadowedSeqs"], refs);
    assert_eq!(rows[6]["sourceEventSeqs"], refs);
    assert_eq!(rows[8]["data"]["sourceEventSeq"], rows[6]["seq"]);
    assert_eq!(rows[9]["data"]["messageSeqs"], refs);
    let mut bad = target;
    let prune = bad
        .events
        .iter_mut()
        .find(|e| e["type"] == "compaction/prune")
        .unwrap();
    prune["data"]["shadowedRange"] = json!({"start":4,"end":4});
    prune["data"]["shadowedSeqs"] = json!([4]);
    err(restore(bad), "protected");
}
#[test]
fn keeps_headerless_aborted_steps_and_metadata_only_logs_without_inventing_a_tail_request() {
    assert!(migrate(vec![]).unwrap().events.is_empty());
    assert_eq!(
        migrate(with(opening(), [user("user")]))
            .unwrap()
            .events
            .last()
            .unwrap()["type"],
        "user/message"
    );
    assert_eq!(
        migrate(vec![ev("feedback/record", json!({"text":"metadata"}))])
            .unwrap()
            .events
            .len(),
        1
    );
}
#[test]
fn detects_deterministic_head_and_update_identity_collisions_with_source_messages_in_either_order()
{
    let input = with(opening(), [user("user"), request(Some("system"))]);
    let a = migrate(input.clone()).unwrap();
    for system in systems(&a) {
        let id = system["data"]["message"]["id"].as_str().unwrap();
        err(
            migrate(with(opening(), [user(id), request(Some("system"))])),
            "collides",
        );
    }
    let inbox = |id: &str| {
        ev(
            "agent/inbox/spliced",
            json!({"target":"next-turn","start":0,"inserted":[user_data(id)]}),
        )
    };
    let shifted = migrate(with(vec![inbox("placeholder")], input.clone())).unwrap();
    let id = systems(&shifted)[0]["data"]["message"]["id"]
        .as_str()
        .unwrap();
    err(migrate(with(vec![inbox(id)], input)), "collides");
}
#[test]
fn refuses_pre_step_surfaces_and_changed_prompts_outside_a_step_rather_than_reorder_source_history()
{
    err(
        migrate(with(vec![user("user")], opening())),
        "before first step",
    );
    err(
        migrate(vec![
            opening()[0].clone(),
            request(Some("early")),
            opening()[1].clone(),
        ]),
        "outside an open step",
    );
    err(
        migrate(with(
            opening(),
            [closing()[0].clone(), request(Some("late"))],
        )),
        "outside an open step",
    );
}
#[test]
fn preserves_delivery_generation_coordinates_and_validates_ownership_before_promotion() {
    for version in [None, Some(0), Some(1), Some(2)] {
        let mut data = json!({"sessionId":"identity","throughSeq":1});
        if let Some(v) = version {
            data["sessionFormatVersion"] = json!(v);
        }
        let marker = ev("session-log-deepseek/delivery-accepted", data);
        assert_eq!(
            migrate(with(opening(), [marker.clone()]))
                .unwrap()
                .events
                .last()
                .unwrap()["data"],
            marker["data"]
        );
        let mut foreign = marker;
        foreign["data"]["sessionId"] = "foreign".into();
        foreign["data"]["sessionFormatVersion"] = 2.into();
        err(migrate(with(opening(), [foreign])), "wrong Session");
    }
}
#[test]
fn derives_unknown_seeded_cuts_after_insertion_and_isolates_simultaneous_stages() {
    let rows = dense(with(
        opening(),
        [
            request(Some("seed")),
            ev(
                "session-log-deepseek/delivery-accepted",
                json!({"sessionId":"parent","throughSeq":1,"sessionFormatVersion":2}),
            ),
            ev("session/end-seed", json!({"inherited":true})),
        ],
    ));
    let mut a = stage(&seeded_header(), None, SessionFormatSourceKind::Decoded).unwrap();
    let mut b = stage(&h(2, "identity"), None, SessionFormatSourceKind::Decoded).unwrap();
    assert_eq!(a.header_inherited_event_count(), None);
    assert_eq!(b.header_inherited_event_count(), Some(0));
    let mut out = SessionFormatEventCollector::new();
    for row in &rows {
        a.transform_event(row.clone(), &mut out).unwrap();
    }
    assert_eq!(a.finish(&mut out).unwrap(), 6);
    assert_eq!(
        b.finish(&mut SessionFormatEventCollector::new()).unwrap(),
        0
    );
    assert_eq!(
        restore(raw(rows.clone(), seeded_header(), Some(4)).unwrap())
            .unwrap()
            .inherited_event_count,
        6
    );
    err(raw(rows, seeded_header(), Some(3)), "source cut");
    err(raw(vec![], seeded_header(), None), "inherited");
    err(
        migrate(vec![ev("session/end-seed", json!({"inherited":true}))]),
        "inherited",
    );
}
#[test]
fn preserves_exact_tool_not_started_message_ids_through_coordinate_shifts() {
    let call = "call-with-dashes";
    let assistant = assistant(
        json!([{"type":"tool-call","id":call,"name":"test","arguments":"{}"}]),
        json!([]),
    );
    let repair = |id: String| {
        surface(
            "tool/result",
            json!({"turn":1,"step":1,"error":{"name":"ToolNotStartedError","code":"TOOL_NOT_STARTED"},"message":{"id":id,"role":"user","source":{"kind":"tool","callId":call},"content":[{"type":"tool-result","toolCallId":call,"isError":true,"content":[{"type":"text","text":"The tool call was interrupted before the Harness recorded it as started. Retry it if it is still needed."}]}]}}),
        )
    };
    for suffix in ["4", "999"] {
        let repair = repair(format!("interrupted-tool-result-{call}-{suffix}"));
        let a = migrate(with(
            opening(),
            [
                request(Some("system")),
                assistant.clone(),
                repair.clone(),
                closing()[0].clone(),
            ],
        ))
        .unwrap();
        let result = a
            .events
            .iter()
            .find(|e| e["type"] == "tool/result")
            .unwrap();
        assert_eq!(result["data"], repair["data"]);
        assert_eq!(result["seq"], 6);
        assert_eq!(restore(a.clone()).unwrap(), a);
    }
    err(
        migrate(with(
            opening(),
            [
                request(Some("system")),
                assistant,
                repair("arbitrary-repair-id".into()),
            ],
        )),
        "canonical historical",
    );
}
#[test]
fn preserves_other_session_captures_workflow_local_seq_and_model_input_containing_source_numbers() {
    let mut captured = user("user");
    captured["data"]["source"] = json!({"kind":"session-reference","form":"recall","version":1,"references":[{"sessionId":"other","label":"other","capturedThroughSeq":99,"capturedFormatVersion":2,"compacted":false,"originalMessages":1,"retainedMessages":1,"omittedMessages":0,"omittedBytes":0,"truncated":false,"inputIndex":0}]});
    let mut message = user_data("title");
    message["source"] = json!({"kind":"plugin","plugin":"dsh-session-title-llm"});
    message["content"] = json!([{"type":"text","text":"source seq=4 (preserved model input)"}]);
    let input = with(
        opening(),
        [
            captured,
            ev(
                "tool-workflow/agent-start",
                json!({"runId":"run","seq":99,"label":"child","childId":"other"}),
            ),
            user("human"),
            title(4, json!([message])),
        ],
    );
    let a = migrate(input.clone()).unwrap();
    let kept: Vec<_> = a
        .events
        .iter()
        .filter(|e| e["type"] != "system/message")
        .collect();
    assert_eq!(kept[2]["data"], input[2]["data"]);
    assert_eq!(kept[3]["data"], input[3]["data"]);
    assert_eq!(kept[5]["data"]["messageSeqs"], json!([5]));
    assert_eq!(kept[5]["data"]["messages"], input[5]["data"]["messages"]);
}
struct Run(Vec<SessionFormatEvent>);
impl SessionFormatEventRun for Run {
    fn run_type(&self) -> &str {
        "test"
    }
    fn first_seq(&self) -> u64 {
        0
    }
    fn event_count(&self) -> u64 {
        self.0.len() as u64
    }
    fn expand(self: Box<Self>) -> Box<dyn Iterator<Item = SessionFormatEvent>> {
        Box::new(self.0.into_iter())
    }
}
#[test]
fn expands_compact_input_incrementally_through_independent_stage_state() {
    let rows = dense(with(opening(), [request(Some("run"))]));
    let mut s = stage(&h(2, "identity"), None, SessionFormatSourceKind::Decoded).unwrap();
    let mut out = SessionFormatEventCollector::new();
    s.transform_run(Box::new(Run(rows.clone())), &mut out)
        .unwrap();
    assert_eq!(s.finish(&mut out).unwrap(), 0);
    assert_eq!(out.values, migrate(rows).unwrap().events);
}
#[test]
fn derives_the_inherited_cut_after_assistant_chunks_collapse_upstream() {
    for version in [0, 1] {
        let mut message = surface(
            "assistant/message",
            json!({"turn":1,"step":1,"message":{"id":"assistant","role":"assistant","source":model(),"content":[{"type":"text","text":"hello"}]}}),
        );
        message.insert("sourceEventSeqs".into(), json!([3, 4]));
        let rows = dense(with(
            opening(),
            [
                request(Some("seed")),
                ev(
                    "assistant/chunk",
                    json!({"turn":1,"step":1,"chunk":{"type":"text-delta","index":0,"text":"hello"}}),
                ),
                ev(
                    "assistant/chunk",
                    json!({"turn":1,"step":1,"chunk":{"type":"finish","reason":{"kind":"stop"}}}),
                ),
                message,
                closing()[0].clone(),
                closing()[1].clone(),
                ev("session/end-seed", json!({})),
            ],
        ));
        let a=read(json!({"type":"session","version":version,"id":"identity","createdAt":1,"delegationDepth":0,"seedLength":8}),rows,SessionFormatRecovery::Strict,SessionFormatValidation::Current).unwrap();
        assert_eq!(a.inherited_event_count, 8);
        assert!(a.events.iter().all(|e| e["type"] != "assistant/chunk"));
        assert_eq!(systems(&a).len(), 2);
        assert_eq!(a.events.last().unwrap()["seq"], a.inherited_event_count);
    }
}
#[test]
fn restores_seeded_logs_through_physical_codecs_and_every_adjacent_stage() {
    for version in [0, 1, 2] {
        let rows = dense(with(
            opening(),
            [
                user("user"),
                request(Some("seed")),
                closing()[0].clone(),
                closing()[1].clone(),
                ev(
                    "session/end-seed",
                    if version == 2 {
                        json!({"inherited":true})
                    } else {
                        json!({})
                    },
                ),
            ],
        ));
        let header = if version == 2 {
            physical(seeded_header())
        } else {
            json!({"type":"session","version":version,"id":"identity","createdAt":1,"delegationDepth":0,"seedLength":6})
        };
        let a = read(
            header,
            rows.clone(),
            SessionFormatRecovery::Strict,
            SessionFormatValidation::Current,
        )
        .unwrap();
        assert_eq!(a.inherited_event_count, 8);
        assert_eq!(requests(&a.events, 3), requests(&rows, 2));
    }
}
#[test]
fn rejects_unaudited_payloads() {
    let mut opaque = ev("external/opaque", json!({}));
    opaque.insert("ignorable".into(), true.into());
    let mut request = request(None);
    request["data"]["futureRef"] = 0.into();
    let mut future = user("user");
    future["data"]["source"] = json!({"kind":"future","seq":0});
    for bad in [
        ev("external/opaque", json!({"seq":1})),
        opaque,
        request,
        future,
    ] {
        let error = migrate(with(opening(), [bad])).unwrap_err().to_string();
        assert!(
            error.contains("unclassified") || error.contains("unexpected"),
            "{error}"
        );
    }
}
#[test]
fn rejects_invalid_references_future_generation_claims_and_unclassified_message_content() {
    let mut bad = user("user");
    bad.insert("sourceEventSeqs".into(), json!([99]));
    err(migrate(with(opening(), [bad])), "earlier");
    err(
        migrate(with(opening(), [ev("system/message", json!({}))])),
        "unclassified",
    );
    err(
        migrate(with(
            opening(),
            [ev(
                "session-log-deepseek/delivery-accepted",
                json!({"sessionId":"identity","throughSeq":1,"sessionFormatVersion":3}),
            )],
        )),
        "format v3",
    );
    let mut bad = user("user");
    bad["data"]["content"] = json!([{"type":"future-block","seq":1}]);
    err(
        migrate(with(opening(), [bad])),
        "unclassified message content",
    );
}
#[test]
fn migrates_audited_agent_relay_sources_and_file_blocks_verbatim_refusing_unclassified_members() {
    let mut data = user_data("relay");
    data["source"] =
        json!({"kind":"agent-message","form":"relay","senderSessionId":"other-session"});
    data["content"] = json!([{"type":"file","attachment":{"attachmentId":"sha256:content","name":"poem.txt","bytes":16}}]);
    let inbox = ev(
        "agent/inbox/spliced",
        json!({"target":"next-turn","start":0,"inserted":[data]}),
    );
    let a = migrate(with(
        with(vec![inbox.clone()], opening()),
        [surface("user/message", data.clone())],
    ))
    .unwrap();
    assert_eq!(a.events[0]["data"], inbox["data"]);
    assert_eq!(a.events.last().unwrap()["data"], data);
    for (path, value) in [
        (0, json!(0)),
        (1, json!("notice")),
        (2, json!(0)),
        (3, json!(-1)),
    ] {
        let mut bad = data.clone();
        match path {
            0 => bad["source"]["sourceEventSeq"] = value,
            1 => bad["source"]["form"] = value,
            2 => bad["content"][0]["attachment"]["seq"] = value,
            _ => bad["content"][0]["attachment"]["bytes"] = value,
        }
        fails(migrate(with(opening(), [surface("user/message", bad)])));
    }
}
#[test]
fn restores_log_only_failed_attempts_without_invoking_the_v0_event_inventory() {
    for version in [0, 1, 2] {
        let stream = json!([{"type":"finish","reason":{"kind":"error","error":{"name":"ProviderError","message":"failed"}}}]);
        let attempt = if version == 2 {
            ev(
                "assistant/attempt",
                json!({"turn":1,"step":1,"stream":stream}),
            )
        } else {
            ev(
                "assistant/chunk",
                json!({"turn":1,"step":1,"chunk":stream[0]}),
            )
        };
        let header = if version == 2 {
            physical(h(2, "identity"))
        } else {
            json!({"type":"session","version":version,"id":"identity","createdAt":1,"delegationDepth":0})
        };
        let a = read(
            header,
            dense(with(opening(), [attempt])),
            SessionFormatRecovery::Strict,
            SessionFormatValidation::Current,
        )
        .unwrap();
        assert_eq!(a.events.last().unwrap()["type"], "assistant/attempt");
    }
}
#[test]
fn rejects_malformed_v2_attempts_and_unaudited_attempt_fields() {
    for data in [
        json!({"turn":0,"step":1,"stream":[]}),
        json!({"turn":1,"step":1,"stream":null}),
        json!({"turn":1,"step":1,"stream":[],"seqRef":0}),
    ] {
        fails(migrate(with(opening(), [ev("assistant/attempt", data)])));
    }
}
#[test]
fn preserves_message_feedback_identities_and_rejects_unaudited_feedback_fields() {
    let mut feedback = ev(
        "feedback/message-put",
        json!({"sessionId":"other","item":{"messageId":"unchanged","rating":"positive","version":"opaque","createdAt":1,"updatedAt":2}}),
    );
    assert_eq!(
        migrate(with(opening(), [feedback.clone()]))
            .unwrap()
            .events
            .last()
            .unwrap()["data"],
        feedback["data"]
    );
    feedback["data"]["seq"] = 2.into();
    err(migrate(with(opening(), [feedback])), "unexpected");
}
#[test]
fn round_trips_system_messages_and_empty_heads_returning_no_projected_user_message_substitutes() {
    let a = migrate(with(opening(), [request(Some("system"))])).unwrap();
    assert_eq!(round_trip(a.clone()).unwrap(), a);
    assert_eq!(restore(a.clone()).unwrap(), a);
    assert!(a.events.iter().all(|e| e["type"] != "user/message"));
}
#[test]
fn rejects_retired_header_system_on_encode_decode_and_logical_restore() {
    let mut bad = request(Some("retired"));
    bad.insert("seq".into(), 2.into());
    err(
        RELEASED_V3_SESSION_FORMAT_CODEC.encode_event(&bad),
        "header.system",
    );
    let mut decoder = decoder(false, SessionFormatRecovery::Strict);
    let mut out = SessionFormatEventCollector::new();
    for row in dense(opening()) {
        decoder.decode_row(Value::Object(row), &mut out).unwrap();
    }
    err(
        decoder.decode_row(Value::Object(bad.clone()), &mut out),
        "header.system",
    );
    err(
        restore(artifact(with(dense(opening()), [bad]))),
        "header.system",
    );
}
#[test]
fn rejects_malformed_native_system_payloads_and_foreign_payload_members() {
    let a = migrate(opening()).unwrap();
    for path in 0..5 {
        let mut bad = a.clone();
        let system = &mut bad.events[2];
        match path {
            0 => system["data"]["extra"] = 0.into(),
            1 => system["data"]["message"]["role"] = "user".into(),
            2 => system["data"]["message"]["source"] = json!({"kind":"user"}),
            3 => system["data"]["message"]["content"] = json!([{"type":"text","text":12}]),
            _ => {
                system.insert(
                    "surfaceOp".into(),
                    json!({"op":"replace","start":0,"end":1}),
                );
                system.insert("sourceEventSeqs".into(), json!([0, 1]));
            }
        }
        fails(restore(bad));
    }
    let mut decoder = decoder(false, SessionFormatRecovery::Recoverable);
    let mut out = SessionFormatEventCollector::new();
    for row in dense(opening()) {
        decoder.decode_row(Value::Object(row), &mut out).unwrap();
    }
    decoder.decode_row(Value::Null, &mut out).unwrap();
    let mut bad = request(Some("retired"));
    bad.insert("seq".into(), 2.into());
    err(
        decoder.decode_row(Value::Object(bad), &mut out),
        "header.system",
    );
}
#[test]
fn rejects_system_nodes_outside_the_open_step_and_mixed_head_replacements() {
    let a = migrate(with(opening(), [user("user"), request(Some("system"))])).unwrap();
    let mut wrong = a.clone();
    wrong.events[2]["data"]["step"] = 2.into();
    err(restore(wrong), "open step");
    let mut wrong = a;
    wrong.events[4].insert(
        "surfaceOp".into(),
        json!({"op":"replace","startSeq":2,"endSeq":3}),
    );
    wrong.events[4].insert("sourceEventSeqs".into(), json!([2, 3]));
    err(restore(wrong), "exactly");
}
#[test]
fn round_trips_in_history_system_append_replacement_and_compaction_without_shadowing_the_head() {
    let mut a = migrate(with(opening(), [user("user")])).unwrap();
    let mut append = a.events[2].clone();
    append.insert("seq".into(), 4.into());
    append["data"]["message"]["id"] = "tail".into();
    append["data"]["message"]["source"] = json!({"kind":"plugin","plugin":"context-plugin"});
    append["data"]["message"]["content"] = json!([{"type":"text","text":"tail context"},{"type":"reasoning","text":"retained content"}]);
    let mut replace = a.events[2].clone();
    replace.insert("seq".into(), 5.into());
    replace.insert(
        "surfaceOp".into(),
        json!({"op":"replace","startSeq":4,"endSeq":4}),
    );
    replace.insert("sourceEventSeqs".into(), json!([4]));
    let prune = e(
        "compaction/prune",
        6,
        json!({"shadowedRange":{"start":5,"end":5},"shadowedSeqs":[5],"shadowedTokenCount":0}),
    );
    let mut checkpoint = user("checkpoint");
    checkpoint.insert("seq".into(), 7.into());
    checkpoint.insert(
        "surfaceOp".into(),
        json!({"op":"replace","startSeq":5,"endSeq":5}),
    );
    checkpoint.insert("sourceEventSeqs".into(), json!([5]));
    a.events.extend([append, replace, prune, checkpoint]);
    assert_eq!(restore(a.clone()).unwrap(), a);
    assert_eq!(round_trip(a.clone()).unwrap(), a);
    a.events.truncate(7);
    a.events[6]["data"] =
        json!({"shadowedRange":{"start":2,"end":2},"shadowedSeqs":[2],"shadowedTokenCount":0});
    err(restore(a), "protected");
}
#[test]
fn keeps_native_ordinary_payload_and_message_source_extensions_distinct_from_structural_migration_admission()
 {
    let mut a = migrate(with(opening(), [user("user")])).unwrap();
    a.events[3]["data"]["installedExtension"] = true.into();
    a.events[3]["data"]["source"] = json!({"kind":"installed-source","revision":1});
    assert_eq!(restore(a.clone()).unwrap(), a);
    let mut bad = user("user");
    bad["data"]["installedExtension"] = true.into();
    err(migrate(with(opening(), [bad])), "unexpected");
}
#[test]
fn round_trips_native_header_extensions_while_refusing_them_as_unclassified_v2_input() {
    let mut a = migrate(opening()).unwrap();
    let mut row = request(None);
    row.insert("seq".into(), a.events.len().into());
    row["data"]["header"]["messagePrefix"] = json!(["current extension"]);
    a.events.push(row.clone());
    assert_eq!(round_trip(a.clone()).unwrap(), a);
    err(migrate(with(opening(), [row.clone()])), "unexpected");
    row["data"]["header"]["system"] = "retired".into();
    err(
        RELEASED_V3_SESSION_FORMAT_CODEC.encode_event(&row),
        "header.system",
    );
}
#[test]
fn retains_equal_generation_ignorable_events_but_rejects_unknown_required_events() {
    let mut row = ev("external/event", Value::Null);
    row.insert("ignorable".into(), true.into());
    let a = artifact(vec![row.clone()]);
    assert_eq!(restore(a.clone()).unwrap(), a);
    row.shift_remove("ignorable");
    err(restore(artifact(vec![row])), "unknown event");
}
#[test]
fn rejects_non_v3_physical_metadata() {
    for value in [Value::Null, json!([]), json!(false), json!({"version":2})] {
        err(
            RELEASED_V3_SESSION_FORMAT_CODEC.decode_header(&value),
            "format v3 physical",
        );
    }
}
#[test]
fn renames_ptc_after_system_insertion_and_remaps_local_references_without_touching_tool_json_or_identities()
 {
    let mut data = user_data("tools-code-mode:message");
    data["source"] = json!({"kind":"plugin","plugin":"tools-code-mode"});
    let mut dispatch = dispatch();
    dispatch["arguments"] = json!({"text":"tools-code-mode","type":"tool/code-dispatch"});
    let mut settle = dispatch.clone();
    settle["isError"] = false.into();
    settle["content"] = json!([{"type":"text","text":"tool/code-dispatch tools-code-mode"}]);
    let mut replacement = surface("user/message", {
        let mut d = user_data("tools-ptc:message");
        d["source"] = json!({"kind":"plugin","plugin":"tools-ptc"});
        d
    });
    replacement.insert(
        "surfaceOp".into(),
        json!({"op":"replace","start":2,"end":2}),
    );
    replacement.insert("sourceEventSeqs".into(), json!([2, 5]));
    let rows = dense(with(
        opening(),
        [
            surface("user/message", data.clone()),
            request(Some("prompt")),
            ev("tool/code-dispatch-start", dispatch.clone()),
            ev("tool/code-dispatch", settle.clone()),
            ev(
                "command/run",
                json!({"commandId":"cmd","name":"test","source":{"kind":"user"}}),
            ),
            ev(
                "command/done",
                json!({"commandId":"cmd","kind":"success","sourceEventSeq":5}),
            ),
            replacement,
            request(None),
        ],
    ));
    let before = hoh_util_values::js_json_stringify(&json!(rows));
    let a = migrate(rows.clone()).unwrap();
    let dispatched = a
        .events
        .iter()
        .find(|e| e["type"] == "tool/ptc-dispatch")
        .unwrap();
    assert_eq!(dispatched["data"], settle);
    assert_eq!(
        a.events
            .iter()
            .find(|e| e["type"] == "tool/ptc-dispatch-start")
            .unwrap()["data"],
        dispatch
    );
    assert_eq!(
        a.events
            .iter()
            .find(|e| e["type"] == "command/done")
            .unwrap()["data"]["sourceEventSeq"],
        dispatched["seq"]
    );
    let users: Vec<_> = a
        .events
        .iter()
        .filter(|e| e["type"] == "user/message")
        .collect();
    data["source"]["plugin"] = "tools-ptc".into();
    assert_eq!(users[0]["data"], data);
    assert_eq!(users[1]["data"]["source"]["plugin"], "tools-ptc");
    assert_eq!(
        users[1]["sourceEventSeqs"],
        json!([users[0]["seq"], dispatched["seq"]])
    );
    assert_eq!(requests(&a.events, 3), requests(&rows, 2));
    assert_eq!(hoh_util_values::js_json_stringify(&json!(rows)), before);
}
#[test]
fn renames_exact_attribution_slots_in_inbox_and_title_messages_but_not_similar_plugin_labels_or_text()
 {
    let message = |plugin: &str| {
        let mut d = user_data(plugin);
        d["source"] = json!({"kind":"plugin","plugin":plugin});
        d
    };
    let old = message("tools-code-mode");
    let untouched = message("tools-code-mode-extra");
    let mut title = title(3, json!([old, untouched]));
    title["data"]["system"] = "tools-code-mode".into();
    let rows = dense(with(
        with(
            vec![ev(
                "agent/inbox/spliced",
                json!({"target":"next-turn","start":0,"inserted":[old,untouched]}),
            )],
            opening(),
        ),
        [user("human"), title],
    ));
    let a = raw(rows, h(2, "identity"), None).unwrap();
    let mut renamed = old;
    renamed["source"]["plugin"] = "tools-ptc".into();
    assert_eq!(a.events[0]["data"]["inserted"], json!([renamed, untouched]));
    assert_eq!(
        a.events.last().unwrap()["data"]["messages"],
        json!([renamed, untouched])
    );
    assert_eq!(
        a.events.last().unwrap()["data"]["system"],
        "tools-code-mode"
    );
}
#[test]
fn refuses_both_required_and_ignorable_reserved_ptc_tags_in_all_sources() {
    for source_kind in [
        SessionFormatSourceKind::Decoded,
        SessionFormatSourceKind::Transformed,
    ] {
        for kind in ["tool/ptc-dispatch-start", "tool/ptc-dispatch"] {
            for ignorable in [false, true] {
                let mut s = stage(&h(2, "identity"), Some(0), source_kind).unwrap();
                err(
                    s.transform_event(
                        ev(kind, Value::Null),
                        &mut SessionFormatEventCollector::new(),
                    ),
                    "unclassified",
                );
                let mut row = ev(kind, Value::Null);
                if ignorable {
                    row.insert("ignorable".into(), true.into());
                }
                err(migrate(vec![row]), "unclassified");
            }
        }
    }
}
#[test]
fn refuses_retired_required_tags_after_recoverable_corruption() {
    for kind in ["tool/code-dispatch-start", "tool/code-dispatch"] {
        let mut d = decoder(false, SessionFormatRecovery::Recoverable);
        let mut out = SessionFormatEventCollector::new();
        d.decode_row(Value::Null, &mut out).unwrap();
        err(
            d.decode_row(Value::Object(e(kind, 0, Value::Null)), &mut out),
            "unknown event type",
        );
        err(
            RELEASED_V3_SESSION_FORMAT_CODEC.encode_event(&e(kind, 0, Value::Null)),
            "unknown event type",
        );
    }
}
#[test]
fn validates_a_frozen_complete_ptc_lifecycle_and_returns_original_names_ids_and_references() {
    let mut rows = ptc_native();
    rows.push(e(
        "turn/end",
        3,
        json!({"turn":1,"reason":{"kind":"completed"}}),
    ));
    let a = artifact(rows);
    let before = hoh_util_values::js_json_stringify(&json!(a.events));
    assert_eq!(
        restore_released_v3_artifact(
            a.clone(),
            &HashSet::from(["tool/ptc-dispatch-start".into(), "tool/ptc-dispatch".into()])
        )
        .unwrap(),
        a
    );
    assert_eq!(hoh_util_values::js_json_stringify(&json!(a.events)), before);
}
#[test]
fn accepts_an_unfinished_ptc_start_without_requiring_a_fabricated_settlement() {
    let mut rows = ptc_native();
    rows.truncate(2);
    let a = artifact(rows);
    assert_eq!(restore(a.clone()).unwrap(), a);
}
#[test]
fn rejects_an_orphan_ptc_settlement() {
    let mut rows = ptc_native();
    rows.remove(1);
    err(restore(artifact(dense(rows))), "no unique start");
}
#[test]
fn rejects_a_ptc_settlement_whose_identity_or_input_disagrees_with_its_start() {
    for (key, value) in [
        ("name", json!("other")),
        ("arguments", json!({"path":"different"})),
        ("subCallId", json!("other-child")),
        ("parentCallId", json!("missing-parent")),
        ("rootCallId", json!("different-root")),
    ] {
        let mut rows = ptc_native();
        rows[2]["data"][key] = value;
        let error = restore(artifact(rows)).unwrap_err().to_string();
        assert!(
            [
                "does not match",
                "no unique start",
                "parentCallId",
                "rootCallId"
            ]
            .iter()
            .any(|s| error.contains(s)),
            "{error}"
        );
    }
}
#[test]
fn rejects_ptc_events_outside_an_open_turn() {
    for mut row in ptc_native().into_iter().skip(1) {
        row.insert("seq".into(), 0.into());
        err(restore(artifact(vec![row])), "outside an open turn");
    }
}
#[test]
fn rejects_duplicate_ptc_starts_and_settlements() {
    let rows = ptc_native();
    let mut duplicate = rows[1].clone();
    duplicate.insert("seq".into(), 2.into());
    err(
        restore(artifact(vec![rows[0].clone(), rows[1].clone(), duplicate])),
        "repeats subCallId",
    );
    let mut duplicate = rows[2].clone();
    duplicate.insert("seq".into(), 3.into());
    err(
        restore(artifact(with(rows, [duplicate]))),
        "no unique start",
    );
}
#[test]
fn rejects_required_obsolete_tags_even_when_installed() {
    for kind in ["tool/code-dispatch-start", "tool/code-dispatch"] {
        err(
            restore_released_v3_artifact(
                artifact(vec![e(kind, 0, Value::Null)]),
                &HashSet::from([kind.into()]),
            ),
            "format v3 contains unknown event type",
        );
    }
}
#[test]
fn preserves_ignorable_tags_as_opaque_data_outside_a_turn() {
    for kind in [
        "tool/code-dispatch-start",
        "tool/code-dispatch",
        "external/future",
    ] {
        let mut row = e(
            kind,
            0,
            json!({"source":{"kind":"plugin","plugin":"tools-code-mode"},"invalidLifecycle":true,"content":["tool/code-dispatch"]}),
        );
        row.insert("time".into(), (-5).into());
        row.insert("ignorable".into(), true.into());
        let a = artifact(vec![row]);
        assert_eq!(restore(a.clone()).unwrap(), a);
        assert_eq!(a.events[0]["type"], kind);
    }
}
#[test]
fn does_not_let_an_ignorable_obsolete_start_satisfy_a_current_ptc_settlement() {
    let mut rows = ptc_native();
    rows[1].insert("type".into(), "tool/code-dispatch-start".into());
    rows[1].insert("ignorable".into(), true.into());
    err(restore(artifact(rows)), "no unique start");
}
