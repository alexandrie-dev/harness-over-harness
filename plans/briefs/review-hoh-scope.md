# Review: hoh-scope against its TypeScript source

You are an independent reviewer in a porting project (DeepSeek Harness, TypeScript → HoH, Rust). The code under review was written by a different model (Codex). You only read. Do not run builds, tests, servers or any command that writes; do not edit anything.

## Read
- `docs/porting/PORTING.md` (in Russian) — the binding porting rules; your checklist is its section 14.
- Kernel the port runs on: `docs/kernel.md` sections 1–3 and 10, and `crates/hoh-kernel/PORT_NOTES.md`, section "Четвёртый круг" (`Context::child`, `Fiber::disposer`, `matches_filter`/`and_filter`, inherited resolution path). The kernel's fifth round is being written in the working tree right now; review hoh-scope against the API as it stands, and say if something hoh-scope relies on is changing there.
- Source: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/core/scope` (`src/index.ts`, `src/store.ts`, `tests/`, `README.md`) and, for the kernel semantics it relies on, `vendor/cordis/src` in the same clone.
- Port: `crates/hoh-scope` (`src/`, `tests/`, `PORT_NOTES.md`), committed in `4f934b5`.

## What to check
1. Scope creation and disposal: synchronous creation through `Context::child`, the order in which early effects, the store and listeners are undone, what a disposed scope does when used.
2. The store (`store.ts`): keys, lookups, ownership of values, what happens on dispose, events and their order.
3. Filters: base filter composition with `and_filter` and short-circuiting; what a listener on a scoped context sees.
4. Service resolution through a scope during and after cleanup, versus the source.
5. Error type and texts (`ScopeError`): PORTING.md section 7 — the base class of the source decides the type.
6. Tests: every one of the 21 source cases has a Rust counterpart with assertions that are not weaker. Check the additional tests too.
7. Each entry in `PORT_NOTES.md`, including the listed Rust adaptations (`Arc` receivers, `ScopeFilter`, `raw_dispose()`): is each one needed?

## Triage rule
This is a port to Rust, not an emulation of JavaScript. A divergence that follows from the Rust type system and cannot reach an observer is a `note`, not a `bug`. A `bug` is a case where an input that the ported API accepts gives a different result, order of effects, error or text than the source gives for the same input.

## Output
A list of findings, most severe first. For each: kind (`bug`, `rule violation`, `missing test`, `note`), location in Rust (`file:line`), location in TypeScript (`file:line`), a concrete input with both outputs, and a test that would confirm it. If a category has no findings, say so in one line. Do not suggest improvements that the source does not have. At most 800 words. Say plainly what you did not check.
