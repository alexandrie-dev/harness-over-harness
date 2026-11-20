# Review: hoh-skill against its TypeScript source

You are an independent reviewer in a porting project (DeepSeek Harness, TypeScript → HoH, Rust). The code was
written by a different model (Codex). You only read. Do not run builds, tests, servers or any command that
writes; do not edit or create any file.

## Read
- `docs/porting/PORTING.md` (Russian) — binding rules; checklist in section 14.
- `docs/kernel.md` sections 1–3, 6, 7; `crates/hoh-kernel/PORT_NOTES.md` (contained dispatch: a source
  `ctx.events.dispatch('emit', [name, …])` with a string first argument applies no receiver filter).
- Source (read-only clone): `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/skill/skill` (`src/`, `tests/`,
  `locale/`). The skill service merges skill providers; the texts it produces go into the system prompt and into tool results.
- Port: `crates/hoh-skill` (`src/`, `tests/`, `PORT_NOTES.md`). Neighbours for conventions: `crates/hoh-scope`,
  `crates/hoh-llm`.

## What to check
1. Every exported function, type and service method against the source: provider registration and selection
   (order, priority, scope/realm of the caller, what happens with none, two, or a disposed provider), tool
   definitions (names, descriptions, input schemas byte for byte — they go to the model), tool results and error
   texts (locale strings), events and their receivers.
2. Config schema and defaults, including explicit `null` (Schemastery: null takes the default; known kernel gap,
   only report a package-specific difference).
3. Key order of JSON the model or a client sees.
4. Tests: all 37 source cases ported with the source's expected values; sample at least two test files in full.

## Triage rule
A divergence that follows from the Rust type system and cannot reach an observer is a `note`. A `bug` is an input
the ported API accepts that gives a different result, order of effects, error or text than the source.

## Output
Findings, most severe first: kind (`bug`, `rule violation`, `missing test`, `note`), Rust `file:line`,
TypeScript `file:line`, concrete input with both outputs, a confirming test. At most 700 words. Say what you did
not check.
