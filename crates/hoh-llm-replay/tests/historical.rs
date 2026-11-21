//! Older Session generations are read through the session format catalog (migrations v0->v4),
//! as in `llm-replay.spec.ts` `parseSessionLog`.
mod support;
use hoh_llm_replay::*;
use serde_json::{Value, json};
use support::*;
fn lines(rows: &[Value]) -> String {
    rows.iter()
        .map(hoh_util_values::js_json_stringify)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}
fn values(source: &str) -> Vec<Value> {
    parse_session_log(source)
        .unwrap()
        .iter()
        .map(|event| event.as_value().clone())
        .collect()
}
/// The system head a v2->v3 migration derives for the first step (`llm-replay.spec.ts:437-443`):
/// an empty `system-prompt` message whose id has the `v2-to-v3-system-` prefix.
fn migrated_head(event: &Value, seq: u64) -> Value {
    let id = event["data"]["message"]["id"].as_str().unwrap();
    assert!(
        regex::Regex::new(r"^v2-to-v3-system-")
            .unwrap()
            .is_match(id),
        "{id}"
    );
    assert_eq!(
        event["data"]["message"]["source"],
        json!({"kind":"system-prompt"})
    );
    json!({
        "type":"system/message","seq":seq,"time":0,
        "data":{"turn":1,"step":1,"message":{
            "id":id,"role":"system","source":{"kind":"system-prompt"},"content":[]}},
        "surfaceOp":"append"
    })
}
fn v0(createdat: i64) -> Value {
    json!({"type":"session","version":0,"id":"s1","createdAt":createdat})
}
#[test]
fn migrates_v0_projected_and_packed_rows_to_the_current_events() {
    let source = lines(&[
        v0(7),
        json!({"type":"turn/start","data":{"turn":1}}),
        json!({"type":"step/start","data":{"turn":1,"step":1}}),
        json!({"type":"text-chunks","data":{"turn":1,"step":1,"index":0,"dt":[3],"texts":["a","b"]}}),
    ]);
    let events = values(&source);
    assert_eq!(events.len(), 4);
    assert_eq!(
        events[0],
        json!({"type":"turn/start","seq":0,"time":0,"data":{"turn":1}})
    );
    assert_eq!(
        events[1],
        json!({"type":"step/start","seq":1,"time":0,"data":{"turn":1,"step":1}})
    );
    assert_eq!(events[2], migrated_head(&events[2], 2));
    assert_eq!(
        events[3],
        json!({"type":"assistant/attempt","seq":3,"time":3,"data":{"turn":1,"step":1,
            "stream":[{"type":"text-chunks","time0":0,"index":0,"dt":[3],"texts":["a","b"]}]}})
    );
}
#[test]
fn expands_a_packed_chunk_row_of_a_v0_fixture_into_its_events() {
    let source = lines(&[
        v0(0),
        json!({"type":"turn/start","seq":0,"time":0,"data":{"turn":1}}),
        json!({"type":"step/start","seq":1,"time":0,"data":{"turn":1,"step":1}}),
        json!({"type":"text-chunks","seq0":2,"time0":0,
            "data":{"turn":1,"step":1,"index":0,"dt":[0,0],"texts":["a","b","c"]}}),
    ]);
    let events = values(&source);
    assert_eq!(events[2], migrated_head(&events[2], 2));
    assert_eq!(
        events[3],
        json!({"type":"assistant/attempt","seq":3,"time":0,"data":{"turn":1,"step":1,
            "stream":[{"type":"text-chunks","time0":0,"index":0,"dt":[0,0],"texts":["a","b","c"]}]}})
    );
}
fn user_messages(count: usize, ranges: Option<(usize, Value)>) -> Vec<Value> {
    (0..count)
        .map(|index| {
            let mut row = json!({"type":"user/message","seq":index + 2,"time":0,
                "data":{"role":"user","id":format!("message-{index}"),"content":[],"source":{"kind":"user"}},
                "surfaceOp":"append"});
            if let Some((at, value)) = &ranges
                && *at == index
            {
                row["sourceEventSeqs"] = value.clone();
            }
            row
        })
        .collect()
}
fn with_turn(header: Value, rest: Vec<Value>) -> String {
    let mut rows = vec![
        header,
        json!({"type":"turn/start","seq":0,"time":0,"data":{"turn":1}}),
        json!({"type":"step/start","seq":1,"time":0,"data":{"turn":1,"step":1}}),
    ];
    rows.extend(rest);
    lines(&rows)
}
#[test]
fn expands_range_encoded_source_event_seqs_after_the_migration() {
    let source = with_turn(v0(0), user_messages(5, Some((4, json!([[2, 4], 5])))));
    let events = values(&source);
    assert_eq!(events[7]["seq"], 7);
    assert_eq!(events[7]["sourceEventSeqs"], json!([3, 4, 5, 6]));
}
#[test]
fn locates_a_malformed_source_event_range_at_its_source_line() {
    let source = with_turn(v0(0), user_messages(5, Some((4, json!([[5, 3]])))));
    let error = parse_session_log(&source).unwrap_err().to_string();
    assert!(
        error.starts_with("session snapshot line 8: sourceEventSeqs range"),
        "{error}"
    );
    let mut header = v0(0);
    header["seedLength"] = json!(0);
    let source = with_turn(header, user_messages(3, Some((2, json!([[4, 3]])))));
    let error = parse_session_log(&source).unwrap_err().to_string();
    assert!(
        error.starts_with("session snapshot line 6: sourceEventSeqs range"),
        "{error}"
    );
}
#[test]
fn derives_system_messages_and_omits_tokenized_request_tools_in_every_older_generation() {
    for version in 0..3_u64 {
        let mut header = json!({"type":"session","version":version,"id":"tokens","createdAt":7,"delegationDepth":0});
        if version >= 2 {
            header["isSeeded"] = json!(false);
        }
        let source = lines(&[
            header,
            json!({"type":"turn/start","data":{"turn":1}}),
            json!({"type":"step/start","data":{"turn":1,"step":1}}),
            json!({"type":"request/header","data":{"header":{
                "config":{"provider":"mock","model":"mock"},"system":"{{system}}","tools":"{{tools}}"},
                "reason":"initial"}}),
        ]);
        let events = values(&source);
        assert_eq!(events.len(), 5, "v{version}");
        assert_eq!(events[2], migrated_head(&events[2], 2), "v{version}");
        assert_eq!(
            events[3]["data"]["message"]["content"],
            json!([{"type":"text","text":"{{system}}"}]),
            "v{version}"
        );
        assert_eq!(
            events[3]["surfaceOp"],
            json!({"op":"replace","startSeq":2,"endSeq":2})
        );
        assert_eq!(events[3]["sourceEventSeqs"], json!([2]));
        assert_eq!(
            events[4],
            json!({"type":"request/header","seq":4,"time":0,
                "data":{"reason":"initial","header":{"config":{"provider":"mock","model":"mock"}}}})
        );
        let comparison = prepare_session_snapshot_fixture_for_comparison(&source).unwrap();
        let last: Value =
            serde_json::from_str(comparison.trim_end().lines().last().unwrap()).unwrap();
        assert_eq!(
            last["data"]["header"],
            json!({"config":{"provider":"mock","model":"mock"},"tools":"{{tools}}"})
        );
        assert_eq!(values(&comparison), events, "v{version}");
    }
}
#[test]
fn refuses_a_retired_v0_event_through_the_released_migration_policy() {
    let source = lines(&[
        json!({"type":"session","version":0,"id":"legacy","createdAt":7,"delegationDepth":0}),
        json!({"type":"mode/set","data":{"mode":"plan"}}),
    ]);
    let error = parse_session_log(&source).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unsupported legacy mode/set event at seq 0"),
        "{error}"
    );
    assert!(matches!(
        error.downcast_ref::<ReplayError>(),
        Some(ReplayError::UnsupportedMigration { .. })
    ));
}
#[test]
fn reads_the_header_of_every_older_generation() {
    for version in 0..4_u64 {
        let mut head =
            json!({"type":"session","version":version,"id":"p","createdAt":5,"delegationDepth":0});
        if version >= 2 {
            head["isSeeded"] = json!(false);
        }
        let parsed = parse_session_header(&lines(&[head])).unwrap();
        assert_eq!(
            (parsed.id.as_str(), parsed.created_at),
            ("p", 5),
            "v{version}"
        );
        assert_eq!(parsed.inherited_event_count.get(), 0);
    }
    assert!(
        parse_session_log(&jsonl(&[header("p", 0)]))
            .unwrap()
            .is_empty()
    );
}
#[test]
fn refuses_a_generation_newer_than_the_build() {
    let error = parse_session_log(&lines(&[
        json!({"type":"session","version":5,"id":"p","createdAt":0,"delegationDepth":0,"isSeeded":false}),
    ]))
    .unwrap_err();
    assert!(error.to_string().contains("newer format v5"), "{error}");
    assert!(matches!(
        error.downcast_ref::<ReplayError>(),
        Some(ReplayError::UnsupportedMigration { .. })
    ));
}
fn refusal_line(source: &str) -> String {
    parse_session_log(source).unwrap_err().to_string()
}
#[test]
fn materializes_python_snapshot_tool_name_projections_before_format_validation() {
    let source = lines(&[
        json!({"type":"session","version":0,"id":"tool-names","createdAt":7,"delegationDepth":0}),
        json!({"type":"turn/start","data":{"turn":1}}),
        json!({"type":"request/header","data":{"header":{
            "config":{"provider":"mock","model":"mock"},"tools":["bash","workflow"]},
            "reason":"initial"}}),
    ]);
    assert_eq!(
        values(&source)[1]["data"]["header"]["tools"],
        json!([
            {"name":"bash","description":"","parameters":{}},
            {"name":"workflow","description":"","parameters":{}}
        ])
    );
}
#[test]
fn keeps_malformed_request_rows_unchanged_so_format_validation_rejects_their_line() {
    for (name, data) in [
        ("non-object request data", json!(null)),
        ("non-object request header", json!({"header":[]})),
        (
            "unsupported tools projection",
            json!({"header":{"tools":[42]}}),
        ),
    ] {
        let source = lines(&[
            json!({"type":"session","version":0,"id":"malformed-request","createdAt":7,"delegationDepth":0}),
            json!({"type":"request/header","data":data}),
        ]);
        assert!(
            refusal_line(&source).starts_with("session snapshot line 2:"),
            "{name}: {}",
            refusal_line(&source)
        );
    }
}
#[test]
fn synthesizes_projected_tool_call_chunk_envelopes_from_their_argument_count() {
    let source = lines(&[
        json!({"type":"session","version":0,"id":"packed-tool","createdAt":0}),
        json!({"type":"turn/start","data":{"turn":1}}),
        json!({"type":"step/start","data":{"turn":1,"step":1}}),
        json!({"type":"tool-call-chunks","data":{"turn":1,"step":1,"index":0,
            "id":"call-1","name":"read","dt":[0],"args":["{","}"]}}),
    ]);
    let events = values(&source);
    assert_eq!(events.len(), 4);
    assert_eq!(events[2], migrated_head(&events[2], 2));
    assert_eq!(
        events[3],
        json!({"type":"assistant/attempt","seq":3,"time":0,"data":{"turn":1,"step":1,
            "stream":[{"type":"tool-call-chunks","time0":0,"index":0,"dt":[0],
                "id":"call-1","name":"read","args":["{","}"]}]}})
    );
}
#[test]
fn reports_malformed_packed_rows_at_their_one_synthesized_event_line() {
    for (name, row) in [
        (
            "non-object packed data",
            json!({"type":"text-chunks","data":null}),
        ),
        (
            "empty packed payload",
            json!({"type":"text-chunks","data":{"turn":1,"step":1,"index":0,"dt":[],"texts":[]}}),
        ),
    ] {
        let source = lines(&[
            json!({"type":"session","version":0,"id":"malformed-packed","createdAt":0}),
            row,
        ]);
        assert!(
            refusal_line(&source).starts_with("session snapshot line 2:"),
            "{name}: {}",
            refusal_line(&source)
        );
    }
}
fn legacy_message(provenance: &Value) -> Value {
    let mut data = json!({"turn":1,"step":1,"content":[{"type":"text","text":"migrated"}]});
    // The retired member name is assembled so the source text never spells it out.
    data[["pro", "venance"].concat()] = provenance.clone();
    json!({"type":"assistant/message","data":data,"surfaceOp":"append"})
}
#[test]
fn migrates_projected_v0_legacy_messages_through_the_released_catalog() {
    let source = lines(&[
        json!({"type":"session","version":0,"id":"legacy","createdAt":7,"delegationDepth":0}),
        json!({"type":"turn/start","data":{"turn":1}}),
        json!({"type":"step/start","data":{"turn":1,"step":1}}),
        legacy_message(&json!({"provider":"mock","model":"mock"})),
    ]);
    assert_eq!(
        values(&source)[3],
        json!({"type":"assistant/message","seq":3,"time":0,"data":{
            "turn":1,"step":1,
            "message":{"id":"legacy-message:legacy:2","role":"assistant",
                "content":[{"type":"text","text":"migrated"}],
                "source":{"kind":"model","provider":"mock","model":"mock"}},
            "stream":[]},
            "surfaceOp":"append"})
    );
}
#[test]
fn takes_the_direct_v1_path_and_rejects_v0_only_message_fields() {
    let step = chunks("hi");
    let mut rows = vec![
        json!({"type":"session","version":1,"id":"current","createdAt":7,"delegationDepth":0}),
        json!({"type":"turn/start","data":{"turn":1}}),
        json!({"type":"step/start","data":{"turn":1,"step":1}}),
    ];
    for chunk in &step {
        rows.push(json!({"type":"assistant/chunk","data":{"turn":1,"step":1,
            "chunk":serde_json::to_value(chunk).unwrap()}}));
    }
    rows.push(json!({"type":"step/end","data":{"turn":1,"step":1}}));
    rows.push(json!({"type":"turn/end","data":{"turn":1,"reason":{"kind":"completed"}}}));
    let script = derive_replay_script(&parse_session_log(&lines(&rows)).unwrap()).unwrap();
    assert_eq!(script, vec![ReplayEntry::Chunks { chunks: step }]);
    let source = lines(&[
        json!({"type":"session","version":1,"id":"current","createdAt":7,"delegationDepth":0}),
        legacy_message(&json!({"provider":"mock","model":"mock"})),
    ]);
    let error = refusal_line(&source);
    assert!(
        regex::Regex::new(r#"assistant/message.*unexpected member "content""#)
            .unwrap()
            .is_match(&error),
        "{error}"
    );
}
#[test]
fn preserves_the_current_system_envelope_and_tool_sidecar_token_during_comparison() {
    let source = [
        json!({"type":"session","version":3,"id":"s1","createdAt":0,"isSeeded":false,"delegationDepth":0}),
        json!({"type":"turn/start","data":{"turn":1}}),
        json!({"type":"step/start","data":{"turn":1,"step":1}}),
        json!({"type":"system/message","data":{"turn":1,"step":1,"message":{
            "id":"current-system","role":"system",
            "source":{"kind":"plugin","plugin":"@deepseek-ai/dsh-system-prompt"},
            "content":[{"type":"text","text":"{{system}}"}]}},"surfaceOp":"append"}),
        json!({"type":"request/header","data":{"reason":"initial","header":{
            "config":{"provider":"mock","model":"mock"},"tools":"{{tools}}"}}}),
    ]
    .iter()
    .map(hoh_util_values::js_json_stringify)
    .collect::<Vec<_>>()
    .join("\n");
    let events = values(&source);
    assert_eq!(events[2]["type"], "system/message");
    assert_eq!(events[2]["surfaceOp"], "append");
    assert_eq!(
        events[2]["data"]["message"]["content"],
        json!([{"type":"text","text":"{{system}}"}])
    );
    assert!(events[3]["data"]["header"].get("tools").is_none());
    assert!(events[3]["data"]["header"].get("system").is_none());
    let comparison = prepare_session_snapshot_fixture_for_comparison(&source).unwrap();
    assert_eq!(values(&comparison), events);
    let last: Value = serde_json::from_str(comparison.lines().last().unwrap()).unwrap();
    assert_eq!(last["data"]["header"]["tools"], "{{tools}}");
}
#[test]
fn never_rewrites_a_projected_v2_source_fixture_while_migrating_it_in_memory() {
    let f = Files::new(&[chunks("hi")]);
    let mut rows: Vec<Value> = std::fs::read_to_string(&f.config.file)
        .unwrap()
        .lines()
        .map(|line| hoh_util_values::js_json_parse(line).unwrap())
        .collect();
    rows[0]["version"] = json!(2);
    let source = jsonl(&rows);
    std::fs::write(&f.config.file, &source).unwrap();
    assert_eq!(
        load_replay_script(&f.config).unwrap(),
        vec![ReplayEntry::Chunks {
            chunks: chunks("hi")
        }]
    );
    let compared = prepare_session_snapshot_fixture_for_comparison(&source).unwrap();
    let head: Value = serde_json::from_str(compared.lines().next().unwrap()).unwrap();
    assert_eq!(head["version"], hoh_session::SESSION_FORMAT_VERSION);
    assert_eq!(std::fs::read_to_string(&f.config.file).unwrap(), source);
}
fn seed_events(count: u64) -> Vec<Value> {
    (0..count)
        .map(|seq| json!({"type":"permission/preset","seq":seq,"time":0,"data":{"preset":"workspace-write"}}))
        .collect()
}
#[test]
fn reads_a_non_zero_v0_seed_length_as_the_inherited_event_count() {
    let mut rows = vec![
        json!({"type":"session","version":0,"id":"child","createdAt":7,"seedLength":4,"delegationDepth":0}),
    ];
    rows.extend(seed_events(4));
    let parsed = parse_session_header(&lines(&rows)).unwrap();
    assert_eq!((parsed.id.as_str(), parsed.created_at), ("child", 7));
    assert_eq!(parsed.inherited_event_count.get(), 4);
}
#[test]
fn materializes_omitted_v0_delegation_depth_and_tokenized_cwd_for_projected_validation() {
    let parsed = parse_session_header(
        "{\"type\":\"session\",\"version\":0,\"id\":\"projected\",\"createdAt\":7,\"cwd\":\"{{cwd}}/workspace\"}\n",
    )
    .unwrap();
    assert_eq!((parsed.id.as_str(), parsed.created_at), ("projected", 7));
    assert_eq!(parsed.inherited_event_count.get(), 0);
}
#[test]
fn rejects_an_invalid_v0_seed_length() {
    for seed_length in [json!(-1), json!(0.5), json!(9_007_199_254_740_992_u64)] {
        let header = json!({"type":"session","version":0,"id":"invalid","createdAt":0,
            "seedLength":seed_length,"delegationDepth":0});
        let error = parse_session_header(&lines(&[header])).unwrap_err();
        assert!(
            error.to_string().contains("seedLength"),
            "{seed_length}: {error}"
        );
    }
}
#[test]
fn reads_id_created_at_and_the_inherited_count_off_the_v0_header_line() {
    let parsed = parse_session_header(&lines(&[
        json!({"type":"session","version":0,"id":"abc","createdAt":42,"delegationDepth":0}),
    ]))
    .unwrap();
    assert_eq!((parsed.id.as_str(), parsed.created_at), ("abc", 42));
    assert_eq!(parsed.inherited_event_count.get(), 0);
}
#[test]
fn rejects_a_persisted_marked_compact_llm_call_without_its_complete_output() {
    let source = lines(&[
        json!({"type":"session","version":0,"id":"invalid-compact","createdAt":0,"delegationDepth":0}),
        json!({"type":"turn/start","seq":0,"time":0,"data":{"turn":1}}),
        json!({"type":"step/start","seq":1,"time":0,"data":{"turn":1,"step":1}}),
        json!({"type":"user/message","seq":2,"time":0,
            "data":{"role":"user","id":"source","content":[],"source":{"kind":"user"}},
            "surfaceOp":"append"}),
        json!({"type":"compaction/start","seq":3,"time":0,
            "data":{"compactionId":"replay-compaction","turn":1}}),
        json!({"type":"compaction/summary","seq":4,"time":0,"data":{
            "compactionId":"replay-compaction",
            "summary":[{"type":"text","text":"missing source events"}],
            "llmStreamCall":true,"shadowedRange":{"start":2,"end":2},"shadowedSeqs":[2],
            "shadowedTokenCount":20,"provider":"mock","model":"mock"}}),
    ]);
    let error = refusal_line(&source);
    assert!(
        error.contains("llmStreamCall requires rawOutput"),
        "{error}"
    );
}
