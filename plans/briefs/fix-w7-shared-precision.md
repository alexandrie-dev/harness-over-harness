# w7: two shared-crate gaps found by the session-format writer

`hoh-llm` is accepted (commit `0136cbe`). The writer of `crates/hoh-session-format-v1-to-v2` (c4) found two places
where a shared crate cannot express what the source does; read its `PORT_NOTES.md`, section «Вопросы ведущему»,
items 1 and 3.

1. **`hoh_llm::AssistantStreamAccumulator::push_value` on malformed chunks** (item 3). Make it check what the source
   checks, in the source's order, at the source's moment: raw `block-start`/`block-end`/`usage`/`finish` after a
   JSON snapshot are not field-checked by the source; tool delta checks run id, name, argumentsDelta. Source:
   `packages/llm/llm/src/assistant-stream*.ts` (find the accumulator). Tests with each malformed chunk the source
   accepts or rejects differently today; correct streams must not change.
2. **`hoh_session_format::SessionFormatError::Format` has no cause** (item 1). The source passes `{ cause: error }`
   (`packages/session/session-format*/src`). Add a way to carry the cause without changing the error's class or
   message (look at how `crates/hoh-error` models causes and follow it); keep existing constructors working.
   Test that the cause is retrievable.

You own `crates/hoh-llm` and `crates/hoh-session-format` for this task only. Do not edit
`crates/hoh-session-format-v1-to-v2` (c4 will remove its workaround). `CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w7`. Done when tests, clippy `-D warnings` and fmt are clean
for both crates and for the crates that depend on them that build today (`cargo test` for each dependent you can
name; list them). Report: what changed, new public API, result lines. At most 300 words.
