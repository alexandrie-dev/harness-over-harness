# Review: hoh-llm against its TypeScript source

You are an independent reviewer in a porting project (DeepSeek Harness, TypeScript → HoH, Rust). The code under review was written by a different model (Codex). You only read. Do not run builds, tests, servers or any command that writes; do not edit or create any file.

Forty-one packages build on this crate (every model adapter, the agent loop, sessions), and recorded sessions of dsh are replayed against the result byte for byte: its public API and everything it serializes matter most.

## Read
- `docs/porting/PORTING.md` (in Russian) — binding rules; checklist in section 14; sections 4, 6, 7, 8 (Typert: transport not ported, methods stay public with a `/// Remote: <namespace>.<method>` line), 9 (zod).
- The task: `plans/briefs/stage1-writer.md` and `plans/briefs/stage1-w7-llm.md`.
- Source: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/llm/llm` (`src/` 13 files, `tests/` 12 files, `README.md`); for the dispatch semantics `vendor/cordis/src/events.ts`.
- Port: `crates/hoh-llm` (`src/`, `tests/`, `PORT_NOTES.md`). Kernel: `crates/hoh-kernel/PORT_NOTES.md` (the `Contained` event mode).
- How the stream is recorded and replayed: `crates/hoh-conformance/README.md` sections 1.4 and 6.1 item 6.

## What to check
1. Message and content types: field names, serialized shapes, key order, optional fields (absent versus `null`), numbers (`hoh_util_values::js_json_*` where JavaScript's number text matters).
2. `StreamChunk` and the assistant stream accumulator (`assistant-stream.ts`): the rules that pack deltas into `text-chunks` / `reasoning-chunks` / `tool-call-chunks` records (same type and index in a row, same `id` and same presence and value of `name`; an empty `id` or `name` gives a raw `chunk`), `time0` and `dt[]`, block start/end, `usage`, `finish`. This is compared with recorded logs by count and array lengths: check every branch against the source.
3. Call configuration and its defaults; retry policy (delays, jitter source, which failures retry, limits); adapter failures and their codes, `LlmError.fromFailure`; API keys; attribution.
4. `LlmRuntime`: registration and removal of adapters, `llm/adapters-updated` through the kernel's contained dispatch (failing listener does not undo, later listeners run, what is logged), the `/// Remote:` lines for every method the source marks `@Remote`, the error type that replaces `RemoteError` (section 7).
5. Cancellation (`AbortSignal` → `hoh_timeout::AbortSignal`): where the source checks it and what it throws.
6. zod schemas reproduced by hand: where a failure text reaches a user or a model, is the text the same?
7. The replay boundary: are `LlmAdapter` and `StreamChunk` public and sufficient for a plugin that feeds recorded chunks (README 1.4), including `throw` after prefix chunks, `hang`, and the side effect after a successful call of provider `deepseek-official`?
8. Tests: the writer reports 205 of 213 source groups ported and eight excluded (Proxy/coercion, `deepFreeze`, private-state corruption, impossible enum casts). For each excluded group say whether the reason holds for the ported API. Sample at least three test files in full and check that expected texts and values are the source's.
9. Each entry of `PORT_NOTES.md` "Отличия", in particular "owned Rust snapshots", "explicit asynchronous `close()`", "lone-surrogate limitation", and the open point about `HostPathMapping`.

## Triage rule
This is a port to Rust, not an emulation of JavaScript. A divergence that follows from the Rust type system and cannot reach an observer is a `note`, not a `bug`. A `bug` is a case where an input the ported API accepts gives a different result, order of effects, error, text or serialized bytes than the source. Statements about libraries or Node made from memory must be checked in the clone or marked "not checked".

## Output
A list of findings, most severe first. For each: kind (`bug`, `rule violation`, `unjustified exclusion`, `missing test`, `note`), location in Rust (`file:line`), location in TypeScript (`file:line`), a concrete input with both outputs, and a test that would confirm it. If a category has no findings, say so in one line. Do not suggest improvements the source does not have. At most 1000 words. Say plainly what you did not check.
