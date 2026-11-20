# Closure check 2: hoh-tools (items still open) and hoh-scope weak keys

You only read. Do not run builds, tests, servers or anything that writes; do not edit files.
Lead ran outside the writers' sandbox: `cargo test -p hoh-scope -p hoh-tools -p hoh-tool-executor-native` 266/0, clippy -D
warnings and fmt --check clean (`/mnt/toshiba/alexandrie/hoh/logs/s7c.lead-test.txt`).

Earlier review: `/mnt/toshiba/alexandrie/hoh/logs/rv-tools.report.md`; first closure check: `/mnt/toshiba/alexandrie/hoh/logs/rv-tools2.full.md`.
TypeScript source: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/core/tools` (and core/scope for weak keys).

Still open after the first check, now reported fixed by writer s7c:
- 1: restore after all wrappers (the outer wrapper saw 42);
- 3: order without start confirmation;
- 4: per-scope executor table on weak scope keys (additive `WeakScopeKey` in hoh-scope) — no leak of scopes, no reuse of a
  dead scope's entry, existing hoh-scope behaviour unchanged;
- 13: test through the owning plugin;
- 7': prepare without tools — TS gives TypeError, not TOOL_EXECUTOR_UNAVAILABLE;
- 8': tuple panic in the output schema outside the normalisation catch (pipeline.rs:756).

For each: closed / partly / not closed, Rust file:line, test that proves it, and whether that test would fail on the old
behaviour. Report any new defect the fix introduced in hoh-tools, hoh-scope or hoh-tool-executor-native. ≤500 words, Russian.
