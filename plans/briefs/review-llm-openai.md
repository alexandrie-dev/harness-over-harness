# Review: hoh-llm-openai (new code, OpenAI-protocol adapter for the window-chat milestone)

You only read. Do not run builds, tests, servers or any command that writes or uses the network; do not edit files.
Crate: `crates/hoh-llm-openai` (src/, tests/, README.md). It is new code, not a port. Contract it must satisfy: the LLM
interface in `crates/hoh-llm` (stream chunks, errors, cancellation) used by the agent loop; `hoh/model/probe` in
`crates/hoh-protocol`. Milestone: the founder enters a base URL and API key for any OpenAI-protocol provider (cloud or a
local Qwen via vLLM / llama.cpp / Ollama / SGLang) and gets a streamed answer; the agent calls tools (read, write, bash).
Check against the public OpenAI Chat Completions streaming format and the README's server-difference table:
(1) request body — messages incl. tool calls and tool results round-trip, tools/JSON Schema, tool_choice, stream_options;
(2) SSE parsing — split `data:` lines, `[DONE]`, keep-alive, error objects inside the stream, early close, idle timeout,
cancellation drops the HTTP connection; (3) tool-call deltas — index/id/name/arguments accumulated across chunks, multiple
parallel calls, malformed arguments → retryable MALFORMED_RESPONSE; (4) reasoning — `reasoning_content`, `reasoning`,
`<think>` split across chunks, never leaks into the answer text; (5) usage and finish reasons mapped correctly;
(6) security — the API key never appears in errors, Debug, logs or probe results; base URL handling (trailing slash, `/v1`);
(7) probe — never returns Err; does it list models (GET /models)? The window needs `models: string[]` in the probe result.
Triage: bug / contract violation / missing case the milestone needs / note.
Output: findings most severe first: kind, file:line, input, observed vs required, confirming test. ≤700 words. Russian.
