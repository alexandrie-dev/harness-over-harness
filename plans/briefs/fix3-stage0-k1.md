# Kernel, sixth round: two gaps found by the loader, one rule to recheck

The fifth round is handed in (270 tests). Your dispute on item 3 is accepted: `vendor/cordis/src/fiber.ts:303-323`
drives a PENDING fiber that is disposed through UNLOADING (when `inertia` is empty) and then DISPOSED. The loader's
writer, building part B on your kernel, found two gaps; both are reproduced in
`crates/hoh-loader/tests/kernel_gaps.rs` (ignored tests) and described in `crates/hoh-loader/PORT_NOTES.md`,
"Вопросы ведущему" 1–2. Same boundaries and environment as before: you own only `crates/hoh-kernel` and
`crates/hoh-kernel-macros`; `CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-k1`.
Nothing that compiles today may stop compiling.

## 1. R54: an absent object equals its default only when there is a default

`config::equal_except_volatile` returns `true` for schema `block: {value: x-volatile}`, previous
`{"block":{"value":"old"}}`, next `{}`. The source (`vendor/loader/src/config/diff.ts:17-31`) compares
`a ?? schema.meta?.default` with `b ?? schema.meta?.default`; with no default the absent side stays `undefined`,
`isRecord` fails, and `deepEqual({value:"old"}, undefined)` is `false`: removing an ordinary parent is not a
volatile-only change, even when all its fields are volatile. Make the port do exactly what `equal` does (the order of
the checks: volatile first, then non-object/dict/backedge → raw equality, then defaults, then the record test), with
the source's cases as tests, and the loader's case `removal_of_an_ordinary_parent_is_not_a_volatile_only_change`
copied into your tests. Correct the wording of R54 in `docs/kernel.md` only through your notes (the lead moves it).

## 2. L19: the provision store, read-only, from a `Context`

`packages/boot/app-boot/src/startup-diagnostics.ts:75-100` reads `ctx.reflect.store` (every provision: its key,
owner fiber, realm/isolation scope) and `fiber.ctx[Context.isolate][service]` to report, for a service a row is
missing, which providers exist in other scopes. The kernel has `Kernel::snapshot()`, but a plugin that holds only a
`Context` cannot reach it. Give `Context` a read-only accessor that yields what that code reads, with the same
consistency as `Kernel::snapshot()` (committed view), and the isolation scope of a name for a given context. Name it
and shape it after the source; document it in PORT_NOTES with the source lines. Test with the loader's case
`context_only_startup_capture_retains_registered_isolated_providers` restated on the kernel alone.

## 3. R14 for an ordinary PENDING fiber

By the same source lines, an ordinary fiber that is disposed while PENDING also passes UNLOADING before DISPOSED
whenever `inertia` is empty at that moment; R14 lists Pending → Disposed. Establish from `fiber.ts` which ordinary
pending fibers emit UNLOADING and which do not, make the kernel do the same, and record `InternalStatus` sequences
in a test for each case. List every earlier test whose expectation changes and why.

## 4. A synchronous effect whose setup fails after its cleanup was registered

Found by the review of `hoh-scope`. In `vendor/cordis/src/fiber.ts:548-564`, when `_execute` throws, the source
runs `finalizeDisposal(dispose)` inside `try { … } finally { rejectSetup?.(reason) }`: if that cleanup throws
synchronously, its error propagates out of the `catch` before `throw reason`, so the caller receives the
**cleanup's** error and the setup's error is lost; a cleanup that returns a promise is awaited only for logging
(`logger.error`). The port returns the setup's error and logs the cleanup's (`effect "<label>": …`). Make the port
do what the source does for a synchronous cleanup error, keep the promise case as it is, state it in the rule
(R-number of the effect rules) with the lines. Test first: an effect whose body registers its cleanup, then fails
with `#1`, while the cleanup fails with `#2` — the caller gets `#2`. Also say, with the source lines, what the
source does with an error of a cleanup called through a direct disposer call (it reaches the caller) versus
during fiber unloading (`logger.error(reason)`), and whether the port matches.

## 5. Left over from the closure check of the fifth round (added after the round started)

The closure check of `ab81102` (Sonnet, read-only) found items 1, 2, 5 closed and 4 partly closed. Add:
- a test with a live listener whose async body does observable work before its first `await` and then stays
  pending: the order is A, B's prefix, C, and a task is spawned afterwards (no current test covers it);
- a test that a panicking factory leaves no owner record behind (item 4 of the fifth round asked for it);
- a test for R65 without a tokio runtime: a ready async listener and a sync listener work, a pending one is
  reported as the rule says; and say in R65 what happens when the inline prefix itself needs a runtime
  (timers, `tokio::spawn`) — today it is reported as `event listener panicked: …`;
- R66: the source's `:309` checks only `!this.inertia`, not whether early effects exist — correct the stated
  reason (this goes together with item 3).

## Done when

`cargo test -p hoh-kernel`, `cargo test -p hoh-kernel-macros`, clippy with `-D warnings` and fmt check clean; for
items 1 and 3 the test was written first and seen to fail; `cargo check --workspace --all-targets --keep-going`
names no crate that broke because of you. Your last message: result lines, one line per item, earlier tests changed
and why, at most 250 words.
