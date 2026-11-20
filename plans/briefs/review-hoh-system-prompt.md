# Review: hoh-system-prompt against its TypeScript source

You are an independent reviewer in a porting project (DeepSeek Harness, TypeScript → HoH, Rust). The code under review was written by a different model (Claude Sonnet). You only read. Do not run builds, tests, servers or any command that writes; do not edit or create any file.

The system prompt this package assembles is compared byte for byte with 81 recorded prompt files of dsh (`crates/hoh-conformance/README.md` section 6.1 item 1; the files are `snapshots/**/system-prompt*.expected.md` in the source clone). Every section, its order, separators, blank lines and trailing newline matter.

## Read
- `docs/porting/PORTING.md` (in Russian) — binding rules; checklist in section 14.
- `docs/kernel.md` sections 1–3, 6, 7, 10; `crates/hoh-kernel/PORT_NOTES.md` (contained dispatch; a source `ctx.events.dispatch('emit', [name, …])` with a string first argument applies no receiver filter).
- Source: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/core/system-prompt` (`src/index.ts`, `tests/`, `locale/`).
- Port: `crates/hoh-system-prompt` (`src/lib.rs`, `tests/`, `PORT_NOTES.md`).

## What to check
1. Rendering: `render_prompt`, `render_context_sections`, `render_context_snapshot`, `join_context_sections` against the source, line by line — order of sections and contexts, `TOOL_ORDER_REST`, persona section names, separators, trimming, the ordering of names by UTF-16 code units. Take three recorded `system-prompt*.expected.md` files and trace by hand which sections produce which lines; say whether the port would produce the same bytes given the same registered sections (state your assumptions).
2. Events `system-prompt/assemble` and `system-prompt/change`: when they fire, in what order relative to registration, what listeners receive, what a failing listener does. The writer says a panicking `change` listener turns into a registration error and rolls the registration back: is that what the source does?
3. The scope of dispatch: the writer says the filter takes the service's scope, not the caller's. Establish from the source what receiver, if any, is passed to each dispatch, and whether the port matches.
4. `AssembleContext`: the source lets other packages add fields by declaration merging; the port uses typed extensions (`with_extension`, `extension::<T>()`). Does any field of `AssembleContext` that the source reads inside this package get lost or change meaning? Which packages in the source add fields (search the clone), and can each of them use the Rust form?
5. Configuration and its defaults; `toolOrder: null` (not tested by the writer); key order of variables with integer-like keys.
6. Service methods taking `ctx: &Context` first and returning `Disposer` (docs/kernel.md section 7): consistent with the other ported services (`crates/hoh-scope`, `crates/hoh-llm`)?
7. The plugin's name: what row name and service name the source uses, what `crates/hoh-official-bundles` and the bundles' `cordis.patch.yml` expect, and whether `NAME` in the port matches.
8. Tests: 79 source blocks reported ported, 4 added. Sample at least two test files in full and check expected strings are the source's exactly.

## Triage rule
This is a port to Rust, not an emulation of JavaScript. A divergence that follows from the Rust type system and cannot reach an observer is a `note`. A `bug` is a case where an input the ported API accepts gives a different result, order of effects, error, text or prompt bytes than the source.

## Output
A list of findings, most severe first. For each: kind (`bug`, `rule violation`, `missing test`, `note`), location in Rust (`file:line`), location in TypeScript (`file:line`), a concrete input with both outputs, and a test that would confirm it. If a category has no findings, say so in one line. At most 900 words. Say plainly what you did not check.
