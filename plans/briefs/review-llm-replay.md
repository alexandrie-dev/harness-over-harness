# Review: hoh-llm-replay against its TypeScript source; closure of hoh-conformance fix2

You only read. Do not run builds, tests, servers or any command that writes; do not edit files.
Read `docs/porting/PORTING.md` (§14 checklist). Source: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/test-support/llm-replay`
(src/, tests/). Port: `crates/hoh-llm-replay` (src/, tests/, PORT_NOTES.md: private shims for catalog and compaction).
Part 1 — llm-replay: every export; StreamChunk replay, overrides, session binding, provider catalog, cancellation,
pacing; error texts; the 28 unported source clauses — confirm each is unobservable or justified; sample two test files
in full. Shims: only uses that would be wrong with the real catalog/compaction.
Part 2 — conformance fix2: the writer's report says items 1–7 of the second conformance review are fixed
(`plans/briefs/fix2-stage0-cf1-conformance.md`); check each closed / not closed against `crates/hoh-conformance`.
Triage: type-system divergences unobservable → note; bug = accepted input with different result/order/error/text.
Output: findings most severe first: kind, Rust file:line, TS file:line, input with both outputs, confirming test. ≤700 words.
