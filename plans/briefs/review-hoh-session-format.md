# Review: hoh-session-format against its TypeScript source

You are an independent reviewer in a porting project (DeepSeek Harness, TypeScript → HoH, Rust). The code under review was written by a different model. You only read. Do not run builds, tests, servers or any command that writes; do not edit anything.

## Read
- `docs/porting/PORTING.md` (in Russian) — the binding porting rules; your checklist is its section 14.
- Source: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/session/session-format` (`src/`, `tests/`, `README.md`).
- Port: `crates/hoh-session-format` (`src/`, `tests/`, `PORT_NOTES.md`).

## What to check
1. Version classification of a physical header, selection and composition of the adjacent migration chain, uniqueness checks, and the order in which rows are consumed and stages are driven (`chain.ts`, `catalog.ts`, `context.ts`).
2. Error kinds, error texts and the order of checks (which error wins when several conditions fail).
3. Lossless JSON value checks (`json.ts`): numbers (integers above 2^53, `-0`, exponent forms), key order, duplicate keys, nesting.
4. File-name rules (`filename.ts`): accepted and rejected names, version suffixes, leading zeros, limits.
5. Ownership: the source passes caller-owned values through stateful stages without copies; check the port neither clones where the source moves in a way that changes observable results nor shares mutable state.
6. Tests: every case of the four source spec files has a Rust counterpart with assertions that are not weaker; list the source cases the writer declared "not expressible" and say for each whether that is true for the ported API.
7. Each entry in PORT_NOTES.md, including the listed extras beyond the source (`_labeled` variants, `session_format_count_u64`, `is_unsupported_migration`): is each one needed by the port or should it go?

## Triage rule
This is a port to Rust, not an emulation of JavaScript. A divergence that follows from the Rust type system and cannot reach an observer (object identity, freezing, `NaN`/fractional inputs to an integer parameter, lone UTF-16 surrogates inside a `String`) is a `note`, not a `bug`. A `bug` is a case where an input that the ported API accepts gives a different result, error code or error text than the source gives for the same input.

## Output
A list of findings, most severe first. For each: kind (`bug`, `rule violation`, `missing test`, `note`), location in Rust (`file:line`), location in TypeScript (`file:line`), a concrete input with both outputs, and a test that would confirm it. If a category has no findings, say so in one line. Do not suggest improvements that the source does not have. At most 800 words. Say plainly what you did not check.
