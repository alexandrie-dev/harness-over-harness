# Review: hoh-compaction-image-offload against its TypeScript source

You only read. Do not run builds, tests, servers or any command that writes; do not edit files.
Read `docs/porting/PORTING.md` (§14 checklist). Source: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/compaction/compaction-image-offload`
(src/, tests/, locale). Port: `crates/hoh-compaction-image-offload` (src/, tests/, PORT_NOTES.md: SHIM(hoh-compaction) for
`compaction/summary-error`, 6 of 8 `summary image offload` cases deferred, 5 tests in tests/image_offload.rs ignored because
hoh-agent-loop sends a request without the user message — that defect is being fixed elsewhere; do not count it here).
Critical path: `image_offload_projection()` is used by `hoh-session-format-catalog` — check the projection exhaustively
(every message/part kind, order, ids, `offloaded` flag, what is kept vs replaced, error texts). Then the plugin: row name
`compaction-image-offload`, dependencies, event names, the `image/offload` decision record, retry and cancellation logic
(statically, since 5 tests cannot run yet). Locale files byte-identical. Sample `projection.spec.ts` vs `tests/projection.rs` in full.
Triage: type-system divergences unobservable → note; bug = accepted input with different result/order/error/text.
Output: findings most severe first: kind, Rust file:line, TS file:line, input with both outputs, confirming test. ≤600 words. Russian.
