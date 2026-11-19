//! Recorded streams per server flavour, replayed by a local server.
//! Each fixture is written from the documented wire format of the server named in its comment;
//! none was captured from a live server (see README).
mod support;
use hoh_llm::StreamChunk;
use hoh_llm_openai::{ReasoningFormat, ServerKind};
use serde_json::json;
use support::*;

fn stream_of(parts: &[String]) -> Reply {
    Reply::sse(parts.iter().map(|p| p.as_bytes().to_vec()).collect())
}

fn blocks(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect()
}

// Source: vLLM docs "Reasoning Outputs" (`--reasoning-parser qwen3`): `reasoning_content` deltas,
// then `content`; with `stream_options.include_usage` a final chunk with `choices: []`.
fn vllm_reasoning_content() -> Vec<String> {
    vec![
        delta(json!({"role": "assistant", "content": ""})),
        delta(json!({"reasoning_content": "The user "})),
        delta(json!({"reasoning_content": "greets me."})),
        delta(json!({"content": "Hello"})),
        delta(json!({"content": "!"})),
        finish("stop"),
        usage_chunk(12, 7),
        done().into(),
    ]
}

#[tokio::test]
async fn vllm_reasoning_content_field_becomes_a_reasoning_block() {
    let server = serve(vec![(CHAT, vec![stream_of(&vllm_reasoning_content())])]).await;
    let run = run_simple(&server, |c| c.server = ServerKind::Vllm).await;
    assert!(run.error.is_none(), "{:?}", run.error);
    assert_eq!(
        run.blocks(),
        blocks(&[("reasoning", "The user greets me."), ("text", "Hello!")])
    );
    assert_eq!(run.finish_kind(), "stop");
    let usage = run.usage().unwrap();
    assert_eq!(
        (usage.input_tokens, usage.output_tokens, usage.total_tokens),
        (12, 7, Some(19))
    );
    // Chunk protocol: start, deltas, end per block; usage before the single finish.
    assert_eq!(
        run.kinds(),
        [
            "block-start",
            "reasoning-delta",
            "reasoning-delta",
            "block-end",
            "block-start",
            "text-delta",
            "text-delta",
            "block-end",
            "usage",
            "finish"
        ]
    );
}

// Source: vLLM after the field rename (`reasoning` instead of `reasoning_content`).
#[tokio::test]
async fn vllm_renamed_reasoning_field_is_read_too() {
    let parts = vec![
        delta(json!({"reasoning": "thinking"})),
        delta(json!({"content": "done"})),
        finish("stop"),
        done().into(),
    ];
    let server = serve(vec![(CHAT, vec![stream_of(&parts)])]).await;
    let run = run_simple(&server, |_| {}).await;
    assert_eq!(
        run.blocks(),
        blocks(&[("reasoning", "thinking"), ("text", "done")])
    );
}

// Source: llama.cpp server README with `--reasoning-format none`: `<think>` stays in `content`;
// the closing tag is split across two deltas. Usage on the final chunk, plus llama.cpp `timings`.
#[tokio::test]
async fn llama_cpp_think_tags_in_content_with_a_split_tag() {
    let parts = vec![
        delta(json!({"role": "assistant", "content": null})),
        delta(json!({"content": "<think>"})),
        delta(json!({"content": "\nLet me "})),
        delta(json!({"content": "check.\n</th"})),
        delta(json!({"content": "ink>\n\nThe answer is 4."})),
        finish("stop"),
        data(
            json!({"choices": [], "usage": {"prompt_tokens": 20, "completion_tokens": 9, "total_tokens": 29},
            "timings": {"prompt_n": 20, "predicted_n": 9}}),
        ),
        done().into(),
    ];
    let server = serve(vec![(CHAT, vec![stream_of(&parts)])]).await;
    let run = run_simple(&server, |c| c.server = ServerKind::LlamaCpp).await;
    assert!(run.error.is_none(), "{:?}", run.error);
    assert_eq!(
        run.blocks(),
        blocks(&[
            ("reasoning", "Let me check.\n"),
            ("text", "The answer is 4.")
        ])
    );
    assert_eq!(run.usage().unwrap().total_tokens, Some(29));
}

