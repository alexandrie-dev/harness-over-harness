# Review: hoh-session-persistence-jsonl against its TypeScript source

You only read. Do not run builds, tests, servers or any command that writes; do not edit files.
Read `docs/porting/PORTING.md` (§14 checklist). Source: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/session/session-persistence-jsonl`
(src/, tests/, locales). Port: `crates/hoh-session-persistence-jsonl` (src/, tests/, PORT_NOTES.md with shims and 96 deferred cases).
Critical path: the engine's persisted session lives here. Focus on data safety: append and durable flush order (fsync of file
and directory), recovery of a torn tail (what is dropped, what is kept, error vs silent), locks and leases (two writers,
stale lease, crash), generation switch atomicity, zstd framing, migration verifier, error texts, locale files byte-identical.
Shims: historical read / auto-migration V0→V4, child catalog, frozen V4 validation — list which uses would be wrong once the real
`hoh-session-format-catalog` / `hoh-session-format-v3-to-v4` (now accepted) replace them. Check the 96 deferred cases: any that
hides behaviour the window-chat milestone needs on Linux. Sample two test files in full against their TS specs.
Triage: type-system divergences unobservable → note; bug = accepted input with different result/order/error/text;
any data loss is P1.
Output: findings most severe first: kind, Rust file:line, TS file:line, input with both outputs, confirming test. ≤800 words.
Answer in Russian.
