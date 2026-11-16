mod common;
use common::*;
use hoh_session_format::*;
use hoh_session_format_v0_to_v1::*;
use serde_json::{Value, json};
use std::collections::HashSet;

#[test]
fn changes_only_version_of_canonical_decoded_artifact() {
    let mut h = header(0, "identity");
    h["cwd"] = "/work".into();
    let mut t = turn(0, 1);
    t["time"] = 2.into();
    let mut st = step(1);
    st["time"] = 3.into();
    let rows = vec![
        t.clone(),
        st.clone(),
        json!({"type":"text-chunks","seq0":2,"time0":4,"data":{"turn":1,"step":1,"index":0,"dt":[1,1],"texts":["a","b","c"]}}),
    ];
    let migrated =
        testing::restore::restore_v0_to_v1(&h, &rows, SessionFormatRecovery::Strict).unwrap();
    assert_eq!(
        migrated.header,
        object(
            json!({"version":1,"id":"identity","createdAt":1,"cwd":"/work","isSeeded":false,"delegationDepth":0})
        )
    );
    assert_eq!(migrated.inherited_event_count, 0);
    let mut expected = vec![t, st];
    for (i, text) in ["a", "b", "c"].iter().enumerate() {
        let mut e = event(
            "assistant/chunk",
            i as u64 + 2,
            json!({"turn":1,"step":1,"chunk":{"type":"text-delta","index":0,"text":text}}),
        );
        e["time"] = (i + 4).into();
        expected.push(e);
    }
    assert_eq!(values(&migrated), expected);
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
fn expands_unrecognized_compact_run_through_identity_stage() {
    let mut st = stage("generic-run");
    let source = object(event("feedback/record", 0, json!({"text":"retained"})));
    let mut output = SessionFormatEventCollector::new();
    st.transform_run(Box::new(GenericRun(source.clone())), &mut output)
        .unwrap();
    assert_eq!(output.values, vec![source]);
}
#[test]
fn accepts_inherited_delivery_marker_for_source_generation() {
    let h = object(
        json!({"version":0,"id":"child","createdAt":1,"parentSession":"parent","isSeeded":true,"delegationDepth":0}),
    );
    let target = SESSION_FORMAT_V0_TO_V1.migrate_header(h.clone()).unwrap();
    let mut st = SESSION_FORMAT_V0_TO_V1
        .create_stage(SessionFormatMigrationStageInput {
            source_header: &h,
            target_header: &target,
            source_inherited_event_count: Some(2),
            source_kind: SessionFormatSourceKind::Decoded,
        })
        .unwrap();
    let marker = event(
        "session-log-deepseek/delivery-accepted",
        1,
        json!({"sessionId":"parent","throughSeq":0}),
    );
    assert_eq!(transform(st.as_mut(), marker.clone()).unwrap(), marker);
}
#[test]
fn recovers_only_complete_row_prefix_and_refuses_later_committing_turn_end() {
    let h = header(0, "recoverable");
    let prefix = vec![turn(0, 1), end(1, 1)];
    let bad = json!({"type":"text-chunks","seq0":7,"time0":4,"data":{"turn":2,"step":0,"index":0,"dt":[1],"texts":["x","y"]}});
    let mut rows = prefix.clone();
    rows.push(bad);
    assert_eq!(
        testing::restore::restore_v0_to_v1(&h, &rows, SessionFormatRecovery::Recoverable).unwrap(),
        testing::restore::restore_v0_to_v1(&h, &prefix, SessionFormatRecovery::Strict).unwrap()
    );
    rows.push(event(
        "turn/end",
        2,
        json!({"turn":2,"reason":{"kind":"interrupted"}}),
    ));
    assert_error(
        testing::restore::restore_v0_to_v1(&h, &rows, SessionFormatRecovery::Recoverable),
        "seq gap",
    );
}
#[test]
fn requires_depth_and_decodes_ranges_without_mutating_source_rows() {
    let h = header(0, "old");
    let source = merge(
        surface(
            "assistant/message",
            3,
            json!({"turn":1,"step":1,"message":{"id":"message","role":"assistant","content":[],"source":{"kind":"model","provider":"test","model":"test"}}}),
        ),
        json!({"sourceEventSeqs":[[0,2]]}),
    );
    let rows = vec![
        turn(0, 1),
        step(1),
        event(
            "assistant/chunk",
            2,
            json!({"turn":1,"step":1,"chunk":{"type":"text-delta","index":0,"text":"x"}}),
        ),
        source.clone(),
    ];
    let a = testing::restore::restore_v0_to_v1(&h, &rows, SessionFormatRecovery::Strict).unwrap();
    let mut incomplete = h;
    incomplete
        .as_object_mut()
        .unwrap()
        .shift_remove("delegationDepth");
    assert_error(
        testing::restore::restore_v0_to_v1(&incomplete, &rows, SessionFormatRecovery::Strict),
        "delegationDepth",
    );
    assert_eq!(a.header, logical(1, "old"));
    assert_eq!(a.events[3]["sourceEventSeqs"], json!([0, 1, 2]));
    assert_eq!(source["sourceEventSeqs"], json!([[0, 2]]));
}
#[test]
fn refuses_v1_only_generation_fields_in_v0_and_accepts_in_v1() {
    let rows = vec![
        turn(0, 1),
        event(
            "session-log-deepseek/delivery-accepted",
            1,
            json!({"sessionId":"delivery","throughSeq":0,"sessionFormatVersion":1}),
        ),
    ];
    assert_error(
        testing::restore::restore_v0_to_v1(
            &header(0, "delivery"),
            &rows,
            SessionFormatRecovery::Strict,
        ),
        "unexpected member \"sessionFormatVersion\"",
    );
    assert_eq!(
        values(
            &testing::restore::restore_v1(
                &header(1, "delivery"),
                &rows,
                SessionFormatRecovery::Strict
            )
            .unwrap()
        ),
        rows
    );
}
#[test]
fn does_not_synthesize_over_explicitly_invalid_retry_id() {
    let mut r = retry(3, 1, "r", "k");
    r["data"]["retryId"] = Value::Null;
    assert_error(migrate(&[turn(0, 1), step(1), request(2), r]), "retryId");
    let mut st = stage("retry");
    for (seq, attempt) in [(0, 1), (1, 2)] {
        let mut r = retry(seq, attempt, "r", "k");
        r["data"].as_object_mut().unwrap().shift_remove("retryId");
        assert_eq!(
            transform(st.as_mut(), r).unwrap()["data"]["retryId"],
            "legacy-retry:retry:0"
        );
    }
}
#[test]
fn normalizes_legacy_compaction_names_and_assigns_one_deterministic_group_id() {
    let rows = [
        event("compaction/start", 0, json!({"turn":null})),
        event(
            "compaction/summary",
            1,
            json!({"summary":[{"type":"text","text":"summary"}],"shadowedRange":{"start":0,"end":0},"shadowedSeqs":[0],"shadowedTokenCount":0,"provider":"p","model":"m"}),
        ),
        event(
            "compaction/prune",
            2,
            json!({"shadowedRange":{"start":0,"end":0},"shadowedSeqs":[0],"shadowedTokenCount":1}),
        ),
        merge(
            surface(
                "user/message",
                3,
                user_source("checkpoint", json!({"kind":"plugin","plugin":"compact"})),
            ),
            json!({"sourceEventSeqs":[0,1],"surfaceOp":{"op":"replace","start":0,"end":0}}),
        ),
        event("compaction/end", 4, json!({"turn":null})),
    ];
    let mut st = stage("compact");
    let migrated: Vec<_> = rows
        .iter()
        .map(|r| transform(st.as_mut(), r.clone()).unwrap())
        .collect();
    for (i, e) in migrated.iter().enumerate() {
        let id = if i == 3 {
            &e["data"]["source"]["compactionId"]
        } else {
            &e["data"]["compactionId"]
        };
        let expected = if i == 2 {
            Value::Null
        } else {
            json!("legacy-compaction:compact:0")
        };
        assert_eq!(id, &expected);
    }
    let mut historical = stage("compact");
    for (r, expected) in rows.iter().zip(&migrated) {
        let mut r = r.clone();
        r["type"] = r["type"]
            .as_str()
            .unwrap()
            .replacen("compaction/", "compact/", 1)
            .into();
        assert_eq!(&transform(historical.as_mut(), r).unwrap(), expected);
    }
    let mut reset = stage("compact");
    let mut invalid = rows[0].clone();
    invalid["data"]["compactionId"] = Value::Null;
    assert_error(transform(reset.as_mut(), invalid), "compactionId");
    transform(reset.as_mut(), rows[0].clone()).unwrap();
    assert_eq!(
        transform(
            reset.as_mut(),
            event("feedback/record", 1, json!({"text":"kept"}))
        )
        .unwrap()["type"],
        "feedback/record"
    );
    transform(reset.as_mut(), event("session/end-seed", 2, json!({}))).unwrap();
    let mut r = rows[1].clone();
    r["seq"] = 3.into();
    assert_error(transform(reset.as_mut(), r), "compactionId");
}
#[test]
fn preserves_complete_canonical_multi_owner_log_except_header_version() {
    let mut am = assistant();
    am["id"] = "assistant".into();
    am["content"] = json!([{"type":"text","text":"hello"},{"type":"tool-call","id":"call","name":"read","arguments":"{}"}]);
    let mut tm = tool();
    tm["id"] = "result".into();
    tm["content"][0]["toolCallId"] = "call".into();
    tm["source"]["callId"] = "call".into();
    tm["content"][0]["content"] = json!([{"type":"text","text":"ok"}]);
    let mut summary = summary(15);
    summary["data"] = merge(
        summary["data"].clone(),
        json!({"compactionId":"compact","sourceCommandId":"command","shadowedRange":{"start":1,"end":6},"shadowedSeqs":[1,4,6],"shadowedTokenCount":10,"provider":"mock","model":"mock","rawOutput":[{"type":"text","text":"summary"}],"llmStreamCall":true}),
    );
    let rows = vec![
        turn(0, 1),
        surface("user/message", 1, user("human")),
        step(2),
        event(
            "assistant/chunk",
            3,
            json!({"turn":1,"step":1,"chunk":{"type":"text-delta","index":0,"text":"hello"}}),
        ),
        merge(
            surface(
                "assistant/message",
                4,
                json!({"turn":1,"step":1,"message":am}),
            ),
            json!({"sourceEventSeqs":[3]}),
        ),
        event(
            "tool/call",
            5,
            json!({"turn":1,"step":1,"callId":"call","name":"read","arguments":"{}"}),
        ),
        merge(
            surface(
                "tool/result",
                6,
                json!({"turn":1,"step":1,"message":tm,"meta":{"opaque":{"seq":999}}}),
            ),
            json!({"sourceEventSeqs":[5]}),
        ),
        ptc(7, false),
        ptc(8, true),
        step_end(9),
        event(
            "session/title",
            10,
            json!({"title":"Title","messageSeqs":[1],"source":{"kind":"fallback"}}),
        ),
        event(
            "command/run",
            11,
            json!({"commandId":"command","name":"compact","source":{"kind":"user"}}),
        ),
        event("feedback/record", 12, json!({"text":"feedback"})),
        event(
            "command/done",
            13,
            json!({"commandId":"command","kind":"success","sourceEventSeq":12}),
        ),
        event(
            "compaction/start",
            14,
            json!({"compactionId":"compact","sourceCommandId":"command","turn":1}),
        ),
        summary,
        merge(
            surface(
                "user/message",
                16,
                user_source(
                    "checkpoint",
                    json!({"kind":"plugin","plugin":"compact","compactionId":"compact","sourceCommandId":"command"}),
                ),
            ),
            json!({"sourceEventSeqs":[1,4,6],"surfaceOp":{"op":"replace","start":1,"end":6}}),
        ),
        event(
            "compaction/end",
            17,
            json!({"compactionId":"compact","sourceCommandId":"command","turn":1}),
        ),
        event(
            "session-log-deepseek/delivery-accepted",
            18,
            json!({"sessionId":"full-identity","throughSeq":17}),
        ),
        end(19, 1),
    ];
    let h = merge(header(0, "full-identity"), json!({"cwd":"/work"}));
    let a = testing::restore::restore_v0_to_v1(&h, &rows, SessionFormatRecovery::Strict).unwrap();
    assert_eq!(values(&a), rows);
    assert_eq!(a.inherited_event_count, 0);
    assert_eq!(
        a.header,
        object(
            json!({"version":1,"id":"full-identity","createdAt":1,"cwd":"/work","isSeeded":false,"delegationDepth":0})
        )
    );
}
#[test]
fn keeps_v1_physical_codec_vocabulary_neutral_for_current_growth_and_future_source_freeze() {
    let ordinary = event("ordinary/post-v1", 0, json!({"required":true}));
    let mut d = RELEASED_V1_SESSION_FORMAT_CODEC
        .create_decoder(&header(1, "ordinary-growth"), SessionFormatRecovery::Strict)
        .unwrap();
    let mut c = SessionFormatEventCollector::new();
    d.decode_row(ordinary.clone(), &mut c).unwrap();
    let a = SessionFormatArtifact {
        header: d.header().clone(),
        inherited_event_count: d.finish(&mut c).unwrap(),
        events: c.values,
    };
    assert_eq!(values(&a), vec![ordinary]);
    assert!(!RELEASED_V0_EVENT_TYPES.contains(&"ordinary/post-v1"));
    let mut types: HashSet<String> = RELEASED_V0_EVENT_TYPES
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    types.insert("ordinary/post-v1".into());
    restore_released_v1_artifact(a.clone(), &types).unwrap();
    restore_released_v1_artifact(a, &types.clone()).unwrap();
    let mut d = RELEASED_V1_SESSION_FORMAT_CODEC
        .create_decoder(&header(1, "ordinary-growth"), SessionFormatRecovery::Strict)
        .unwrap();
    let mut c = SessionFormatEventCollector::new();
    let extended = event("turn/start", 0, json!({"turn":1,"postReleaseMember":true}));
    d.decode_row(extended.clone(), &mut c).unwrap();
    let a = SessionFormatArtifact {
        header: d.header().clone(),
        inherited_event_count: d.finish(&mut c).unwrap(),
        events: c.values,
    };
    assert_eq!(values(&a), vec![extended]);
    restore_released_v1_artifact(a, &types).unwrap();
}