#[tokio::test]
async fn think_tags_are_not_parsed_when_only_the_reasoning_field_is_expected() {
    let parts = vec![
        delta(json!({"content": "<think>x</think>y"})),
        finish("stop"),
        done().into(),
    ];
    let server = serve(vec![(CHAT, vec![stream_of(&parts)])]).await;
    let run = run_simple(&server, |c| {
        c.reasoning_format = ReasoningFormat::ReasoningField
    })
    .await;
    assert_eq!(run.blocks(), blocks(&[("text", "<think>x</think>y")]));
}

// Source: Qwen3 thinking chat templates put `<think>` into the prompt, so the reply carries only
// the closing tag. Selected explicitly; `auto` does not guess it.
#[tokio::test]
async fn implicit_open_think_tag() {
    let parts = vec![
        delta(json!({"content": "Step one. "})),
        delta(json!({"content": "Step two.</think>"})),
        delta(json!({"content": "\n\nFinal."})),
        finish("stop"),
        done().into(),
    ];
    let server = serve(vec![(CHAT, vec![stream_of(&parts)])]).await;
    let run = run_simple(&server, |c| {
        c.reasoning_format = ReasoningFormat::ThinkTagsImplicitOpen
    })
    .await;
    assert_eq!(
        run.blocks(),
        blocks(&[("reasoning", "Step one. Step two."), ("text", "Final.")])
    );
}

// Source: OpenAI streaming tool-call format as produced by vLLM/llama.cpp: id and name in the
// first delta, `index`-keyed argument fragments, two parallel calls, finish `tool_calls`.
#[tokio::test]
async fn tool_calls_are_assembled_by_index() {
    let parts = vec![
        delta(
            json!({"tool_calls": [{"index": 0, "id": "call_a", "type": "function",
            "function": {"name": "weather", "arguments": ""}}]}),
        ),
        delta(json!({"tool_calls": [{"index": 0, "function": {"arguments": "{\"city\":"}}]})),
        delta(json!({"tool_calls": [{"index": 0, "function": {"arguments": "\"Paris\"}"}}]})),
        delta(
            json!({"tool_calls": [{"index": 1, "id": "call_b", "type": "function",
            "function": {"name": "weather", "arguments": "{\"city\":\"Rome\"}"}}]}),
        ),
        finish("tool_calls"),
        usage_chunk(30, 15),
        done().into(),
    ];
    let server = serve(vec![(CHAT, vec![stream_of(&parts)])]).await;
    let adapter = adapter(&server, |_| {});
    let mut opts = options(vec![user("weather?")]);
    opts.tools = Some(vec![tool("weather", &["city"])]);
    let run = run(&adapter, opts).await;
    assert!(run.error.is_none(), "{:?}", run.error);
    assert_eq!(
        run.blocks(),
        blocks(&[
            ("tool-call", "weather({\"city\":\"Paris\"})"),
            ("tool-call", "weather({\"city\":\"Rome\"})")
        ])
    );
    assert_eq!(run.finish_kind(), "tool-calls");
    let ids: Vec<_> = run
        .chunks
        .iter()
        .filter_map(|c| match c {
            StreamChunk::BlockEnd { block, .. } => block
                .data
                .get("id")
                .and_then(|v| v.as_str().map(str::to_owned)),
            _ => None,
        })
        .collect();
    assert_eq!(ids, ["call_a", "call_b"]);
}

// Source: Ollama `docs/openai.md`: the whole call in one delta and finish reason "stop"
// even though a tool was called.
#[tokio::test]
async fn ollama_tool_call_in_one_delta_with_finish_stop() {
    let parts = vec![
        delta(
            json!({"tool_calls": [{"index": 0, "id": "call_xyz", "type": "function",
            "function": {"name": "weather", "arguments": "{\"city\":\"Oslo\"}"}}]}),
        ),
        finish("stop"),
        usage_chunk(5, 4),
        done().into(),
    ];
    let server = serve(vec![(CHAT, vec![stream_of(&parts)])]).await;
    let adapter = adapter(&server, |c| c.server = ServerKind::Ollama);
    let mut opts = options(vec![user("weather?")]);
    opts.tools = Some(vec![tool("weather", &["city"])]);
    let run = run(&adapter, opts).await;
    assert_eq!(
        run.blocks(),
        blocks(&[("tool-call", "weather({\"city\":\"Oslo\"})")])
    );
    assert_eq!(run.finish_kind(), "tool-calls");
}

