# Review: hoh-credentials-local against its TypeScript source

You only read. Do not run builds, tests, servers, browsers or any command that writes; do not edit files.
Writer s9 reports: cargo test 130/0, clippy -D warnings and fmt --check clean (target-sonnet-cred).

Read `docs/porting/PORTING.md` (§14 checklist). Source: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/credentials/credentials-local`
(src/, tests/, locale). Port: `crates/hoh-credentials-local` (src/, tests/, PORT_NOTES.md — read its declared differences first).

This crate stores the founder's model API key in the desktop milestone. Priority:
1. Secret never leaks: Debug, Display, error texts, warnings, parse diagnostics, logs, panic messages, temp files.
2. File safety: 0600 file / 0700 dir, permission check on load, reload and before every write; atomic write; cross-process
   lock (30 s); re-read before edit; post-edit re-parse vs expected snapshot. Races between watcher reload and write.
3. Own YAML editing over saphyr-parser ranges: untouched lines and comments byte-identical; any input where the edit
   corrupts the document, loses another record, or writes a value that re-parses differently (quoting, special strings
   like `yes`, `~`, `0x1`, multi-line, unicode, keys needing quotes).
4. Watcher (notify instead of chokidar): stabilityThreshold/pollInterval semantics, missed events, delete/recreate.
5. Locale byte-identical; sample `local.spec.ts` and `records.spec.ts` vs Rust tests in full.

Triage: type-system divergences unobservable → note; bug = accepted input with different result/order/error/text, data
loss, or secret exposure. Output: findings most severe first: kind, Rust file:line, TS file:line, input with both outputs,
confirming test to add. ≤700 words. Russian.
