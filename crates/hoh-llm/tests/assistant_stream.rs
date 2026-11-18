mod common;
use common::*;
use hoh_llm::*;
use serde_json::{Value, json};
fn timed(time: i64, c: Value) -> TimedStreamChunk {
    TimedStreamChunk {
        time,
        chunk: chunk(c),
    }
}
fn run(time: i64, gaps: Vec<i64>, texts: Vec<&str>) -> AssistantStreamRecord {
    AssistantStreamRecord::TextChunks {
        time0: time,
        index: 0,
        dt: gaps,
        texts: texts.into_iter().map(str::to_owned).collect(),
    }
}
fn raw(time: i64, c: Value) -> AssistantStreamRecord {
    AssistantStreamRecord::Chunk { time, chunk: c }
}
fn tool_run(
    time: i64,
    gaps: Vec<i64>,
    args: Vec<&str>,
    name: Option<&str>,
) -> AssistantStreamRecord {
    AssistantStreamRecord::ToolCallChunks {
        time0: time,
        index: 0,
        dt: gaps,
        id: "call".into(),
        name: name.map(str::to_owned),
        args: args.into_iter().map(str::to_owned).collect(),
    }
}
#[test]
fn keeps_delta_boundaries_and_timestamps_while_compacting_one_attempt() {
    let chunks = vec![
        timed(1000, json!({"type":"text-delta","index":0,"text":"hel"})),
        timed(1006, json!({"type":"text-delta","index":0,"text":"lo"})),
        timed(
            1008,
            json!({"type":"tool-call-delta","index":1,"id":"call-1","name":"run_code","argumentsDelta":"{"}),
        ),
        timed(
            1011,
            json!({"type":"tool-call-delta","index":1,"id":"call-1","name":"run_code","argumentsDelta":"}"}),
        ),
        timed(1020, json!({"type":"finish","reason":{"kind":"stop"}})),
    ];
    let mut a = AssistantStreamAccumulator::new();
    for c in &chunks {
        a.push(c.clone()).unwrap();
    }
    assert_eq!(
        value(a.snapshot()),
        json!([{"type":"text-chunks","time0":1000,"index":0,"dt":[6],"texts":["hel","lo"]},{"type":"tool-call-chunks","time0":1008,"index":1,"dt":[3],"id":"call-1","name":"run_code","args":["{","}"]},{"type":"chunk","time":1020,"chunk":{"type":"finish","reason":{"kind":"stop"}}}])
    );
    assert_eq!(expand_assistant_stream(&a.snapshot()).unwrap(), chunks);
    let compact = hoh_util_values::js_json_stringify(&value(a.snapshot()));
    assert!(compact.contains(r#""index":1,"dt":[3],"id":"call-1","name":"run_code","args"#));
}
#[test]
fn snapshots_each_admitted_chunk_once_and_detaches_earlier_compact_views() {
    let mut a = AssistantStreamAccumulator::new();
    a.push(TimedStreamChunk {
        time: 10,
        chunk: text("a"),
    })
    .unwrap();
    let mut usage = json!({"type":"usage","usage":{"inputTokens":3,"outputTokens":2}});
    a.push(timed(11, usage.clone())).unwrap();
    usage["usage"]["inputTokens"] = json!(99);
    let first = a.snapshot();
    a.push(TimedStreamChunk {
        time: 12,
        chunk: text("b"),
    })
    .unwrap();
    assert_eq!(
        expand_assistant_stream(&first).unwrap()[1].chunk,
        chunk(json!({"type":"usage","usage":{"inputTokens":3,"outputTokens":2}}))
    );
    assert_eq!(expand_assistant_stream(&a.snapshot()).unwrap().len(), 3);
}
#[test]
fn detaches_raw_records_while_expanding_durable_input() {
    let mut stream = json!([{"type":"chunk","time":10,"chunk":{"type":"usage","usage":{"inputTokens":3,"outputTokens":2}}}]);
    let result = expand_assistant_stream_value(&stream).unwrap();
    stream[0]["chunk"]["usage"]["inputTokens"] = json!(99);
    assert_eq!(result[0]["chunk"]["usage"]["inputTokens"], 3);
}
#[test]
fn keeps_incompatible_delta_runs_separate_and_expands_reasoning_and_nameless_tool_calls() {
    let mut a = AssistantStreamAccumulator::new();
    let mut chunks = vec![];
    for (time, c) in [
        (1, json!({"type":"reasoning-delta","index":0,"text":"r1"})),
        (2, json!({"type":"reasoning-delta","index":0,"text":"r2"})),
        (3, json!({"type":"text-delta","index":0,"text":"a"})),
        (4, json!({"type":"text-delta","index":1,"text":"b"})),
        (
            5,
            json!({"type":"tool-call-delta","index":0,"id":"one","argumentsDelta":"{"}),
        ),
        (
            6,
            json!({"type":"tool-call-delta","index":0,"id":"one","argumentsDelta":"}"}),
        ),
        (
            7,
            json!({"type":"tool-call-delta","index":0,"id":"one","name":"read","argumentsDelta":""}),
        ),
        (
            8,
            json!({"type":"tool-call-delta","index":1,"id":"two","name":"read","argumentsDelta":"x"}),
        ),
        (
            9007199254740991,
            json!({"type":"text-delta","index":1,"text":"far"}),
        ),
        (
            -9007199254740991,
            json!({"type":"text-delta","index":1,"text":"apart"}),
        ),
    ] {
        let c = timed(time, c);
        a.push(c.clone()).unwrap();
        chunks.push(c);
    }
    assert_eq!(expand_assistant_stream(&a.snapshot()).unwrap(), chunks);
    assert_eq!(
        value(a.snapshot())
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "reasoning-chunks",
            "text-chunks",
            "text-chunks",
            "tool-call-chunks",
            "tool-call-chunks",
            "tool-call-chunks",
            "text-chunks",
            "text-chunks"
        ]
    );
}
#[test]
fn rejects_unsafe_values_while_preserving_json_safe_empty_adapter_identities() {
    let mut a = AssistantStreamAccumulator::new();
    for (v, message) in [
        (
            json!({"time":0.5,"chunk":{"type":"finish","reason":{"kind":"stop"}}}),
            "safe integer",
        ),
        (
            json!({"time":1,"chunk":{"type":"text-delta","index":-1,"text":"bad"}}),
            "index",
        ),
        (
            json!({"time":1,"chunk":{"type":"text-delta","index":0,"text":1}}),
            "text must be a string",
        ),
        (
            json!({"time":1,"chunk":{"type":"tool-call-delta","index":0,"id":1,"argumentsDelta":"{}"}}),
            "id must be a string",
        ),
        (
            json!({"time":1,"chunk":{"type":"tool-call-delta","index":0,"id":"call","name":1,"argumentsDelta":"{}"}}),
            "name must be a string",
        ),
        (
            json!({"time":1,"chunk":{"type":"tool-call-delta","index":0,"id":"call","argumentsDelta":1}}),
            "argumentsDelta must be a string",
        ),
        (
            json!({"time":1,"chunk":{"type":"future"}}),
            "unreachable variant in AssistantStreamAccumulator.push",
        ),
    ] {
        assert!(a.push_value(&v).unwrap_err().to_string().contains(message));
    }
    for c in [
        json!({"type":"tool-call-delta","index":0,"id":"","argumentsDelta":"{}"}),
        json!({"type":"tool-call-delta","index":0,"id":"call","name":"","argumentsDelta":"{}"}),
    ] {
        a.push(timed(1, c)).unwrap();
    }
    assert!(
        a.snapshot()
            .iter()
            .all(|r| matches!(r, AssistantStreamRecord::Chunk { .. }))
    );
    assert_eq!(expand_assistant_stream(&a.snapshot()).unwrap().len(), 2);
}
#[test]
fn rejects_malformed_compact_records() {
    for (r, message) in [
        (json!(null), "must be an object"),
        (json!([]), "must be an object"),
        (json!({"type":"future"}), "Unsupported"),
        (
            json!({"type":"text-chunks","time0":1,"index":0,"dt":[],"texts":[]}),
            "non-empty",
        ),
        (
            json!({"type":"reasoning-chunks","time0":1,"index":0,"dt":[],"texts":[1]}),
            "string array",
        ),
        (
            json!({"type":"text-chunks","time0":0.5,"index":0,"dt":[],"texts":["a"]}),
            "safe integer",
        ),
        (
            json!({"type":"text-chunks","time0":1,"index":-0.0,"dt":[],"texts":["a"]}),
            "index",
        ),
        (
            json!({"type":"text-chunks","time0":1,"index":-1,"dt":[],"texts":["a"]}),
            "index",
        ),
        (
            json!({"type":"text-chunks","time0":1,"index":0,"dt":[0.5],"texts":["a","b"]}),
            "dt",
        ),
        (
            json!({"type":"text-chunks","time0":1,"index":0,"dt":[1],"texts":["a"]}),
            "dt length",
        ),
        (
            json!({"type":"text-chunks","time0":9007199254740991_i64,"index":0,"dt":[1],"texts":["a","b"]}),
            "member times",
        ),
        (
            json!({"type":"tool-call-chunks","time0":1,"index":0,"id":"","dt":[],"args":["a"]}),
            "id",
        ),
        (
            json!({"type":"tool-call-chunks","time0":1,"index":0,"id":"id","name":"","dt":[],"args":["a"]}),
            "name",
        ),
        (
            json!({"type":"tool-call-chunks","time0":1,"index":0,"id":"id","dt":[],"args":[]}),
            "non-empty",
        ),
        (
            json!({"type":"tool-call-chunks","time0":1,"index":0,"id":"id","dt":[],"args":[1]}),
            "string array",
        ),
        (
            json!({"type":"chunk","time":0.5,"chunk":{"type":"finish","reason":{"kind":"stop"}}}),
            "safe integer",
        ),
        (
            json!({"type":"chunk","time":1,"chunk":null}),
            "lossless JSON object",
        ),
        (
            json!({"type":"chunk","time":1,"chunk":[]}),
            "lossless JSON object",
        ),
        (
            json!({"type":"text-chunks","time0":1,"index":0,"dt":[],"texts":["a"],"extra":true}),
            "exactly",
        ),
        (
            json!({"type":"chunk","time":1,"chunk":{"type":"finish","reason":{"kind":"stop"}},"extra":true}),
            "exactly",
        ),
    ] {
        let e = expand_assistant_stream_value(&json!([r])).unwrap_err();
        assert!(e.to_string().contains(message), "{r}: {e}");
    }
}
#[test]
fn recognizes_the_first_token_as_a_non_empty_fragment_or_a_name_bearing_tool_call_delta() {
    for (v, expected) in [
        (json!({"type":"text-delta","index":0,"text":" "}), true),
        (json!({"type":"text-delta","index":0,"text":""}), false),
        (json!({"type":"reasoning-delta","index":0,"text":"r"}), true),
        (json!({"type":"reasoning-delta","index":0,"text":""}), false),
        (
            json!({"type":"tool-call-delta","index":0,"id":"c","argumentsDelta":"{"}),
            true,
        ),
        (
            json!({"type":"tool-call-delta","index":0,"id":"c","argumentsDelta":""}),
            false,
        ),
        (
            json!({"type":"tool-call-delta","index":0,"id":"c","name":"read","argumentsDelta":""}),
            true,
        ),
        (
            json!({"type":"tool-call-delta","index":0,"id":"c","name":"","argumentsDelta":""}),
            true,
        ),
        (
            json!({"type":"block-start","index":0,"blockType":"text"}),
            false,
        ),
        (
            json!({"type":"block-end","index":0,"block":{"type":"text","text":"x"}}),
            false,
        ),
        (
            json!({"type":"usage","usage":{"inputTokens":1,"outputTokens":1}}),
            false,
        ),
        (json!({"type":"finish","reason":{"kind":"stop"}}), false),
    ] {
        assert_eq!(is_token_delta(&chunk(v)), expected);
    }
}
#[test]
fn classifies_reader_visible_chunks_by_non_whitespace_text_and_non_tool_call_block_kinds() {
    for (v, expected) in [
        (json!({"type":"text-delta","index":0,"text":" \t\n"}), false),
        (json!({"type":"text-delta","index":0,"text":" x"}), true),
        (
            json!({"type":"reasoning-delta","index":0,"text":"\u{00a0}"}),
            false,
        ),
        (json!({"type":"reasoning-delta","index":0,"text":"r"}), true),
        (
            json!({"type":"block-start","index":0,"blockType":"text"}),
            false,
        ),
        (
            json!({"type":"block-start","index":0,"blockType":"reasoning"}),
            false,
        ),
        (
            json!({"type":"block-start","index":0,"blockType":"tool-call"}),
            false,
        ),
        (
            json!({"type":"block-start","index":0,"blockType":"image"}),
            true,
        ),
        (
            json!({"type":"block-end","index":0,"block":{"type":"text","text":"  "}}),
            false,
        ),
        (
            json!({"type":"block-end","index":0,"block":{"type":"reasoning","text":"why"}}),
            true,
        ),
        (
            json!({"type":"block-end","index":0,"block":{"type":"tool-call","id":"c","name":"read","arguments":"{}"}}),
            false,
        ),
        (
            json!({"type":"block-end","index":0,"block":{"type":"image","attachment":{}}}),
            true,
        ),
        (
            json!({"type":"tool-call-delta","index":0,"id":"c","name":"read","argumentsDelta":"{}"}),
            false,
        ),
        (json!({"type":"finish","reason":{"kind":"stop"}}), false),
    ] {
        assert_eq!(is_visible_chunk(&chunk(v)), expected);
    }
}
#[test]
fn counts_visible_text_only_from_text_deltas_and_completed_text_blocks() {
    for c in [
        text("a"),
        chunk(json!({"type":"block-end","index":0,"block":{"type":"text","text":" a "}})),
    ] {
        assert!(chunk_has_visible_text(&c));
    }
    for c in [
        text("\r\n"),
        chunk(json!({"type":"reasoning-delta","index":0,"text":"a"})),
        chunk(json!({"type":"block-end","index":0,"block":{"type":"text","text":" "}})),
        chunk(json!({"type":"block-end","index":0,"block":{"type":"reasoning","text":"a"}})),
        chunk(json!({"type":"block-start","index":0,"blockType":"text"})),
        StreamChunk::finish(FinishReason::new("stop")),
    ] {
        assert!(!chunk_has_visible_text(&c));
    }
}
#[test]
fn reconstructs_the_first_token_member_time_from_time0_and_the_preceding_gaps() {
    assert_eq!(
        run_first_token_time(&run(1000, vec![5, -3, 10], vec!["", "", "x", "y"])),
        Some(1002)
    );
    assert_eq!(
        run_first_token_time(&run(1000, vec![5], vec!["a", "b"])),
        Some(1000)
    );
    assert_eq!(
        run_first_token_time(&run(7, vec![1, 1], vec!["", "", ""])),
        None
    );
    assert_eq!(
        run_first_token_time(&tool_run(50, vec![2, 2], vec!["", "", "{"], None)),
        Some(54)
    );
    assert_eq!(
        run_first_token_time(&tool_run(50, vec![2], vec!["", ""], None)),
        None
    );
    assert_eq!(
        run_first_token_time(&tool_run(50, vec![2], vec!["", ""], Some("read"))),
        Some(50)
    );
}
#[test]
fn reconstructs_the_first_visible_member_time_from_non_whitespace_fragments_only() {
    assert_eq!(
        run_first_visible_time(&run(1000, vec![5, 1, 1], vec!["", "   ", "\t", "answer"])),
        Some(1007)
    );
    assert_eq!(
        run_first_visible_time(&run(20, vec![3], vec![" ", "think"])),
        Some(23)
    );
    assert_eq!(
        run_first_visible_time(&run(20, vec![3], vec![" ", "\n"])),
        None
    );
    assert_eq!(
        run_first_visible_time(&tool_run(20, vec![3], vec!["{\"x\":", "1}"], Some("read"))),
        None
    );
}
#[test]
fn stops_reading_fragments_at_the_first_qualifying_member() {
    let malformed_tail = run(0, vec![], vec!["x", "unread"]);
    assert_eq!(run_first_token_time(&malformed_tail), Some(0));
    assert_eq!(run_first_visible_time(&malformed_tail), Some(0));
    assert_eq!(
        run_first_token_time(&tool_run(9, vec![], vec![], Some("read"))),
        Some(9)
    );
}
fn reader_stream() -> Vec<AssistantStreamRecord> {
    vec![
        raw(
            100,
            json!({"type":"block-start","index":0,"blockType":"reasoning"}),
        ),
        AssistantStreamRecord::ReasoningChunks {
            time0: 101,
            index: 0,
            dt: vec![2, 2],
            texts: vec!["".into(), " ".into(), "think".into()],
        },
        raw(
            106,
            json!({"type":"block-end","index":0,"block":{"type":"reasoning","text":" think"}}),
        ),
        run(108, vec![1, 1], vec!["\n", "ans", "wer"]),
        raw(
            111,
            json!({"type":"block-end","index":1,"block":{"type":"text","text":"\nanswer"}}),
        ),
        raw(
            112,
            json!({"type":"usage","usage":{"inputTokens":10,"outputTokens":4}}),
        ),
        raw(
            113,
            json!({"type":"usage","usage":{"inputTokens":10,"outputTokens":9}}),
        ),
        raw(114, json!({"type":"finish","reason":{"kind":"stop"}})),
    ]
}
#[test]
fn answers_first_token_visibility_text_and_usage_questions_from_records() {
    let s = reader_stream();
    assert_eq!(assistant_stream_first_token_time(&s), Some(103));
    assert!(assistant_stream_has_visible_content(&s));
    assert!(assistant_stream_has_visible_text(&s));
    assert_eq!(
        value(last_assistant_stream_chunk(&s, RawStreamChunkType::Usage))["usage"]["outputTokens"],
        9
    );
    assert_eq!(
        assistant_stream_chunks(&s, RawStreamChunkType::BlockEnd).len(),
        2
    );
    assert_eq!(
        assistant_stream_chunks(&s, RawStreamChunkType::Usage).len(),
        2
    );
    assert_eq!(join_assistant_stream_text(&s), "\nanswer");
}
#[test]
fn reports_absence_on_empty_whitespace_only_and_tool_call_only_streams() {
    let s = vec![
        tool_run(2, vec![1], vec!["", ""], Some("read")),
        run(5, vec![1], vec![" ", "\t"]),
    ];
    for stream in [&[][..], &s] {
        assert!(!assistant_stream_has_visible_content(stream));
        assert!(!assistant_stream_has_visible_text(stream));
        assert!(last_assistant_stream_chunk(stream, RawStreamChunkType::Usage).is_none());
        assert!(assistant_stream_chunks(stream, RawStreamChunkType::Usage).is_empty());
    }
    assert_eq!(assistant_stream_first_token_time(&s), Some(2));
    assert_eq!(assistant_stream_first_token_time(&[]), None);
    assert_eq!(join_assistant_stream_text(&s), " \t");
    assert_eq!(join_assistant_stream_text(&[]), "");
}
#[test]
fn reads_raw_text_deltas_and_empty_argument_tool_call_deltas_the_accumulator_kept_as_chunks() {
    let s = vec![
        raw(
            1,
            json!({"type":"tool-call-delta","index":0,"id":"","argumentsDelta":""}),
        ),
        raw(
            2,
            json!({"type":"tool-call-delta","index":0,"id":"call","name":"","argumentsDelta":""}),
        ),
        raw(3, value(text(" "))),
        raw(4, value(text("raw"))),
    ];
    assert_eq!(assistant_stream_first_token_time(&s), Some(2));
    assert!(assistant_stream_has_visible_content(&s));
    assert!(!assistant_stream_has_visible_content(&s[..3]));
    assert!(assistant_stream_has_visible_text(&s));
    assert!(!assistant_stream_has_visible_text(&s[..3]));
    assert_eq!(join_assistant_stream_text(&s), " raw");
}
#[test]
fn stops_at_the_first_qualifying_record() {
    let bad = run(0, vec![], vec!["", "bad"]);
    assert_eq!(
        assistant_stream_first_token_time(&[run(5, vec![], vec!["x"]), bad.clone()]),
        Some(5)
    );
    assert!(assistant_stream_has_visible_content(&[
        run(1, vec![], vec!["r"]),
        bad.clone()
    ]));
    assert!(assistant_stream_has_visible_text(&[
        run(1, vec![], vec!["t"]),
        bad.clone()
    ]));
    assert!(
        last_assistant_stream_chunk(
            &[
                bad,
                raw(9, json!({"type":"finish","reason":{"kind":"stop"}}))
            ],
            RawStreamChunkType::Finish
        )
        .is_some()
    );
}
#[test]
fn assembles_the_same_blocks_usage_finish_and_replay_state_as_the_expanded_members() {
    let mut a = AssistantStreamAccumulator::new();
    let chunks = vec![
        chunk(json!({"type":"reasoning-delta","index":0,"text":"th"})),
        chunk(json!({"type":"reasoning-delta","index":0,"text":"ink"})),
        text("an"),
        text("swer"),
        chunk(
            json!({"type":"tool-call-delta","index":2,"id":"call-1","name":"read","argumentsDelta":""}),
        ),
        chunk(
            json!({"type":"tool-call-delta","index":2,"id":"call-1","argumentsDelta":"{\"path\":"}),
        ),
        chunk(json!({"type":"tool-call-delta","index":2,"id":"call-1","argumentsDelta":"\"a\"}"})),
        chunk(json!({"type":"tool-call-delta","index":3,"id":"","argumentsDelta":"{}"})),
        chunk(json!({"type":"usage","usage":{"inputTokens":3,"outputTokens":2}})),
        chunk(
            json!({"type":"finish","reason":{"kind":"tool-calls"},"replayState":{"response":{"id":"r"}}}),
        ),
    ];
    for (i, c) in chunks.into_iter().enumerate() {
        a.push(TimedStreamChunk {
            time: 1000 + i as i64,
            chunk: c,
        })
        .unwrap();
    }
    let s = a.snapshot();
    let mut expanded = BlockAssembler::new();
    for t in expand_assistant_stream(&s).unwrap() {
        expanded.push(t.chunk);
    }
    let mut packed = BlockAssembler::new();
    assemble_assistant_stream(&s, &mut packed).unwrap();
    assert_eq!(packed.blocks().unwrap(), expanded.blocks().unwrap());
    assert_eq!(packed.usage(), expanded.usage());
    assert_eq!(packed.finish(), expanded.finish());
    assert_eq!(
        packed.replay_state().unwrap(),
        expanded.replay_state().unwrap()
    );
    assemble_assistant_stream(&[], &mut packed).unwrap();
}
