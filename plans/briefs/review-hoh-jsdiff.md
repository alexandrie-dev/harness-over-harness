# Independent review: `hoh-jsdiff`

You are reviewing one small Rust crate in `/home/save/Alexandrie/HarnessOverHarness` (Rust 1.97.1, edition 2024):
`crates/hoh-jsdiff`, a port of `structuredPatch`, `formatPatch`, `createPatch`, `createTwoFilesPatch` and
`diffLines` from the npm library `diff` (jsdiff) 9.0.0. The hunks it returns are stored in session logs and shown
to a model, so the port must return exactly the hunks the library returns for every pair of texts — the same
edit script, not merely a shortest one.

This is a read-only review. Do not create, edit or delete any file. Do not run builds, tests or any program other
than read-only shell commands. Read only inside this repository and `/mnt/toshiba/alexandrie/hoh/refs/jsdiff`
(the library's source at tag v9.0.0).

## What to read

- The task the writer had: `plans/briefs/stage1-m1-jsdiff.md`. The project's rules: `docs/porting/PORTING.md`.
- The port: `crates/hoh-jsdiff/src/diff/base.rs`, `src/diff/line.rs`, `src/patch/create.rs`, `tests/`,
  `PORT_NOTES.md`.
- The source: `src/diff/base.ts`, `src/diff/line.ts`, `src/patch/create.ts`, `src/util/string.ts`,
  `test/patch/create.js`, `test/diff/line.js`.

The lead has verified that 47 tests pass and has read the diff loop and the hunk builder against the source.

## What to look for

1. Any pair of texts for which the port's change list or hunks differ from the library's. Trace the loop of
   `base.ts:131-187` against `base.rs:56-121` with small inputs, including: the bounds of the diagonal loop (the
   source re-evaluates its upper bound on every iteration, the port computes it once), cleared and pruned
   entries of `bestPath`, the choice between insertion and removal, texts where one side is empty, texts
   differing only in the final newline, `\r\n` and lone `\r`.
2. `quote_file_name_if_needed` and `needs_quoting` (`create.rs:214-243`) against the source in `src/util/string.ts`:
   which characters trigger quoting, how non-ASCII and control characters are escaped.
3. `format_patch` against `formatPatch` for the default header options: `Index:` line, headers, the `-1` start of
   an empty range.
4. Limits: `max_edit_length` and `timeout_ms` against the source's `maxEditLength` and `timeout`, including the
   moment the clock is first read and what is returned when the limit is hit exactly.
5. Tests: compare each ported case with its source case; report dropped or weakened expectations and cases
   the notes claim that do not exist. Check the list of cases not ported against the stated reasons.
6. Porting rules: panics reachable from public functions (slicing, arithmetic underflow with hand-made `Hunk`
   values), `as` casts, recursion.

## Report

Numbered findings, most serious first. Each: kind (`bug`, `rule violation`, `hazard`, `missing test`, `false
note`, `note`), `path:line`, the concrete input, what the port returns and what the library returns, and the
source line it rests on. No style remarks. Then "Checked with no findings" and "Not checked". At most 600 words.
