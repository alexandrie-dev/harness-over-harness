# Review: hoh-util-time against its TypeScript source

You are an independent reviewer in a porting project (DeepSeek Harness, TypeScript → HoH, Rust). The code under review was written by a different model. You only read. Do not run builds, tests, servers or any command that writes; do not edit anything.

## Read
- `docs/porting/PORTING.md` (in Russian) — the binding porting rules; your checklist is its section 14.
- Source: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/util/time` (`src/`, `tests/`, `README.md`).
- Port: `crates/hoh-util-time` (`src/`, `tests/`, `PORT_NOTES.md`).

## What to check
1. Strict parsers (`instant.ts`, `duration.ts`): accepted and rejected forms, error codes and error texts byte for byte, range limits, leap seconds, offsets, zone names.
2. Cron (`cron.ts`): field grammar, ranges, steps, day-of-month versus day-of-week semantics, next-occurrence search across DST gaps and overlaps, bounds on iteration.
3. RRULE (`rrule.ts`): the writer dropped the `rrule` library and wrote the iteration in `src/rrule.rs`. The npm library the source delegates to is not vendored in the clone, so judge this part against RFC 5545 and against what `rrule.ts` itself validates, normalises and expects in `tests/rrule.spec.ts`: FREQ/INTERVAL/COUNT/UNTIL, BYDAY with ordinals, BYMONTHDAY negative values, BYSETPOS, BYWEEKNO, BYYEARDAY, WKST, DTSTART anchoring and time-zone conversion. Give concrete rules with the expected first occurrences where you believe the port is wrong.
4. Zone handling: the writer replaced `Intl` canonicalisation with a table of IANA links (`src/zone_links.rs`). Check that link resolution is applied wherever the source canonicalises and nowhere else.
5. Integer arithmetic: overflow, negative values, 2^53 limits, rounding direction.
6. Tests: every case of the six source spec files has a Rust counterpart with assertions that are not weaker; list missing or weakened cases.
7. Each entry in PORT_NOTES.md: is the stated reason true?

## Triage rule
This is a port to Rust, not an emulation of JavaScript. A divergence that follows from the Rust type system and cannot reach an observer (object identity, freezing, `NaN`/fractional inputs to an integer parameter, lone UTF-16 surrogates inside a `String`) is a `note`, not a `bug`. A `bug` is a case where an input that the ported API accepts gives a different result, error code or error text than the source gives for the same input.

## Output
A list of findings, most severe first. For each: kind (`bug`, `rule violation`, `missing test`, `note`), location in Rust (`file:line`), location in TypeScript (`file:line`), a concrete input with both outputs, and a test that would confirm it. If a category has no findings, say so in one line. Do not suggest improvements that the source does not have. At most 800 words. Say plainly what you did not check.
