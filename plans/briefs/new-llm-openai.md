# New crate: hoh-llm-openai — the model adapter for a local OpenAI-compatible server (Qwen)

There is no dsh source for this crate: it is new HoH code (`PLAN.md`, stage 1: "адаптер Chat Completions со
стримингом, таблицей расхождений серверов, строгим вызовом инструментов и форматами рассуждений Qwen"). The
founder's example runs on a local Qwen behind an OpenAI-compatible server (vLLM, llama.cpp server, Ollama, SGLang).

## Read first
- `PLAN.md` (Russian) stage 1 and the "Утвердить" section about the model; `docs/window.md` §3 and §4 (`hoh/model/*`).
- `crates/hoh-llm` (accepted): the adapter interface, `StreamChunk`, `AssistantStreamAccumulator`, errors,
  retry classification, `PORT_NOTES.md`. Your adapter plugs in exactly where dsh's DeepSeek adapter does.
- dsh's DeepSeek adapter for the shape of an adapter (read-only clone
  `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/llm/llm-deepseek/src`) and how it maps the wire
  stream to `StreamChunk`s, usage and finish reasons.
- `docs/porting/PORTING.md` for code rules (errors, async, no global state, tests).

## Build
`crates/hoh-llm-openai` (create it; workspace members are `crates/*`). Dependencies: workspace crates and
`[workspace.dependencies]` only (`reqwest` with rustls and stream is there). If you need another library, stop on
that point and report.
- Request: `POST {baseUrl}/chat/completions`, `stream: true`, `stream_options.include_usage`, messages mapped from
  hoh-llm's content blocks (text, images as data URLs where the server supports them, tool calls and results),
  tools as `function` with JSON Schema, `tool_choice`, temperature/max tokens from the call config. API key optional.
- Stream: SSE parsing robust to `data:` splits across chunks, `[DONE]`, keep-alive comments, error objects inside
  the stream, a server closing early. Map to `StreamChunk`s: text deltas, tool call deltas (index-based assembly,
  arguments as concatenated string), usage, finish reason.
- Reasoning: Qwen's formats — `reasoning_content` delta field (vLLM/SGLang), `<think>…</think>` inside content
  (llama.cpp/Ollama, including a tag split across chunks) — map to hoh-llm's reasoning blocks; a config switch for
  which to expect and auto-detection.
- A small table of server quirks (field names, usage placement, tool-call id absence) as data with tests.
- Strict tool calling: validate tool arguments JSON at the end of the stream and surface a clear error the agent loop
  can feed back (follow how hoh-llm models such errors).
- `hoh/model/probe` support: a function that checks a `baseUrl` (models list, a 1-token completion) with timeouts.

## Tests
No network: a local HTTP server on 127.0.0.1 inside tests (tokio) replaying recorded SSE streams. Fixtures for
each server flavour (write them from the servers' documented formats; mark each fixture's source in a comment).

## Environment
`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-s5`; `cargo check`
while writing, `cargo test`, `clippy --all-targets -D warnings`, `fmt --check` once at the end. Edit only
`crates/hoh-llm-openai`. No network, no installs, no git commands that change state.

## Done when
Tests, clippy and fmt clean; a `README.md` in the crate (Russian) describing supported servers, config, and what was
not checked against a real server. Report: result lines, what is supported, open questions. At most 400 words.
