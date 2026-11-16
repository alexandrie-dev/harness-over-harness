# Kernel, fifth round: after the review of the fourth

The fourth round is accepted as a base (commit `86647a0`; the lead's run of the kernel and seven crates on
it: 409 tests). An independent reviewer read what was added. He confirmed the snapshot semantics against
`events.ts`, the isolation of panics and of the `report` closure, the locking, `Fiber::disposer`,
`matches_filter`/`and_filter` and the inherited resolution path. What follows is what he found, as decided
by the lead, plus one request from a consumer. Same boundaries and commands as before: you own only
`crates/hoh-kernel` and `crates/hoh-kernel-macros`; environment
`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-k1`.
Four writers build against the kernel right now (`hoh-scope`, `hoh-credentials`, `hoh-llm`,
`hoh-storage-domain`): nothing that compiles today may stop compiling, and keep the crate building between
your steps.

## 1. An async contained listener starts inside `invoke`, as an `async` function starts inside the call

`events.rs`, `Callbacks::invoke`, the async arm: the factory runs inline, but the future it returns is
polled for the first time by a spawned task. In JavaScript an `async` function runs synchronously up to
its first `await`: `callback(...args)` in `packages/core/session/src/index.ts:413` executes the listener's
prefix before the loop goes on to the next listener. With listeners sync A, `async` B whose body begins
with `log.push("B")`, sync C, the source's order is A, B, C; the port's is A, C, B, and an async observer
that reads the emitter's state in its prefix reads it after later listeners and after the emitter's next
statements. `tests/contained.rs` does not see it because its async listener pushes in the factory, outside
the future.

Decided:
- `invoke` polls the future of an async listener once, inline, on the caller's thread, before it calls the
  next listener;
- a future that finishes at that poll never becomes a task: `Ok` ends it; `Err` is reported as `Rejected`
  after the loop over the snapshot has finished and before `invoke` returns (in JavaScript the rejection
  handler runs in a later microtask, never between two listeners);
- a future that is still pending goes to a task of the listener's fiber as today (cancelled with the
  fiber, nothing reported then);
- a listener whose fiber was closed between `dispatch()` and `invoke()` — today skipped silently, while a
  removed synchronous listener is still called, as the source's array would call both: the factory runs
  and the future gets its one inline poll like any other; if it is still pending it is dropped (its fiber
  is gone, so it is cancelled with it). Say so in the rule;
- a panic during the inline poll is contained like a panic of the factory.

Tests: A, B, C with B's push inside the `async` block; a future that fails at its first poll — reported as
`Rejected`, after the last listener was called and before `invoke` returns; a future that completes at its
first poll leaves no task and no effect record behind; the window "disposed after `dispatch()`, before
`invoke()`" for a synchronous and for an async listener.
Rewrite R65 (and the example) so that the rule states what runs inline and what does not.

## 2. Effect labels built from data

`Context::try_effect` and the effect record take `label: &'static str`. The source builds labels from data
(`packages/llm/deepseek-llm-api-extensions/src/index.ts:99`:
``deepseekLlmApiExtensions.register(${JSON.stringify(fieldName)})``), and they appear in teardown
diagnostics. Accept `impl Into<Arc<str>>` wherever a label of an effect is given (every public function
that takes one: check `effect`, `try_effect`, `effect_async`, `spawn` and the others) and keep it as
`Arc<str>`; a string literal keeps working unchanged. A test with a label made by `format!`, seen in the
diagnostic that prints labels.

## 3. `Context::child` disposed before its first activation: the same status sequence as the source

`fiber.rs`, the drive branch for a pre-activated child that is disposed while Pending, goes
Pending → Unloading → Disposed, so `internal/status` observers see a transition that an ordinary pending
fiber does not make (R14 lists Pending → Disposed). Establish from `vendor/cordis/src/fiber.ts` what the
source emits for `const fiber = ctx.plugin(fn); fiber.ctx.effect(...); fiber.dispose()` before the fiber
ever became active — the statuses and the moment the early effects are undone — and make the child do
exactly that. Test: record `InternalStatus` for that sequence. State in R66 what was established, with the
source lines, and which of R14 and R17 the child departs from and why (it holds an open activation while
Pending).

## 4. Tests the reviewer found weak or missing

- `tests/scoped_context.rs:279-285`: the assertion that `isolate` hides the creator's resource runs after
  the provision was withdrawn, so `None` follows without any barrier. Write tests that exercise
  `ServiceAncestor.barriers` and the realm-mismatch `break` with the provision still published: what a
  scoped context sees across an `isolate` boundary of its creator chain, against
  `vendor/cordis/src/reflect.ts:145-165`.
- The release of the owner records of contained tasks is tested only for completion. Add: the listener's
  fiber disposed with a hundred tasks in flight (the fiber's dispose completes, nothing stays in the
  parent's effect list), a factory that panics, a listener registered on a context that is not the root.

## 5. Small things

- A panic of a listener, of the factory or of the future is reported as the fixed text
  `event listener panicked`, and the payload is lost; the source logs what was thrown. When the payload is
  a `&str` or a `String`, put it after the fixed text (`event listener panicked: <text>`); update R64.
- R70: say that descendants of a scoped context inherit the creator's resolution path on purpose, because
  `locate` in `reflect.ts:145-165` walks parents for every fiber.
- The citation `fiber.ts:270` in the notes points at the `uid` line; cite the lines that say what the rule
  says (the reviewer suggests `:305-323`). Check `fiber.ts:734-759`, which R70 cites: the reviewer did not.

## Not changing (recorded, no action)

- The cost of one async invocation is linear in the number of live effects of the listener's activation
  (`detach` scans the list). With item 1 a listener that finishes at its first poll no longer reserves an
  effect at all; if after that you see a cheap way to make the remaining path constant, take it and say
  so; otherwise leave it.
- An event name chosen at run time (`packages/core/agent/src/dispatch.ts`, `emit(name, payload)`) needs
  one event type per name in Rust: that is the kernel's design and the consumer's work.

## Notes

Update the section "Четвёртый круг: добавленный API" of `crates/hoh-kernel/PORT_NOTES.md` in place (rules
R63–R70, signatures, examples), so that it describes the kernel as it is after this round; the lead moves
it into `docs/kernel.md` afterwards.

## Done when

`cargo test -p hoh-kernel` and `cargo test -p hoh-kernel-macros` pass with every earlier test unchanged
(tests of the fourth round change only where item 1 or 3 changes the rule they assert — list each);
`cargo clippy -p hoh-kernel -p hoh-kernel-macros --all-targets -- -D warnings` and
`cargo fmt -p hoh-kernel -p hoh-kernel-macros -- --check` are clean. For items 1 and 3 write the test first
and see it fail.

Work until it is done. Your last message is the report: the result line of each command, one line per
item, every earlier test you changed and why, anything decided differently and why. At most 300 words.
