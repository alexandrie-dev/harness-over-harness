# Review: hoh-session-persistence and hoh-session-projection against their TypeScript source

You are an independent reviewer in a porting project (DeepSeek Harness, TypeScript → HoH, Rust). The code was
written by another model (Codex). You only read. Do not run builds, tests, servers or any command that
writes; do not edit or create any file.

## Read
- `docs/porting/PORTING.md` (Russian) — binding rules; checklist in section 14 (and §6: tokio flush before sync_all).
- `docs/kernel.md` sections 1–3, 6, 7; `crates/hoh-session/PORT_NOTES.md`.
- Source: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/session/session-persistence` and
  `.../session/session-projection` (`src/`, `tests/`, `locale/`).
- Port: `crates/hoh-session-persistence`, `crates/hoh-session-projection` (`src/`, `tests/`, `PORT_NOTES.md`).

## What to check
1. Every exported function, type and service method; error kinds and texts (locale byte for byte); revision
   branding and storage validation (what is accepted/rejected).
2. Projection: registration order, snapshot/checkpoint/restore/hydrate semantics, change-feed ordering, behaviour on
   errors in readers, cleanup on dispose. The writer replaced a weak header identity with a private shim
   (lifetime limitation documented) — decide whether a stale or leaked entry can be observed.
3. Excluded source cases (JS getter during adoption; two thenable-assimilation cases) — confirm they cannot be
   observed in Rust.
4. Tests: sample at least two test files in full against source expected values.

## Triage rule
A divergence that follows from the Rust type system and cannot reach an observer is a `note`. A `bug` is an input
the ported API accepts that gives a different result, order of effects, error or text than the source.

## Output
Findings, most severe first: kind, Rust `file:line`, TypeScript `file:line`, concrete input with both outputs, a
confirming test. At most 800 words. Say what you did not check.
