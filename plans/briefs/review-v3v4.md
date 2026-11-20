# Review: hoh-session-format-v3-to-v4 against its TypeScript source

You only read. Do not run builds, tests, servers or any command that writes; do not edit files.
Read `docs/porting/PORTING.md` (§14 checklist). Source: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/session/session-format-v3-to-v4`
(src/, tests/, fixtures). Port: `crates/hoh-session-format-v3-to-v4` (src/, tests/, PORT_NOTES.md).
This crate is on the critical path (persisted session). Check: every export; codec V4 (decode/encode, field order,
unknown fields, error texts); migration V3→V4 for every event kind; admission and relationship rules; the excluded
UTF-16 surrogate lines and the null-event — confirm unobservable or justified; TokenMeter/ToolResultPruner substitution —
does it hide a real behaviour the catalog or persistence will rely on; cwd validation delegated to V3 validator.
Sample two test files in full against their TS specs.
Triage: type-system divergences unobservable → note; bug = accepted input with different result/order/error/text.
Output: findings most severe first: kind, Rust file:line, TS file:line, input with both outputs, confirming test. ≤700 words.
Answer in Russian.
