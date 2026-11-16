# Fix round 3 for writer c1: the independent review of the cancellation type and of the lock guard

Same commands as before (`CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-c1`).
Your round 2 is accepted and committed; `AbortError::with_cause` is accepted as an API addition. You own
`crates/hoh-timeout` and `crates/hoh-atomic-write` (and `crates/hoh-error` only if an item needs it). Touch nothing
else. Other writers are working in this repository now.

## `hoh-atomic-write`

1. **Cancel-safety before the guard exists** (the defect of round 2, one step earlier). `LockGuard` is created only
   after `write_new` returns (`lib.rs:393-420`).
   a. If the `with_file_lock` future is dropped while `write_new` awaits `open`, `write_all` or `flush`, the blocking
      work still completes: `<file>.lock` exists with this PID (or empty), no guard exists, the lock is never
      released, and later callers in this process fail with `FILE_LOCK_TIMEOUT`.
   b. If the future is dropped inside `take_over_exited_lock` (`:272-296`) after the claim file is written,
      `.takeover-<hash>` is never removed; every later contender gets `AlreadyExists` and returns `Ok(false)`, so
      that stale record can never be taken over again.
   c. The `.tmp` file of `write_file_atomic` (`:220-227`) is left behind the same way.
   Make each of these owned by a guard that exists before the first await that creates the file, and that cleans up
   in `Drop` when the operation did not finish. Tests with the `Fake` runtime: `write_new` creates the file and then
   never completes, the future is dropped, the lock file is gone and a second caller acquires at once; `remove`
   never completes during takeover, the future is dropped, the claim file is gone; the same for the temporary file.
2. **A guard that silently disarms.** If `File::open` or `metadata` fails in `LockGuard::new` (`:336-343`), `inode` is
   `None` and `Drop` returns early (`:369`): the lock leaks exactly when the system is short of descriptors. The guard
   must still release in that case: fall back to comparing the lock file's record with the record this holder wrote.
   Take the identity from the handle that created the lock file instead of opening it a second time, if the runtime
   abstraction allows it. Test: a `Fake` whose identity read fails, the future is dropped, the lock is released.
3. **Abort versus timer in the wait.** The `select!` in `wait()` (`:311`) is unbiased, so when the signal and the timer
   are both ready it may take the timer; Node rejects with the abort. Make the abort branch win. Test under paused
   time.
4. **Tests that can fail elsewhere or prove too little** (`tests/atomic-write.rs:893-927`). `getppid().unwrap()` panics
   when the test runs as PID 1 or the parent is in another PID namespace: skip with a printed reason. The `transient`
   test has two negative codes (`EIO`, `ENOENT`): add `EAGAIN`, `EEXIST`, `ENOTEMPTY`, so that an implementation
   accepting more than the source's three codes fails.
5. **Notes.** `PORT_NOTES.md`: state which files are released when a future is dropped and at which points; state that
   the guard's constructor and destructor do blocking file calls on the async thread and why that is accepted; keep
   the Windows statement.

## `hoh-timeout`

6. **Timer and controller at the same instant.** `abort.rs:157-159` and `:170` compare with a strict `<`. Scenario
   under paused time: `t = AbortSignal::timeout(10)`, `r = AbortSignal::any([controller.signal(), t])`, advance 10 ms
   with nobody observing, then `controller.abort()`. In JavaScript the timer fired at 10 ms, before the later
   `abort()`, so the reason of `r` is the timeout. The port gives the controller's reason. A timer that is due at the
   moment of a later abort must win. Add this test (the existing one advances 20 ms and misses the tie).
7. **A waker called under a lock.** In the `idle_watchdog` timer task (`lib.rs:242-245`) `abort_with` runs while the
   `WatchdogState` guard is alive, and it wakes waiters. Release the guard first. Check the rest of the crate for the
   same pattern.
8. **`Debug` with a side effect.** Formatting an `AbortSignal` (`abort.rs:102`) refreshes it and can abort a due timer.
   `Debug` must only read.
9. **Missing tests.** A chain of `any` of `any`: `any([any([timeout(10)]), controller.signal()])`, the controller
   aborted after 20 ms, the reason is `ptr_eq` to the timer's reason; the same with the controller first. A result
   that outlives its sources and still reports the reason. Aborts from several threads at once: exactly one reason
   wins and every observer sees the same one. `AbortSignal::timeout` and `from_token` created and read with no tokio
   runtime: decide the behaviour (it must not panic in `aborted()`, `reason()` and `throw_if_aborted()`), test it and
   document it.
10. **Notes.** `PORT_NOTES.md`: a result of `any` holds its sources strongly (sources hold results weakly); the cost of
    `aborted()`, `reason()` and `throw_if_aborted()` (what is locked and read per call, and how it grows with the
    depth and width of an `any` graph), since the source calls them hundreds of times.

## Done when

`cargo test`, `cargo clippy --all-targets -- -D warnings` and `cargo fmt -- --check` are clean for `hoh-timeout`,
`hoh-atomic-write` and `hoh-error`. Final report: result lines and one line per item saying what changed; anything
unfinished; any item you consider wrong, with the reason.