// Source: servers that omit the tool call id (and, in one case, the index too).
#[tokio::test]
async fn missing_tool_call_ids_are_made_up_and_unique() {
    let parts = vec![
        delta(
            json!({"tool_calls": [{"index": 0, "function": {"name": "weather", "arguments": "{\"city\":\"A\"}"}}]}),
        ),
        delta(
            json!({"tool_calls": [{"index": 1, "id": "", "function": {"name": "weather", "arguments": "{\"city\":\"B\"}"}}]}),
        ),
        finish("tool_calls"),
        done().into(),
    ];
    let server = serve(vec![(CHAT, vec![stream_of(&parts)])]).await;
    let adapter = adapter(&server, |_| {});
    let mut opts = options(vec![user("x")]);
    opts.tools = Some(vec![tool("weather", &["city"])]);
    let run = run(&adapter, opts).await;
    let assembled = run.assembled().blocks().unwrap();
    let ids: Vec<_> = assembled
        .iter()
        .map(|b| b.data["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(
        ids.iter()
            .all(|id| id.starts_with("call_") && id.len() > 10)
    );
    assert_ne!(ids[0], ids[1]);
}

#[tokio::test]
async fn duplicate_ids_in_one_response_are_replaced() {
    let parts = vec![
        delta(
            json!({"tool_calls": [{"index": 0, "id": "call_0", "function": {"name": "weather", "arguments": "{\"city\":\"A\"}"}}]}),
        ),
        delta(
            json!({"tool_calls": [{"index": 1, "id": "call_0", "function": {"name": "weather", "arguments": "{\"city\":\"B\"}"}}]}),
        ),
        finish("tool_calls"),
        done().into(),
    ];
    let server = serve(vec![(CHAT, vec![stream_of(&parts)])]).await;
    let adapter = adapter(&server, |_| {});
    let mut opts = options(vec![user("x")]);
    opts.tools = Some(vec![tool("weather", &["city"])]);
    let blocks = run(&adapter, opts).await.assembled().blocks().unwrap();
    assert_eq!(blocks[0].data["id"], "call_0");
    assert_ne!(blocks[1].data["id"], "call_0");
}

// Source: SGLang reports index-derived ids ("call_0") that repeat between responses.
#[tokio::test]
async fn sglang_ids_are_always_synthesized() {
    let parts = vec![
        delta(
            json!({"tool_calls": [{"index": 0, "id": "call_0", "function": {"name": "weather", "arguments": "{\"city\":\"A\"}"}}]}),
        ),
        finish("tool_calls"),
        done().into(),
    ];
    let server = serve(vec![(CHAT, vec![stream_of(&parts)])]).await;
    let adapter = adapter(&server, |c| c.server = ServerKind::Sglang);
    let mut opts = options(vec![user("x")]);
    opts.tools = Some(vec![tool("weather", &["city"])]);
    let blocks = run(&adapter, opts).await.assembled().blocks().unwrap();
    assert_ne!(blocks[0].data["id"], "call_0");
}

// Source: Qwen tool use via hermes parsers may emit text before the call.
#[tokio::test]
async fn text_then_tool_call_are_separate_blocks() {
    let parts = vec![
        delta(json!({"content": "Checking."})),
        delta(
            json!({"tool_calls": [{"index": 0, "id": "c", "function": {"name": "weather", "arguments": "{\"city\":\"A\"}"}}]}),
        ),
        finish("tool_calls"),
        done().into(),
    ];
    let server = serve(vec![(CHAT, vec![stream_of(&parts)])]).await;
    let adapter = adapter(&server, |_| {});
    let mut opts = options(vec![user("x")]);
    opts.tools = Some(vec![tool("weather", &["city"])]);
    let run = run(&adapter, opts).await;
    assert_eq!(
        run.blocks(),
        blocks(&[
            ("text", "Checking."),
            ("tool-call", "weather({\"city\":\"A\"})")
        ])
    );
    assert_eq!(run.kinds()[..3], ["block-start", "text-delta", "block-end"]);
}

#[tokio::test]
async fn keep_alive_comments_and_split_frames_are_handled() {
    let whole = format!(
        ": keep-alive\n\n{}: ping\n\n{}{}{}",
        delta(json!({"content": "Hel"})),
        delta(json!({"content": "lo"})),
        finish("stop"),
        done()
    );
    // Split the byte stream into 7-byte pieces: frames, lines and the UTF-8 text all break.
    let pieces = whole.as_bytes().chunks(7).map(<[u8]>::to_vec).collect();
    let server = serve(vec![(CHAT, vec![Reply::sse(pieces)])]).await;
    let run = run_simple(&server, |_| {}).await;
    assert_eq!(run.blocks(), blocks(&[("text", "Hello")]));
}

#[tokio::test]
async fn multibyte_text_split_inside_a_character() {
    let whole = format!(
        "{}{}{}",
        delta(json!({"content": "Привет, мир"})),
        finish("stop"),
        done()
    );
    let bytes = whole.as_bytes();
    let at = whole.find("Привет").unwrap() + 3; // inside the second Cyrillic letter
    let server = serve(vec![(
        CHAT,
        vec![Reply::sse(vec![bytes[..at].to_vec(), bytes[at..].to_vec()])],
    )])
    .await;
    let run = run_simple(&server, |_| {}).await;
    assert_eq!(run.blocks(), blocks(&[("text", "Привет, мир")]));
}

#[tokio::test]
async fn finish_length_maps_to_max_tokens_and_prunes_the_truncated_call() {
    let parts = vec![
        delta(json!({"content": "Starting"})),
        delta(
            json!({"tool_calls": [{"index": 0, "id": "c", "function": {"name": "weather", "arguments": "{\"ci"}}]}),
        ),
        finish("length"),
        done().into(),
    ];
    let server = serve(vec![(CHAT, vec![stream_of(&parts)])]).await;
    let adapter = adapter(&server, |_| {});
    let mut opts = options(vec![user("x")]);
    opts.tools = Some(vec![tool("weather", &["city"])]);
    let run = run(&adapter, opts).await;
    assert!(run.error.is_none(), "{:?}", run.error);
    assert_eq!(run.finish_kind(), "max-tokens");
    assert_eq!(run.blocks(), blocks(&[("text", "Starting")]));
}

#[tokio::test]
async fn usage_on_the_finish_chunk_is_read() {
    // Source: older llama.cpp put `usage` on the chunk that carries the finish reason.
    let parts = vec![
        delta(json!({"content": "x"})),
        data(
            json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 3, "completion_tokens": 1, "prompt_tokens_details": {"cached_tokens": 2},
                      "completion_tokens_details": {"reasoning_tokens": 0}}}),
        ),
        done().into(),
    ];
    let server = serve(vec![(CHAT, vec![stream_of(&parts)])]).await;
    let usage = run_simple(&server, |_| {}).await.usage().unwrap();
    assert_eq!(
        (usage.input_tokens, usage.output_tokens, usage.total_tokens),
        (3, 1, Some(4))
    );
    assert_eq!(
        (usage.cache_read_tokens, usage.reasoning_tokens),
        (Some(2), Some(0))
    );
}

