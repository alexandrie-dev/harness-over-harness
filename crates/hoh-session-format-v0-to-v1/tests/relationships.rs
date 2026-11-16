mod common;
use common::*;
use hoh_session_format::*;
use hoh_session_format_v0_to_v1::*;
use serde_json::{Value, json};

#[test]
fn refuses_invalid_whole_artifact_relationships() {
    let replacement = merge(
        surface("user/message", 1, user("two")),
        json!({"surfaceOp":{"op":"replace","start":0,"end":0}}),
    );
    let bad_stats = json!({"kind":"session-reference","form":"recall","version":1,"references":[{"sessionId":"source","label":"Source","capturedThroughSeq":null,"compacted":false,"originalMessages":1,"retainedMessages":2,"omittedMessages":0,"omittedBytes":0,"truncated":false,"inputIndex":0}]});
    let cases = [
        vec![end(0, 1)],
        vec![
            turn(0, 1),
            step(1),
            event(
                "llm/retry-started",
                2,
                json!({"retryId":"retry","turn":1,"step":1,"retry":1}),
            ),
        ],
        vec![
            turn(0, 1),
            event(
                "session/title",
                1,
                json!({"title":"Bad","messageSeqs":[0],"source":{"kind":"fallback"}}),
            ),
        ],
        vec![compaction_end(0)],
        vec![ptc(0, false)],
        vec![surface("user/message", 0, user("one")), replacement],
        vec![surface(
            "user/message",
            0,
            user_source("reference", bad_stats),
        )],
    ];
    for rows in cases {
        assert!(decode(&rows).is_err(), "{rows:?}");
    }
}
#[test]
fn refuses_relative_restored_cwd() {
    assert_error(
        testing::restore::restore_v1(
            &merge(header(1, "relationships"), json!({"cwd":"relative"})),
            &[],
            SessionFormatRecovery::Strict,
        ),
        "cwd must be absolute",
    );
}
#[test]
fn accepts_complete_core_retry_title_compaction_ptc_and_replacement_relationships() {
    let mut finish = compaction_end(11);
    finish["data"]["error"] = "skipped".into();
    let rows = vec![
        turn(0, 1),
        surface("user/message", 1, user("one")),
        step(2),
        request(3),
        retry(4, 1, "retry", "default"),
        event(
            "llm/retry-started",
            5,
            json!({"retryId":"retry","turn":1,"step":1,"retry":1}),
        ),
        ptc(6, false),
        ptc(7, true),
        step_end(8),
        event(
            "session/title",
            9,
            json!({"title":"Title","messageSeqs":[1],"source":{"kind":"fallback"}}),
        ),
        compaction_start(10, json!(1)),
        finish,
        merge(
            surface("user/message", 12, user("replacement")),
            json!({"sourceEventSeqs":[1],"surfaceOp":{"op":"replace","start":1,"end":1}}),
        ),
        end(13, 1),
    ];
    assert_eq!(values(&decode(&rows).unwrap()), rows);
}
#[test]
fn accepts_compaction_surface_span_whose_sequence_values_descend() {
    let mut sum = summary(6);
    sum["data"]["shadowedRange"] = json!({"start":4,"end":3});
    sum["data"]["shadowedSeqs"] = json!([4, 3]);
    sum["data"]["shadowedTokenCount"] = 2.into();
    let rows = vec![
        turn(0, 1),
        surface("user/message", 1, user("zero")),
        surface("user/message", 2, user("one")),
        surface("user/message", 3, user("two")),
        merge(
            surface("user/message", 4, user("prior checkpoint")),
            json!({"sourceEventSeqs":[1,2],"surfaceOp":{"op":"replace","start":1,"end":2}}),
        ),
        compaction_start(5, json!(1)),
        sum,
        merge(
            surface(
                "user/message",
                7,
                user_source(
                    "checkpoint",
                    json!({"kind":"plugin","plugin":"compact","compactionId":"c"}),
                ),
            ),
            json!({"sourceEventSeqs":[5,6,4,3],"surfaceOp":{"op":"replace","start":4,"end":3}}),
        ),
        compaction_end(8),
        end(9, 1),
    ];
    assert_eq!(values(&decode(&rows).unwrap()), rows);
}
#[test]
fn keeps_latest_request_provider_across_later_steps_and_turns() {
    let mut st = step(6);
    st["data"]["turn"] = 2.into();
    let mut r = retry(7, 1, "retry", "default");
    r["data"]["turn"] = 2.into();
    let mut se = step_end(9);
    se["data"]["turn"] = 2.into();
    let rows = vec![
        turn(0, 1),
        step(1),
        request(2),
        step_end(3),
        end(4, 1),
        turn(5, 2),
        st,
        r,
        event(
            "llm/retry-started",
            8,
            json!({"retryId":"retry","turn":2,"step":1,"retry":1}),
        ),
        se,
        end(10, 2),
    ];
    assert_eq!(values(&decode(&rows).unwrap()), rows);
}
#[test]
fn preserves_merge_extensible_nested_variants_and_ignorable_current_events() {
    let rows = vec![
        turn(0, 1),
        surface(
            "user/message",
            1,
            json!({"id":"plugin-message","role":"user","content":[{"type":"plugin/block","value":{"nested":true}}],"source":{"kind":"plugin/source","detail":"opaque"}}),
        ),
        step(2),
        event(
            "assistant/chunk",
            3,
            json!({"turn":1,"step":1,"chunk":{"type":"finish","reason":{"kind":"plugin-finish","detail":1}}}),
        ),
        step_end(4),
        event(
            "turn/end",
            5,
            json!({"turn":1,"reason":{"kind":"plugin-turn","detail":1}}),
        ),
        merge(
            event("plugin/informational", 6, json!({"value":1})),
            json!({"ignorable":true}),
        ),
    ];
    assert_eq!(values(&decode(&rows).unwrap()), rows);
    assert!(
        decode(&[event("plugin/required", 0, json!({}))])
            .unwrap_err()
            .is_unsupported_migration()
    );
    for data in [json!("value"), json!([1]), Value::Null] {
        assert_eq!(
            decode(&[merge(
                event("plugin/scalar", 0, data.clone()),
                json!({"ignorable":true})
            )])
            .unwrap()
            .events[0]["data"],
            data
        );
    }
}
fn advertised() -> Value {
    let mut m = assistant();
    m["id"] = "assistant-tools".into();
    m["content"] = json!([{"type":"tool-call","id":"a","name":"read","arguments":"{}"}]);
    surface(
        "assistant/message",
        2,
        json!({"turn":1,"step":1,"message":m}),
    )
}
fn started(seq: u64) -> Value {
    event(
        "tool/call",
        seq,
        json!({"turn":1,"step":1,"callId":"a","name":"read","arguments":"{}"}),
    )
}
fn result(seq: u64, source: &str, block: &str) -> Value {
    surface(
        "tool/result",
        seq,
        json!({"turn":1,"step":1,"message":{"id":"result","role":"user","content":[{"type":"tool-result","toolCallId":block,"content":[],"isError":false}],"source":{"kind":"tool","callId":source}}}),
    )
}
#[test]
fn binds_advertised_started_repaired_and_resolved_tool_lifecycles() {
    let prefix = vec![turn(0, 1), step(1)];
    let check = |tail: Vec<Value>, text| {
        let mut rows = prefix.clone();
        rows.extend(tail);
        assert_error(decode(&rows), text);
    };
    let mut wrong = started(3);
    wrong["data"]["name"] = "write".into();
    check(vec![advertised(), wrong], "advertised tool call");
    check(vec![started(2)], "advertised tool call");
    check(vec![result(2, "a", "a")], "no advertised tool lifecycle");
    let mut duplicate = advertised();
    duplicate["data"]["message"]["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"tool-call","id":"a","name":"read","arguments":"{}"}));
    check(vec![duplicate], "repeats advertised tool call");
    check(
        vec![advertised(), started(3), result(4, "b", "a")],
        "tool-result block",
    );
    check(vec![advertised(), step_end(3)], "unresolved tool call");
    check(
        vec![advertised(), started(3), end(4, 1)],
        "unresolved tool call",
    );
    for tail in [
        vec![advertised()],
        vec![advertised(), started(3)],
        vec![advertised(), started(3), result(4, "a", "a")],
    ] {
        let expected = tail.len() + 2;
        let mut rows = prefix.clone();
        rows.extend(tail);
        assert_eq!(decode(&rows).unwrap().events.len(), expected);
    }
    let mut repair = result(3, "a", "a");
    repair["data"]["message"]["id"] = "interrupted-tool-result-a-3".into();
    repair["data"]["message"]["content"][0]["isError"] = true.into();
    repair["data"]["message"]["content"][0]["content"] = json!([{"type":"text","text":"The tool call was interrupted before the Harness recorded it as started. Retry it if it is still needed."}]);
    repair["data"]["error"] = json!({"name":"ToolNotStartedError","code":"TOOL_NOT_STARTED"});
    let mut rows = prefix.clone();
    rows.extend([advertised(), repair.clone()]);
    assert_eq!(decode(&rows).unwrap().events.len(), 4);
    repair["data"]["error"]["name"] = "Other".into();
    check(vec![advertised(), repair], "TOOL_NOT_STARTED repair");
}
#[test]
fn enforces_retry_mode_failure_route_and_policy_chain_relationships() {
    let prefix = vec![turn(0, 1), step(1), request(2)];
    let check = |extra: Value, text: &str| {
        let mut rows = prefix.clone();
        let mut r = retry(3, 1, "r", "k");
        r["data"] = merge(r["data"].clone(), extra);
        rows.push(r);
        assert_error(decode(&rows), text);
    };
    let mut r = retry(3, 1, "r", "k");
    r["data"]
        .as_object_mut()
        .unwrap()
        .shift_remove("maxRetries");
    let mut rows = prefix.clone();
    rows.push(r);
    assert!(decode(&rows).is_err());
    for (extra, text) in [
        (json!({"retry":2,"maxRetries":1}), "exceeds"),
        (json!({"turn":2}), "current turn"),
        (json!({"provider":"q"}), "provider"),
        (
            json!({"failure":{"message":"x","code":"X","status":99}}),
            "status",
        ),
        (json!({"delayMs":-0.5}), "non-negative"),
        (json!({"delayMs":2147483648_u64}), "timer"),
        (
            json!({"failure":{"message":"x","code":"X","providerRetryAfterMs":0}}),
            "positive",
        ),
    ] {
        check(extra, text);
    }
    for extra in [
        json!({"delayMs":1.5}),
        json!({"failure":{"message":"x","code":"X","providerRetryAfterMs":1.5}}),
    ] {
        let mut rows = prefix.clone();
        let mut r = retry(3, 1, "r", "k");
        r["data"] = merge(r["data"].clone(), extra);
        rows.push(r);
        assert_eq!(decode(&rows).unwrap().events.len(), 4);
    }
}
#[test]
fn accepts_legacy_retry_scheduled_immediately_after_step_closes() {
    let rows = vec![
        turn(0, 1),
        step(1),
        request(2),
        step_end(3),
        retry(4, 1, "r", "k"),
        end(5, 1),
    ];
    assert_eq!(decode(&rows).unwrap().events.len(), rows.len());
}
#[test]
fn enforces_command_pairing_and_authoritative_source_event_rules() {
    assert_error(
        decode(&[event(
            "command/done",
            0,
            json!({"commandId":"c","kind":"success"}),
        )]),
        "no prior command/run",
    );
    let run = event(
        "command/run",
        0,
        json!({"commandId":"c","name":"x","source":{"kind":"user"}}),
    );
    for kind in ["error", "success"] {
        assert_error(
            decode(&[
                run.clone(),
                event(
                    "command/done",
                    1,
                    json!({"commandId":"c","kind":kind,"text":"x","sourceEventSeq":0}),
                ),
            ]),
            "sourceEventSeq",
        );
    }
    assert_eq!(
        decode(&[
            run,
            event("command/done", 1, json!({"commandId":"c","kind":"success"}))
        ])
        .unwrap()
        .events
        .len(),
        2
    );
}
fn title_request() -> Value {
    event(
        "session/title-llm-request",
        2,
        json!({"titleProvider":"p","messageSeqs":[1],"route":{"provider":"p","model":"m"},"system":"s","maxTokens":1,"messages":[user_source("framed",json!({"kind":"plugin","plugin":"dsh-session-title-llm"}))]}),
    )
}
#[test]
fn enforces_title_source_cardinality_and_exact_auxiliary_framing() {
    let prefix = [turn(0, 1), surface("user/message", 1, user("human"))];
    for (seqs, kind) in [(json!([]), "fallback"), (json!([1]), "user")] {
        let mut rows = prefix.to_vec();
        rows.push(event(
            "session/title",
            2,
            json!({"title":"bad","messageSeqs":seqs,"source":{"kind":kind}}),
        ));
        assert_error(decode(&rows), "empty exactly");
    }
    let mut rows = prefix.to_vec();
    rows.push(title_request());
    assert_error(decode(&rows), "do not represent");
}
#[test]
fn validates_own_delivery_ids_while_preserving_inherited_ancestor_markers() {
    let marker = |id: Value, version: u64, through: Value| {
        event(
            "session-log-deepseek/delivery-accepted",
            1,
            json!({"sessionId":id,"sessionFormatVersion":version,"throughSeq":through}),
        )
    };
    assert_error(
        decode(&[turn(0, 1), marker("wrong".into(), 1, 0.into())]),
        "wrong Session",
    );
    let h = merge(
        header(1, "child"),
        json!({"parentSession":"parent","seedLength":2,"delegationDepth":1}),
    );
    assert_eq!(
        testing::restore::restore_v1(
            &h,
            &[turn(0, 1), marker("ancestor-of-parent".into(), 1, 0.into())],
            SessionFormatRecovery::Strict
        )
        .unwrap()
        .events
        .len(),
        2
    );
    let inert = marker(9.into(), 0, json!({"futureCoordinate":true}));
    let a = decode(&[turn(0, 1), inert.clone()]).unwrap();
    assert_eq!(Value::Object(a.events[1].clone()), inert);
    assert_released_artifact_relationships(
        &a,
        &ReleasedRelationshipExtensions {
            legacy_interrupted_turn_restart: true,
            ..Default::default()
        },
    )
    .unwrap();
}
#[test]
fn validates_versioned_subagent_descriptors_by_source_current_policy() {
    let future = event("subagent/descriptor", 0, json!({"version":4,"future":true}));
    assert_eq!(
        values(&decode(std::slice::from_ref(&future)).unwrap()),
        vec![future.clone()]
    );
    assert!(migrate(&[future]).unwrap_err().is_unsupported_migration());
}
#[test]
fn enforces_compaction_ownership_summaries_turn_boundaries_and_surface_spans() {
    assert_error(
        decode(&[turn(0, 1), compaction_start(1, Value::Null)]),
        "open turn",
    );
    assert_error(
        decode(&[
            turn(0, 1),
            compaction_start(1, 1.into()),
            compaction_start(2, 1.into()),
        ]),
        "overlaps",
    );
    let mut wrong = compaction_end(2);
    wrong["data"]["turn"] = 2.into();
    wrong["data"]["error"] = "x".into();
    assert_error(
        decode(&[turn(0, 1), compaction_start(1, 1.into()), wrong]),
        "owner turn",
    );
    assert_error(
        decode(&[turn(0, 1), compaction_start(1, 1.into()), compaction_end(2)]),
        "requires one summary",
    );
    assert_error(
        decode(&[turn(0, 1), compaction_start(1, 1.into()), end(2, 1)]),
        "crosses",
    );
    let mut bad_sum = summary(2);
    bad_sum["data"]["shadowedRange"] = json!({"start":0,"end":0});
    bad_sum["data"]["shadowedSeqs"] = json!([0]);
    assert_error(
        decode(&[turn(0, 1), compaction_start(1, 1.into()), bad_sum]),
        "surface span",
    );
    assert_error(
        decode(&[
            turn(0, 1),
            surface("user/message", 1, user("one")),
            compaction_start(2, 1.into()),
            summary(3),
            summary(4),
        ]),
        "repeats",
    );
    assert_error(
        decode(&[
            turn(0, 1),
            event(
                "compaction/prune",
                1,
                json!({"shadowedRange":{"start":0,"end":0},"shadowedSeqs":[0],"shadowedTokenCount":1}),
            ),
        ]),
        "surface span",
    );
    let checkpoint = merge(
        surface(
            "user/message",
            2,
            user_source(
                "checkpoint",
                json!({"kind":"plugin","plugin":"compact","compactionId":"missing"}),
            ),
        ),
        json!({"sourceEventSeqs":[1],"surfaceOp":{"op":"replace","start":1,"end":1}}),
    );
    assert!(
        decode(&[
            turn(0, 1),
            surface("user/message", 1, user("base")),
            checkpoint
        ])
        .is_err()
    );
}
#[test]
fn requires_each_ptc_settle_to_match_exactly_one_start() {
    for extra in [json!({"name":"b"}), json!({"arguments":{"x":2}})] {
        let mut settle = ptc(2, true);
        settle["data"] = merge(settle["data"].clone(), extra);
        assert_error(
            decode(&[turn(0, 1), ptc(1, false), settle]),
            "does not match",
        );
    }
    assert_error(
        decode(&[turn(0, 1), ptc(1, false), ptc(2, true), ptc(3, true)]),
        "unique start",
    );
    assert_eq!(
        decode(&[turn(0, 1), ptc(1, false)]).unwrap().events.len(),
        2
    );
    let mut start = ptc(1, false);
    let mut settle = ptc(2, true);
    for e in [&mut start, &mut settle] {
        e["data"]["subCallId"] = "array".into();
        e["data"]["arguments"] = json!([1,{"x":2}]);
    }
    assert_eq!(
        decode(&[turn(0, 1), start.clone(), settle.clone()])
            .unwrap()
            .events
            .len(),
        3
    );
    settle["data"]["arguments"] = json!([1,{"x":2},3]);
    assert_error(decode(&[turn(0, 1), start, settle]), "does not match");
}
#[test]
fn refuses_wrong_own_session_v0_delivery_marker_before_header_bump() {
    assert_error(
        migrate(&[
            turn(0, 1),
            event(
                "session-log-deepseek/delivery-accepted",
                1,
                json!({"sessionId":"wrong","throughSeq":0}),
            ),
        ]),
        "wrong Session",
    );
}
#[test]
fn accepts_released_interrupted_turn_restart_sequence_during_v0_migration() {
    let mut rows = vec![
        turn(0, 1),
        step(1),
        step_end(2),
        event(
            "agent/inbox/spliced",
            3,
            json!({"target":"next-turn","start":0,"inserted":[user("queued")]}),
        ),
        turn(4, 2),
        end(5, 2),
    ];
    assert_eq!(values(&migrate(&rows).unwrap()), rows);
    rows[3]["data"]["inserted"] = json!([]);
    assert_error(migrate(&rows), "does not open expected turn");
}
#[test]
fn accepts_recoverable_open_tails_for_later_interrupted_turn_repair() {
    let mut ad = advertised();
    ad["seq"] = 4.into();
    ad["time"] = 5.into();
    ad["data"]["message"]["content"][0]["id"] = "call".into();
    let mut call = started(5);
    call["data"]["callId"] = "call".into();
    let rows = vec![
        turn(0, 1),
        step(1),
        request(2),
        retry(3, 1, "r", "k"),
        ad,
        call,
        ptc(6, false),
        compaction_start(7, 1.into()),
    ];
    assert_eq!(
        testing::restore::restore_v0_to_v1(
            &header(0, "relationships"),
            &rows,
            SessionFormatRecovery::Recoverable
        )
        .unwrap()
        .events
        .len(),
        rows.len()
    );
}
#[test]
fn refuses_invalid_core_openings_request_placement_and_replacement_placement() {
    assert_error(decode(&[turn(0, 1), turn(1, 2)]), "expected turn");
    let mut st = step(1);
    st["data"]["step"] = 2.into();
    assert_error(decode(&[turn(0, 1), st]), "next step");
    assert_error(decode(&[request(0)]), "outside an open turn");
    assert_error(
        decode(&[event(
            "request/context",
            0,
            json!({"provider":"p","model":"m"}),
        )]),
        "outside an open turn",
    );
    assert_error(
        decode(&[turn(0, 1), step(1), end(2, 1)]),
        "crosses an open step",
    );
    let base = surface("user/message", 0, user("base"));
    let replacement = merge(
        result(1, "a", "a"),
        json!({"sourceEventSeqs":[0],"surfaceOp":{"op":"replace","start":0,"end":0}}),
    );
    assert_error(decode(&[base.clone(), replacement]), "outside an open turn");
    let mut without = base.clone();
    without.as_object_mut().unwrap().shift_remove("surfaceOp");
    assert_error(decode(&[without]), "surfaceOp");
    assert_error(
        decode(&[
            base,
            event("feedback/record", 1, json!({"text":"log-only"})),
            merge(
                surface("user/message", 2, user("missing")),
                json!({"sourceEventSeqs":[1],"surfaceOp":{"op":"replace","start":1,"end":1}}),
            ),
        ]),
        "not on the current surface",
    );
}
#[test]
fn rejects_ptc_ancestry_root_start_violations() {
    let mut bad = ptc(1, false);
    bad["data"]["parentCallId"] = "missing".into();
    assert_error(decode(&[turn(0, 1), bad]), "parentCallId");
    let mut bad = ptc(2, false);
    bad["data"]["rootCallId"] = "other".into();
    assert_error(decode(&[turn(0, 1), ptc(1, false), bad]), "rootCallId");
    assert_error(
        decode(&[turn(0, 1), ptc(1, false), ptc(2, false)]),
        "repeats subCallId",
    );
}
#[test]
fn rejects_repeated_retry_starts_and_invalid_retry_chains() {
    let prefix = vec![turn(0, 1), step(1), request(2)];
    let started = |seq, step| {
        event(
            "llm/retry-started",
            seq,
            json!({"retryId":"r","turn":1,"step":step,"retry":1}),
        )
    };
    let check = |tail: Vec<Value>, text| {
        let mut rows = prefix.clone();
        rows.extend(tail);
        assert_error(decode(&rows), text);
    };
    check(vec![retry(3, 1, "r", "k"), started(4, 2)], "scheduled turn");
    check(
        vec![retry(3, 1, "r", "k"), started(4, 1), started(5, 1)],
        "repeats",
    );
    check(vec![retry(3, 2, "r", "k")], "retry 1");
    check(
        vec![retry(3, 1, "r", "k"), retry(4, 2, "other", "k")],
        "preserve retryId",
    );
    check(
        vec![retry(3, 1, "r", "k"), retry(4, 1, "r", "other")],
        "reuses retryId",
    );
    let mut rows = prefix;
    rows.extend([retry(3, 1, "r", "k"), retry(4, 2, "r", "k")]);
    assert_eq!(decode(&rows).unwrap().events.len(), 5);
}
#[test]
fn rejects_duplicate_commands_and_clears_inherited_orphan_compactions_at_end_seed() {
    let run = |seq| {
        event(
            "command/run",
            seq,
            json!({"commandId":"c","name":"x","source":{"kind":"user"}}),
        )
    };
    assert_error(decode(&[run(0), run(1)]), "repeats commandId");
    let rows = vec![
        compaction_start(0, Value::Null),
        event("session/end-seed", 1, json!({})),
        turn(2, 1),
    ];
    assert_eq!(values(&decode(&rows).unwrap()), rows);
    assert_eq!(
        decode(&[event("session/end-seed", 0, json!({}))])
            .unwrap()
            .events
            .len(),
        1
    );
}
#[test]
fn accepts_exact_title_llm_framing_and_rejects_non_human_citations() {
    let human = surface("user/message", 1, user("human"));
    let mut request = title_request();
    request["data"]["messages"][0]["content"][0]["text"] = "Generate the session title from this JSON array of human messages:\n[{\"seq\":1,\"text\":\"human\"}]".into();
    assert_eq!(
        decode(&[turn(0, 1), human.clone(), request.clone()])
            .unwrap()
            .events
            .len(),
        3
    );
    let mut mixed = human.clone();
    mixed["data"]["content"] =
        json!([{"type":"reasoning","text":"hidden"},{"type":"text","text":"visible"}]);
    let mut mr = request.clone();
    mr["data"]["messages"][0]["content"][0]["text"] = "Generate the session title from this JSON array of human messages:\n[{\"seq\":1,\"text\":\"visible\"}]".into();
    assert_eq!(decode(&[turn(0, 1), mixed, mr]).unwrap().events.len(), 3);
    assert_error(
        decode(&[
            turn(0, 1),
            surface(
                "user/message",
                1,
                user_source("plugin", json!({"kind":"plugin","plugin":"x"})),
            ),
            event(
                "session/title",
                2,
                json!({"title":"x","messageSeqs":[1],"source":{"kind":"fallback"}}),
            ),
        ]),
        "human user",
    );
    request["data"]["messages"] = json!([]);
    assert_error(decode(&[turn(0, 1), human, request]), "do not represent");
}
#[test]
fn accepts_turn_enclosed_request_context_and_refuses_step_work_without_open_step() {
    assert_eq!(
        decode(&[
            turn(0, 1),
            event("request/context", 1, json!({"provider":"p","model":"m"}))
        ])
        .unwrap()
        .events
        .len(),
        2
    );
    assert_error(
        decode(&[
            turn(0, 1),
            event(
                "assistant/chunk",
                1,
                json!({"turn":1,"step":1,"chunk":{"type":"text-delta","index":0,"text":"x"}}),
            ),
        ]),
        "open turn and step",
    );
}
