# Review: hoh-tools against its TypeScript source

You are an independent reviewer in a porting project (DeepSeek Harness, TypeScript → HoH, Rust). The code was
written by another model (Sonnet). You only read. Do not run builds, tests, servers or any command that
writes; do not edit or create any file.

## Read
- `docs/porting/PORTING.md` (Russian) — binding rules; checklist in section 14.
- `docs/kernel.md` sections 1–3, 6, 7; `crates/hoh-kernel/PORT_NOTES.md`.
- Source (read-only clone): `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/core/tools` (`src/`, `tests/`, `locale/`).
- Port: `crates/hoh-tools` (`src/`, `tests/`, `PORT_NOTES.md`). `src/shim.rs` stands in for hoh-agent and
  hoh-user-approval (not yet written) — judge only uses of the shim that would be wrong with the real packages.

## Already found by an earlier review (confirm, and give the exact fix location)
1. `src/batch.rs:252` vs `batch.ts:163`: two parallel-safe calls A, B, concurrency 2 — TS dispatches
   `execute(A)` before `pre(B)`; Rust polls B's preparation first.
2. `src/pipeline.rs:125` vs `pipeline.ts:68`: constructing the pipeline without a `tools` service fails at once in
   Rust; TS stores `undefined` and `describe([])` still works.
3. Source `index.ts:107` and `pipeline.ts:85,91,94` use `WeakMap` keyed by scope / run context. Check how the port
   holds these (the writer says the per-scope executor table holds `ScopeKey` strongly) and whether entries are
   released when the scope or run ends — a leak or a stale entry observable through a later scope is a `bug`.

## What else to check
1. Every exported function, type and service method: registration layers (global/scoped), visibility and
   activation of deferred tools, name collisions, order of events (`tools/pre-execute`, `tools/execute`, …) and
   of effects in batches (concurrency, cancellation, abort, finalizers), error texts (locale strings byte for byte).
2. JSON-schema handling written without a library: every accepted/rejected schema and its error text.
3. JSON key order the model or a client sees.
4. Tests: the writer reports 194 Rust tests and lists unported source cases in `PORT_NOTES.md` with reasons —
   check that each skipped case really cannot be observed in Rust; sample at least three test files in full.

## Triage rule
A divergence that follows from the Rust type system and cannot reach an observer is a `note`. A `bug` is an input
the ported API accepts that gives a different result, order of effects, error or text than the source.

## Output
Findings, most severe first: kind (`bug`, `rule violation`, `missing test`, `note`), Rust `file:line`,
TypeScript `file:line`, concrete input with both outputs, a confirming test. At most 900 words. Say what you did
not check.
