# Review: hoh-loader part B against its TypeScript source

You are an independent reviewer in a porting project (DeepSeek Harness, TypeScript → HoH, Rust). The code under review was written by a different model (Codex). You only read. Do not run builds, tests, servers or any command that writes; do not edit or create any file. Your answer is your printed reply.

## Read
- `docs/porting/PORTING.md` (in Russian) — binding rules; your checklist is its section 14.
- `docs/kernel.md` sections 2.9 (the loader's public API), 5 (loader rules `L1`…), 3.8, 8.2–8.4, 10 and 10.1 (10.1 wins over older rule texts).
- The task the writer was given: `plans/briefs/stage0-l1-loader-b.md`.
- Source, read-only, `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness`: `vendor/loader/src`, `vendor/include/src`, `vendor/group/src`, `packages/boot/app-boot/src` (`index.ts`, `startup-diagnostics.ts`, `profile-sanitize.ts`, `config-schema/`), `packages/boot/config-editor/src`, their tests, and `scripts/loader-volatile-update.spec.ts`, `scripts/cordis-entry-commit.spec.ts`.
- Port: `crates/hoh-loader` (`src/`, `tests/`, `PORT_NOTES.md`). Part A (YAML, patches, expressions, profile) was reviewed and accepted earlier; review part B: `registry.rs`, `runtime.rs`, `config_editor.rs`, `config_schema.rs`, `startup_diagnostics.rs`, `profile_sanitize.rs`, `identity.rs`, the changes to `lib.rs`, and the tests. `git diff HEAD -- crates/hoh-loader` and `git status crates/hoh-loader` (read-only git) show what part B changed.

## What to check
1. Rows as fibers: `disabled` (value and expression), `inject`, `isolate`, `intercept`, configuration resolution, row status; the order in which rows are created, updated and removed.
2. `Group`, `cordis:group`, `cordis:include` with their dsh aliases, carriers (L7).
3. `Loader` as service `loader`: `entries`, `resolve`, `create`, `remove`, `settled`, `recompose` (create / remove / update in place / volatile commit / rebuild — section 5.3), `audit`, `dump`, `boot`. Concurrency: recomposition while rows load, removal during activation.
4. Configuration reload and start-up diagnostics of `app-boot`; the part of `profile-sanitize.ts` that needs the tree.
5. `ConfigEditor`: what it writes back (decision O4: the journal of inclusions is not ported) — the writer says the YAML is rewritten canonically without keeping comments or layout. Does the source keep them (`packages/boot/config-editor/src/index.ts:109-135`, the `yaml` Document API)? If it does, that is a difference a user sees in his own file: say so with the source lines.
6. Duplicate explicit ids: what the source's tree does (`vendor/loader/src/config/tree.ts`) and what the port does.
7. Tests. The writer reports many source cases excluded: app-boot 40 of 62, profile 23 of 31, schemastery-json-schema 46 of 47, config-schema 9 of 41, loader-volatile-update 4 of 23 (plus 1 blocked), config-pattern 3 of 3. For each group of excluded cases, read the reason in `PORT_NOTES.md` and say whether it holds for the ported API (excluded because the feature is not ported by a recorded decision — fine; excluded because it was hard — not fine). Check that ported cases keep the source's expected texts exactly.
8. The two kernel gaps in `tests/kernel_gaps.rs` (R54, L19): are the workarounds in the loader correct until the kernel is fixed?
9. Each entry in `PORT_NOTES.md` "Отличия".

## Triage rule
This is a port to Rust, not an emulation of JavaScript. A divergence that follows from the Rust type system and cannot reach an observer is a `note`, not a `bug`. A `bug` is a case where an input the ported API accepts gives a different result, order of effects, error or text than the source. Statements about third-party libraries (js-yaml, yaml, schemastery) or Node made from memory must be checked against their source in the clone (`/mnt/toshiba/alexandrie/hoh/refs/` holds js-yaml) or marked "not checked".

## Output
A list of findings, most severe first. For each: kind (`bug`, `rule violation`, `unjustified exclusion`, `missing test`, `note`), location in Rust (`file:line`), location in TypeScript (`file:line`), a concrete input with both outputs, and a test that would confirm it. If a category has no findings, say so in one line. Do not suggest improvements the source does not have. At most 1000 words. Say plainly what you did not check.
