mod common;
use common::*;
use hoh_llm::*;
use indexmap::IndexSet;
use serde_json::json;
fn streams() -> Vec<Vec<StreamChunk>> {
    let mut state = 0x9e3779b97f4a7c15_u64;
    let mut streams = vec![];
    for _ in 0..512 {
        let mut chunks = vec![];
        for _ in 0..30 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let i = (state >> 32) % 5;
            let c = match state % 11 {
                0 => json!({"type":"block-start","index":i,"blockType":"text"}),
                1 => json!({"type":"block-start","index":i,"blockType":"reasoning"}),
                2 => json!({"type":"block-start","index":i,"blockType":"tool-call"}),
                3 => json!({"type":"text-delta","index":i,"text":format!("x{state}")}),
                4 => json!({"type":"reasoning-delta","index":i,"text":"😀"}),
                5 => json!({"type":"tool-call-delta","index":i,"id":"c","argumentsDelta":"{}"}),
                6 => json!({"type":"block-end","index":i,"block":{"type":"text","text":"end"}}),
                7 => {
                    json!({"type":"block-end","index":i,"block":{"type":"tool-call","id":"c","name":"n","arguments":"{}"}})
                }
                8 => json!({"type":"usage","usage":{"inputTokens":1,"outputTokens":1}}),
                9 => json!({"type":"finish","reason":{"kind":"tool-calls"}}),
                _ => {
                    json!({"type":"finish","reason":{"kind":"error","failure":{"message":"bad","code":"UNKNOWN"}}})
                }
            };
            chunks.push(chunk(c));
        }
        streams.push(chunks);
    }
    streams.push(vec![]);
    streams
}
fn feed(chunks: &[StreamChunk]) -> BlockAssembler {
    let mut a = BlockAssembler::new();
    for c in chunks {
        a.push(c.clone());
    }
    a
}
#[test]
fn partials_map_size_never_exceeds_the_number_of_distinct_indices_seen() {
    for chunks in streams() {
        let distinct: IndexSet<_> = chunks
            .iter()
            .filter_map(|c| value(c).get("index").and_then(|v| v.as_u64()))
            .collect();
        assert!(feed(&chunks).blocks().unwrap().len() <= distinct.len());
    }
}
#[test]
fn re_assembly_is_idempotent_blocks_is_stable_across_repeated_calls() {
    for chunks in streams() {
        let a = feed(&chunks);
        assert_eq!(a.blocks().unwrap(), a.blocks().unwrap());
        assert_eq!(
            a.message(AssistantProviderMetadata {
                provider: "mock".into(),
                model: "mock".into(),
                replay_state: None
            })
            .unwrap()
            .content(),
            a.blocks().unwrap()
        );
    }
}
#[test]
fn blocks_never_throws_and_yields_only_valid_content_block_tags() {
    for chunks in streams() {
        for b in feed(&chunks).blocks().unwrap() {
            assert!(["text", "reasoning", "tool-call", "tool-result"].contains(&b.r#type.as_str()));
        }
    }
}
#[test]
fn finish_reflects_the_last_finish_chunk_or_defaults_to_stop_when_none_arrives() {
    for chunks in streams() {
        let expected = chunks
            .iter()
            .rev()
            .find_map(|c| {
                if let StreamChunk::Finish { reason, .. } = c {
                    Some(reason.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| FinishReason::new("stop"));
        assert_eq!(feed(&chunks).finish(), expected);
    }
}
