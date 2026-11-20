# Review: hoh-session against its TypeScript source

You are an independent reviewer in a porting project (DeepSeek Harness, TypeScript → HoH, Rust). The code was
written by Codex. You only read: no builds, tests, servers, edits or new files.

`hoh-session` is the session service: 42 other packages wait for it, and its events are what every recorded
scenario compares byte for byte (the V4 session log). A divergence here breaks everything downstream.

## Read
- `docs/porting/PORTING.md` (Russian), checklist in section 14; `docs/kernel.md` sections 1–3, 6, 7;
  `crates/hoh-kernel/PORT_NOTES.md` (contained dispatch, unfiltered `ctx.root()`).
- Source (read-only): `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/session/session`
  (`src/index.ts`, `types.ts`, `surface.ts`, `request-header.ts`, `tool-history.ts`, `repair.ts`, `fork.ts`,
  `seq-ranges.ts`, `preparation.ts`, `known-event-types.ts`, `tests/`).
- Port: `crates/hoh-session` (`src/`, `tests/`, `PORT_NOTES.md`).

## What to check
1. Event appending: sequence numbers, timestamps, key order of each event and of the request header, unknown
   nested fields kept, what is validated and when, error texts.
2. Repair, fork and seq-ranges: the exact transformation of a damaged or forked log; edge cases (empty, single
   event, gaps, overlapping ranges).
3. Surface manager and preparation: window bounds, ordering, what listeners see and in what order; lifecycle on
   dispose.
4. Tool history: pairing of calls and results, missing results, ordering.
5. **The 57 excluded source test cases.** For each exclusion category in PORT_NOTES, sample cases and say whether
   the exclusion is justified by PORTING.md (JavaScript-only behaviour that cannot be reached from the Rust API) or
   whether it hides behaviour the Rust API can reach — then it is a `missing test`. Pay attention to "the
   depth-limit case".
6. Tests: sample at least three test files in full and check expected values are the source's exactly.

## Triage rule
A divergence that follows from the Rust type system and cannot reach an observer is a `note`. A `bug` is an input
the ported API accepts that gives a different result, order, error or log bytes than the source.

## Output
Findings, most severe first: kind (`bug`, `rule violation`, `missing test`, `note`), Rust `file:line`, TypeScript
`file:line`, concrete input with both outputs, a confirming test. At most 900 words. Say what you did not check.
