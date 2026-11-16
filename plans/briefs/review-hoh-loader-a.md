# Independent review: `hoh-loader`, part A

You are a reviewer. Read-only: do not edit, create or delete any file; do not run builds, tests, cargo, Node,
servers or browsers; do not use git to change anything; no network. Other agents are writing code in this
repository right now. Everything you need is on disk.

## What is reviewed

A TypeScript agent harness is being ported to Rust. Another writer produced part A of the loader crate: the pure
half (text in, data out). You check it against its specification and its TypeScript sources.

- Port: `/home/save/Alexandrie/HarnessOverHarness/crates/hoh-loader` (`src/yaml.rs`, `src/patches.rs`, `src/expr.rs`,
  `src/profile.rs`, `src/lib.rs`, `tests/`, `PORT_NOTES.md`), about 2,200 lines of source.
- Specification: `/home/save/Alexandrie/HarnessOverHarness/docs/kernel.md`, sections 2.9 (public API), 5 (rules
  L1–L19 and the closed expression grammar of 5.5 with the full list of 76 expressions), 8.2–8.4 (test plan),
  10 (the lead's decisions). The writer's task: `plans/briefs/stage0-l1-loader-a.md`.
- Sources, under `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness`: `vendor/include/src` (`parseEntryYaml`,
  `patch.ts`), `vendor/loader/src`, `vendor/group`, `packages/boot/app-boot/src` (`index.ts`, `profile.ts`,
  `profile-context.ts`, `bundle-platform.ts`, `profile-sanitize.ts`), the tests `scripts/cordis-entry-yaml.spec.ts`,
  `scripts/cordis-entry-commit.spec.ts` and `packages/boot/app-boot/tests/`. The real configuration files the loader
  must read are `packages/bundle/*/cordis.patch.yml`, `packages/bundle/web-app/presets/*.patch.yml` and the `*.yml`
  under `snapshots/`.
- Project rules: `/home/save/Alexandrie/HarnessOverHarness/docs/porting/PORTING.md` (sections 4, 7, 8, 10 and the
  reviewer's checklist in 14).

The lead already ran the crate's tests, clippy and formatting (62 tests pass). Do not repeat that. Your value is
reading both sides.

## What to look for

1. **YAML reading.** The source reads with js-yaml 4 and its `JSON_SCHEMA`. For which inputs does the port resolve a
   plain scalar differently (null forms, booleans in three cases, decimal, `0x`, `0o`, `0b` integers, floats with
   dot or exponent, `.inf`, `.nan`, leading `+`, underscores, leading zeros), treat quoting, block scalars, anchors
   and aliases, merge keys, duplicate keys, tags, multiple documents, empty documents or tabs differently, or
   report a different error position? Does a `!!js` scalar always become an expression node, also inside flow
   collections and as a mapping value with a quoted or block form?
2. **YAML writing and `render_dump`.** Where a test of the source compares dumped text, is the port's text the same
   (indentation, quoting rules, key order, expression nodes, comments with origins)? Can the writer emit text that
   its own reader reads back differently (strings that look like numbers, booleans or null; strings with `: `,
   `#`, leading or trailing spaces, newlines; empty strings; keys needing quotes)?
3. **Patches and layers.** Compare `apply_patches` and `compose` with `vendor/include/src/patch.ts` and
   `composeEntries`: the order of checks, each warning text byte for byte, what a failed patch leaves behind, how
   inserted groups become addressable, `null` meaning "key absent", deep copy of the base, the origin records.
4. **Expressions.** For each construct of the grammar in 5.5: does the port parse exactly that grammar (and nothing
   more), with JavaScript's precedence and associativity for the 76 listed expressions; does evaluation follow
   JavaScript (`&&` and `||` return operands, `??` only for null and undefined, truthiness of `0`, `""`, `NaN`,
   `===` without coercion, string `+`, `Number(..)` of strings with spaces, empty strings, hex, `JSON.parse`
   errors, `[..].includes`), Node's `path.join`, `path.resolve` and `path.dirname`, and
   `decodeURIComponent(new URL(rel, 'file://' + x).pathname)`? Take several of the listed expressions and evaluate
   them by hand both ways.
5. **Profile.** Compare `Profile::load` with `loadProfileDirectory` and `readProfilePatches`: the order of layers,
   which failures skip a bundle and which stop the start, platform matching, missing optional files versus named
   files that must exist.
6. **Tests.** Pair the source cases with the Rust tests by name and list every case that is missing, weaker or
   changed in meaning. Check each reason given in `PORT_NOTES.md` for a case that was not ported.
7. **Hazards the source cannot have**: panics on slicing or indexing, integer overflow, unbounded recursion on
   nested input, quadratic work on large files, paths built from untrusted text.
8. **False or incomplete statements in `PORT_NOTES.md`**, and deviations from section 2.9 that it does not report.

Do not report style, naming or possible refactorings.

## Report

In English, at most 800 words, as your final message. Findings numbered, most serious first. For each: class
(`bug`, `rule violation`, `missing test`, `false note`, `hazard`, `note`); a concrete input; what the source or
JavaScript does (`path:line` where there is one) and what the port does (`path:line`); a test that would confirm
it. If you are not sure a difference is real, say so and say what would settle it. Then "Checked with no findings"
naming what you actually compared, and "Not checked" naming what you did not get to.
