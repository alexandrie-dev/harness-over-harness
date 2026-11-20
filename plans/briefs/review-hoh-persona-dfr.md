# Review: hoh-persona and hoh-deliverable-file-references against their TypeScript source

You are an independent reviewer in a porting project (DeepSeek Harness, TypeScript → HoH, Rust). The code was
written by a different model (Claude Sonnet). You only read. Do not run builds, tests, servers or any command
that writes; do not edit or create any file.

Both packages add sections to the system prompt, which is compared byte for byte with dsh's recorded
`snapshots/**/system-prompt*.expected.md`.

## Read
- `docs/porting/PORTING.md` (Russian) — binding rules; checklist in section 14.
- `docs/kernel.md` sections 1–3, 6, 7; `crates/hoh-system-prompt/src/lib.rs` (the service these use).
- Source (read-only clone): `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/preset/persona` and
  `packages/deliverables/deliverable-file-references` (`src/`, `tests/`, `locale/`, `package.json`).
- Port: `crates/hoh-persona`, `crates/hoh-deliverable-file-references` (`src/lib.rs`, `tests/`, `PORT_NOTES.md`).

## What to check
1. Section texts byte for byte (including U+2013 and trailing newlines), section names, order and when sections
   are registered or removed (config change, `complete`, `includeRuntimeContext`, empty prefix/suffix).
2. Config schema: names, defaults, which fields are optional; the writer replaced `?? ''`/`?? true` with serde
   defaults — can any accepted input now give a different result?
3. Plugin and service names and dependencies against the source and against what `crates/hoh-official-bundles`
   and the bundles' `cordis.patch.yml` expect.
4. Tests: all `persona.spec.ts` cases and the deliverable test ported with the source's expected strings; the
   writer copied four `system-prompt.expected.md` into `tests/fixtures/` — are they identical to the source's?

## Triage rule
A divergence that follows from the Rust type system and cannot reach an observer is a `note`. A `bug` is an input
the ported API accepts that gives a different result, order, error or prompt bytes than the source.

## Output
Findings, most severe first: kind (`bug`, `rule violation`, `missing test`, `note`), Rust `file:line`,
TypeScript `file:line`, concrete input with both outputs, a confirming test. At most 600 words. Say what you did
not check.
