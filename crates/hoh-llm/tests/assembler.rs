mod common;
use common::*;
use hoh_llm::*;
use serde_json::{Value, json};
fn feed(chunks: Vec<Value>) -> BlockAssembler {
    let mut a = BlockAssembler::new();
    for c in chunks {
        a.push_value(c).unwrap();
    }
    a
}
fn end(index: u64, block: Value) -> Value {
    json!({"type":"block-end","index":index,"block":block})
}
#[test]
fn assembles_interleaved_text_reasoning_and_tool_call_deltas() {
    let a = feed(vec![
        json!({"type":"block-start","index":0,"blockType":"reasoning"}),
        json!({"type":"reasoning-delta","index":0,"text":"thinking…"}),
        end(0, json!({"type":"reasoning","text":"thinking…"})),
        json!({"type":"block-start","index":1,"blockType":"text"}),
        json!({"type":"text-delta","index":1,"text":"Hello"}),
        json!({"type":"text-delta","index":1,"text":" world"}),
        json!({"type":"block-start","index":2,"blockType":"tool-call"}),
        json!({"type":"tool-call-delta","index":2,"id":"call-1","name":"echo","argumentsDelta":"{\"text\":"}),
        json!({"type":"tool-call-delta","index":2,"id":"call-1","argumentsDelta":"\"hi\"}"}),
        json!({"type":"usage","usage":{"inputTokens":10,"outputTokens":5}}),
        json!({"type":"finish","reason":{"kind":"tool-calls"}}),
    ]);
    assert_eq!(
        value(a.blocks().unwrap()),
        json!([{"type":"reasoning","text":"thinking…"},{"type":"text","text":"Hello world"},{"type":"tool-call","id":"call-1","name":"echo","arguments":"{\"text\":\"hi\"}"}])
    );
    assert_eq!(value(a.usage()), json!({"inputTokens":10,"outputTokens":5}));
    assert_eq!(a.finish().kind, "tool-calls");
    assert_eq!(
        a.message(AssistantProviderMetadata {
            provider: "mock".into(),
            model: "mock".into(),
            replay_state: None
        })
        .unwrap()
        .role(),
        "assistant"
    );
}
#[test]
fn records_the_completed_block_from_block_end() {
    let a = feed(vec![
        json!({"type":"block-start","index":0,"blockType":"text"}),
        json!({"type":"text-delta","index":0,"text":"hi"}),
        end(0, json!({"type":"text","text":"hi"})),
    ]);
    assert_eq!(a.blocks().unwrap(), [ContentBlock::text("hi")]);
}
#[test]
fn tolerates_deltas_without_explicit_block_start_end() {
    let a = feed(vec![
        json!({"type":"text-delta","index":0,"text":"implicit"}),
    ]);
    assert_eq!(a.blocks().unwrap(), [ContentBlock::text("implicit")]);
    assert_eq!(a.finish().kind, "stop");
}
#[test]
fn returns_undefined_usage_when_no_usage_chunk_was_received() {
    let mut a = BlockAssembler::new();
    a.push(text("no usage"));
    assert!(a.usage().is_none());
}
#[test]
fn reuses_an_existing_partial_when_ensure_is_called_with_a_tracked_index() {
    let a = feed(vec![
        json!({"type":"block-start","index":0,"blockType":"text"}),
        json!({"type":"text-delta","index":0,"text":"hi"}),
        end(0, json!({"type":"text","text":"hi"})),
    ]);
    assert_eq!(a.blocks().unwrap().len(), 1);
}
#[test]
fn throws_from_assemble_when_a_partial_has_an_unhandled_block_type() {
    let a = feed(vec![
        json!({"type":"block-start","index":0,"blockType":"video"}),
    ]);
    assert_eq!(
        a.blocks().unwrap_err().to_string(),
        "cannot assemble incomplete block of type \"video\""
    );
}
#[test]
fn ignores_duplicate_block_start_for_the_same_index() {
    let a = feed(vec![
        json!({"type":"block-start","index":0,"blockType":"text"}),
        json!({"type":"text-delta","index":0,"text":"one"}),
        json!({"type":"block-start","index":0,"blockType":"text"}),
        json!({"type":"text-delta","index":0,"text":" two"}),
        end(0, json!({"type":"text","text":"one two"})),
    ]);
    assert_eq!(a.blocks().unwrap(), [ContentBlock::text("one two")]);
}
#[test]
fn ignores_tool_call_delta_stragglers_after_block_end() {
    let call = json!({"type":"tool-call","id":"c1","name":"echo","arguments":"{}"});
    let a = feed(vec![
        end(0, call.clone()),
        json!({"type":"tool-call-delta","index":0,"id":"c1","name":"evil","argumentsDelta":"oops"}),
    ]);
    assert_eq!(value(a.blocks().unwrap()), json!([call]));
}
#[test]
fn assembles_tool_call_with_generated_id_fallback_when_no_id_provided() {
    let a = feed(vec![
        json!({"type":"tool-call-delta","index":0,"argumentsDelta":"{}"}),
    ]);
    assert_eq!(
        value(a.blocks().unwrap()),
        json!([{"type":"tool-call","id":"call-0","name":"","arguments":"{}"}])
    );
    let a = feed(vec![
        json!({"type":"block-start","index":7,"blockType":"tool-call"}),
    ]);
    assert_eq!(a.blocks().unwrap()[0].data["id"], "call-7");
}
#[test]
fn exposes_usage_via_the_getter_when_a_usage_chunk_was_received() {
    let a = feed(vec![
        json!({"type":"usage","usage":{"inputTokens":5,"outputTokens":3}}),
    ]);
    assert_eq!(value(a.usage()), json!({"inputTokens":5,"outputTokens":3}));
}
fn replay_assembler(reason: &str, blocks: Option<Vec<Value>>, tool: bool) -> BlockAssembler {
    let mut a = feed(vec![end(0, json!({"type":"text","text":"partial"}))]);
    if tool {
        a.push_value(end(
            1,
            json!({"type":"tool-call","id":"c1","name":"echo","arguments":"{\"text\":"}),
        ))
        .unwrap();
    }
    a.push(StreamChunk::Finish {
        reason: FinishReason::new(reason),
        replay_state: Some(ReplayEnvelope {
            response: json!({"responseId":"resp-1"}),
            blocks,
        }),
    });
    a
}
#[test]
fn prunes_per_block_replay_entries_with_the_tool_calls_a_max_tokens_finish_drops() {
    let mut a = replay_assembler(
        "max-tokens",
        Some(vec![json!("meta-0"), json!("meta-1"), json!("meta-2")]),
        true,
    );
    a.push_value(end(2, json!({"type":"reasoning","text":"tail"})))
        .unwrap();
    assert_eq!(
        value(a.blocks().unwrap()),
        json!([{"type":"text","text":"partial"},{"type":"reasoning","text":"tail"}])
    );
    assert_eq!(
        a.replay_state().unwrap().unwrap().blocks,
        Some(vec![json!("meta-0"), json!("meta-2")])
    );
}
#[test]
fn omits_replay_metadata_whose_per_block_entries_misalign_with_the_emitted_blocks() {
    let a = replay_assembler("stop", Some(vec![json!("meta-0")]), true);
    assert_eq!(a.blocks().unwrap().len(), 2);
    assert!(a.replay_state().unwrap().is_none());
}
#[test]
fn passes_replay_metadata_through_unchanged_when_assembly_drops_nothing() {
    let entries = vec![json!("meta-0"), json!("meta-1")];
    let a = replay_assembler("tool-calls", Some(entries.clone()), true);
    assert_eq!(a.replay_state().unwrap().unwrap().blocks, Some(entries));
}
#[test]
fn keeps_a_max_tokens_replay_state_with_no_per_block_entries_across_a_tool_call_drop() {
    let a = replay_assembler("max-tokens", None, true);
    assert_eq!(a.blocks().unwrap(), [ContentBlock::text("partial")]);
    assert!(a.replay_state().unwrap().unwrap().blocks.is_none());
}
#[test]
fn keeps_a_text_only_max_tokens_response_and_its_replay_metadata_intact() {
    let a = replay_assembler("max-tokens", Some(vec![json!("meta-0")]), false);
    assert_eq!(a.blocks().unwrap(), [ContentBlock::text("partial")]);
    assert_eq!(
        a.replay_state().unwrap().unwrap().blocks,
        Some(vec![json!("meta-0")])
    );
}
#[test]
fn block_assembler_push_rejects_chunks_outside_the_closed_stream_chunk_union() {
    assert!(
        BlockAssembler::new()
            .push_value(json!({"type":"rogue-chunk"}))
            .unwrap_err()
            .to_string()
            .contains("unreachable variant in BlockAssembler.push")
    );
}
#[test]
fn first_block_end_wins_a_duplicate_block_end_for_a_closed_index_is_ignored() {
    let a = feed(vec![
        end(0, json!({"type":"reasoning","text":"first"})),
        end(0, json!({"type":"text","text":"second"})),
    ]);
    assert_eq!(a.blocks().unwrap(), [ContentBlock::reasoning("first")]);
}
#[test]
fn keeps_closed_and_open_text_reasoning_blocks_with_streamed_content_in_order() {
    let a = feed(vec![
        end(0, json!({"type":"reasoning","text":"planning"})),
        json!({"type":"text-delta","index":1,"text":"half an ans"}),
    ]);
    assert_eq!(
        a.interrupted_blocks(),
        [
            ContentBlock::reasoning("planning"),
            ContentBlock::text("half an ans")
        ]
    );
}
#[test]
fn drops_tool_calls_whether_open_or_closed_interruption_precedes_dispatch() {
    let a = feed(vec![
        end(0, json!({"type":"text","text":"calling"})),
        end(
            1,
            json!({"type":"tool-call","id":"c1","name":"read","arguments":"{}"}),
        ),
        json!({"type":"tool-call-delta","index":2,"id":"c2","name":"read","argumentsDelta":"{\"pa"}),
    ]);
    assert_eq!(a.interrupted_blocks(), [ContentBlock::text("calling")]);
}
#[test]
fn drops_empty_and_whitespace_only_text_reasoning_blocks_and_unknown_open_block_types() {
    let a = feed(vec![
        json!({"type":"text-delta","index":0,"text":"  \n"}),
        json!({"type":"block-start","index":1,"blockType":"mystery"}),
        json!({"type":"block-start","index":2,"blockType":"reasoning"}),
    ]);
    assert!(a.interrupted_blocks().is_empty());
}
