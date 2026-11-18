# Review: hoh-credentials against its TypeScript source

You are an independent reviewer in a porting project (DeepSeek Harness, TypeScript → HoH, Rust). The code under review was written by a different model (Codex). You only read. Do not run builds, tests, servers or any command that writes; do not edit any file. Write nothing to disk: your answer is your printed reply.

## Read
- `docs/porting/PORTING.md` (in Russian) — the binding porting rules; your checklist is its section 14.
- Kernel: `docs/kernel.md` sections 1–3 and 10; `crates/hoh-kernel/PORT_NOTES.md`, section "Четвёртый круг" — the `Contained` event mode (`on_contained`, `on_contained_async`, `Context::dispatch(..).invoke(report)`), which reproduces the source's `ctx.events.dispatch('emit', …)` with per-listener failure containment.
- Source: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/credentials/credentials` (`src/`, `tests/`, `README.md`), and `vendor/cordis/src/events.ts` in the same clone for the dispatch semantics.
- Port: `crates/hoh-credentials` (`src/`, `tests/`, `PORT_NOTES.md`), committed in `4f934b5`.

## What to check
1. The public types and services: names, fields, serialized shapes and key order, defaults.
2. The notifications `credentials/*-updated`: when they are sent, with what payload, in what order relative to the commit; that a failing listener (sync throw, async rejection) does not undo the commit, that later listeners still run, that async listeners are not awaited, and what is logged (the writer says two warning records per failure — check against the source).
3. Error types and texts (PORTING.md section 7: the base class of the source decides the type; native `TypeError` stays separate from `HarnessError`).
4. Default trait helpers that take an explicit context: is any behaviour of the source lost by that?
5. Tests: every one of the eight source cases plus the two fan-out cases has a Rust counterpart with assertions that are not weaker; the memory fixture used instead of the borrowed file provider: does it test the same thing?
6. Each entry in `PORT_NOTES.md`.

## Triage rule
This is a port to Rust, not an emulation of JavaScript. A divergence that follows from the Rust type system and cannot reach an observer is a `note`, not a `bug`. A `bug` is a case where an input that the ported API accepts gives a different result, order of effects, error or text than the source gives for the same input. Statements about third-party libraries or about Node made from memory must be checked against their source in the clone, or marked "not checked".

## Output
A list of findings, most severe first. For each: kind (`bug`, `rule violation`, `missing test`, `note`), location in Rust (`file:line`), location in TypeScript (`file:line`), a concrete input with both outputs, and a test that would confirm it. If a category has no findings, say so in one line. Do not suggest improvements that the source does not have. At most 800 words. Say plainly what you did not check.
