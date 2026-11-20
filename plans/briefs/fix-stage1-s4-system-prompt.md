# hoh-system-prompt: one round of fixes after review

You wrote `crates/hoh-system-prompt`. An independent Codex review (read-only) is at
`/mnt/toshiba/alexandrie/hoh/logs/review-hoh-system-prompt.codex.md`; read it in full. Source clone (read-only):
`/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness`, `packages/core/system-prompt/src/index.ts` (S) and
`vendor/cordis/src` (V). Before fixing each item, confirm it against the source yourself; if the source does not
support it, say so with lines and leave the code. Each fix gets a test in `tests/review_fixes.rs` that fails
before the fix.

Fix:
1. **Realm of assembly** (review 1, S:684, V/service.ts:64, V/utils.ts:163–204): the source dispatches to the
   caller's scope (`scopeTarget(this, scope)`), not the service's. You wrote the opposite; settle it from the
   source and the kernel's facilities (`docs/kernel.md` §6–7; `crates/hoh-scope` for how a caller's scope is
   taken).
2. **Waterfall mutations lost** (review 2, S:684–686): the tail must return the same presentation object the
   listeners mutate, so a later listener's added section survives. Test: two listeners as in the review.
3. **Explicit null in config** (review 3): `{"toolOrder":null}` must fail to load as in the source (state the
   error text you choose and why). Defaults for null fields (`{"includeHarnessIdentity":null}` etc.) are being
   fixed in the kernel for all packages (Schemastery's null rule, kernel round 7): add the tests for every
   defaulted field, and if the kernel change has not landed when you finish, mark them `#[ignore = "kernel round 7:
   schemastery null rule"]`.
4. **Change notifications are unfiltered** (review 4, S:448, V/events.ts:162–170): a dispatch whose first argument
   is the event-name string applies no receiver filter. Use the kernel's unfiltered dispatch
   (`ctx.root().dispatch(event).invoke(...)`, see `crates/hoh-kernel/PORT_NOTES.md`, contained dispatch).
5. **Integer-like variable keys** (review 7, S:128, 627–635): enumeration order of variables must be JavaScript's
   (array-index keys ascending first, then insertion order) wherever a listener or the renderer can see it, not
   only in the diagnostic helper. Add the missing diagnostics test from the review.

Not in this round (kernel decisions, record them in PORT_NOTES as inherited with the review's inputs): review 5
(listener removal during dispatch) and 6 (direct disposal logs a failing change listener).

Environment as before: `CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-s4`;
only `crates/hoh-system-prompt` (if a fix needs a kernel change, stop and report it). Done when `cargo test`,
`clippy --all-targets -D warnings`, `fmt --check` are clean and dependent crates `hoh-persona` and
`hoh-deliverable-file-references` still pass their tests. Report: per item confirmed/rejected with source lines,
what changed, result lines. At most 400 words.
