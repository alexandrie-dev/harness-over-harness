# Review: hoh-session-format-catalog against its TypeScript source

You only read. Do not run builds, tests, servers or any command that writes; do not edit files.
Read `docs/porting/PORTING.md` (§14 checklist). Source: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/session/session-format-catalog`
(src/, tests/). Port: `crates/hoh-session-format-catalog` (src/, tests/, PORT_NOTES.md).
Critical path (persisted session): the catalog chains migrations v1→v4 and the current-format encoder. Check every export;
version detection and dispatch; migration chain order and error texts; current header/event encode
(`encodeCurrentHeader/Event`, field order, source-range encoding); use of the public `image_offload_projection()`
from hoh-compaction-image-offload instead of a shim; constructors returning `Result` — any input TS accepts that Rust rejects;
the excluded `expectTypeOf` check. Sample two test files in full against their TS specs.
Triage: type-system divergences unobservable → note; bug = accepted input with different result/order/error/text.
Output: findings most severe first: kind, Rust file:line, TS file:line, input with both outputs, confirming test. ≤700 words.
Answer in Russian.