#[tokio::test]
async fn missing_done_after_finish_is_accepted() {
    let parts = vec![delta(json!({"content": "x"})), finish("stop")];
    let server = serve(vec![(CHAT, vec![stream_of(&parts)])]).await;
    let run = run_simple(&server, |_| {}).await;
    assert!(run.error.is_none(), "{:?}", run.error);
    assert_eq!(run.finish_kind(), "stop");
}

#[tokio::test]
async fn non_streaming_json_reply_is_translated() {
    // The server ignored `stream: true`.
    let body = json!({"id": "c", "object": "chat.completion", "choices": [{"index": 0, "finish_reason": "tool_calls",
        "message": {"role": "assistant", "content": null, "reasoning_content": "think",
            "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "weather", "arguments": "{\"city\":\"Z\"}"}}]}}],
        "usage": {"prompt_tokens": 2, "completion_tokens": 3}});
    let server = serve(vec![(CHAT, vec![Reply::json(200, &body.to_string())])]).await;
    let adapter = adapter(&server, |_| {});
    let mut opts = options(vec![user("x")]);
    opts.tools = Some(vec![tool("weather", &["city"])]);
    let run = run(&adapter, opts).await;
    assert!(run.error.is_none(), "{:?}", run.error);
    assert_eq!(
        run.blocks(),
        blocks(&[
            ("reasoning", "think"),
            ("tool-call", "weather({\"city\":\"Z\"})")
        ])
    );
    assert_eq!(run.finish_kind(), "tool-calls");
}
