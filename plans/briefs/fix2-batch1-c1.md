# Fix round 2 for writer c1: cancellation with a reason; the review of `hoh-atomic-write`

Same commands as before (`CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-c1`).
Your previous round is accepted. In this round you own exactly three crates: `crates/hoh-error`,
`crates/hoh-timeout` (written by another writer: read its code, tests and `PORT_NOTES.md` first) and
`crates/hoh-atomic-write`. Touch nothing else. Other writers are working in this repository now.

Read `docs/porting/PORTING.md` section 6 (its first bullet about `AbortSignal` is new) and the paragraph
"Отмена с причиной" in section 10 of `docs/kernel.md`.

## Why

The source hands an `AbortSignal` through about 90 packages and uses its reason. In the packages being ported
`signal.reason` is read 93 times, `signal.throwIfAborted()` (which throws the reason) is called 426 times,
`controller.abort(reason)` 50 times, `new AbortController()` 91 times, `AbortSignal.any` 54 times,
`AbortSignal.timeout` 19 times. The reason is compared by identity (`error === signal.reason`,
`core/tools/src/pipeline.ts:720`), tested by class (`signal.reason instanceof ToolCallLimitExceededError`),
forwarded (`controller.abort(signal.reason)`) and thrown (`reject(signal.reason)`). A bare cancellation flag cannot
carry that. So all ported code uses one cancellation type. It lives in `hoh-timeout` and replaces the provisional
`AbortSignal` there. The kernel's `ctx.token()` stays a `tokio_util::sync::CancellationToken` (the end of an
activation has no reason); `AbortSignal::from_token` is the bridge.

## 1. `hoh-error`: add `SharedError`

The Rust form of a JavaScript value that is thrown, stored, forwarded and compared by identity.

```rust
#[derive(Clone, Debug)]
pub struct SharedError(/* Arc<dyn Error + Send + Sync + 'static> */);

impl SharedError {
    /// Wraps an error. Given a `SharedError` (also one inside a `BoxError`) returns that same value: no nesting,
    /// identity survives a round trip through `BoxError`.
    pub fn new(error: impl Into<BoxError>) -> Self;
    /// `a === b`.
    pub fn ptr_eq(&self, other: &SharedError) -> bool;
    /// `x instanceof T`.
    pub fn downcast_ref<T: std::error::Error + 'static>(&self) -> Option<&T>;
    pub fn get_ref(&self) -> &(dyn std::error::Error + Send + Sync + 'static);
    /// Recognises a thrown reason: a downcast of `error` itself, never of its sources.
    pub fn of<'a>(error: &'a (dyn std::error::Error + 'static)) -> Option<&'a SharedError>;
}
impl std::fmt::Display for SharedError   // the wrapped error's text
impl std::error::Error for SharedError   // `source()` is the wrapped error's `source()`: the wrapper is transparent
```

`HarnessError::of` must also look through a `SharedError`: a `HarnessError` used as an abort reason and then thrown
is still recognised (`error instanceof HarnessError`). Tests: identity through `clone`, through `BoxError` and back,
through `SharedError::new(shared)`; `downcast_ref` hit and miss; `of` at depth 0 and not through `source()`;
`HarnessError::of` through a `SharedError`, directly and inside a `BoxError`; `Display` and `source()` transparent.

## 2. `hoh-timeout`: the cancellation type

Add the dependencies `hoh-error = { path = "../hoh-error" }` and `tokio-util.workspace = true` (already fetched).
Replace the provisional `AbortSignal`. Public API:

```rust
/// A signal aborted without a named reason carries this (JavaScript: a DOMException named "AbortError").
/// Also the port of source errors built as `Object.assign(new Error(text), { name: 'AbortError' })`.
#[derive(Debug, Clone)] pub struct AbortError { /* message */ }
impl AbortError { pub fn new(message: impl Into<String>) -> Self; }   // `Default`: "This operation was aborted"
/// `error.name === 'AbortError'`: an `AbortError` itself or inside a `SharedError`; never through `source()`.
pub fn is_abort_error(error: &(dyn std::error::Error + 'static)) -> bool;
/// The reason of `AbortSignal::timeout` (JavaScript: a DOMException named "TimeoutError").
#[derive(Debug, Clone)] pub struct DomTimeoutError;                   // "The operation was aborted due to timeout"

#[derive(Clone)] pub struct AbortSignal { /* .. */ }                  // Send + Sync, Debug
impl AbortSignal {
    pub fn aborted(&self) -> bool;
    pub fn reason(&self) -> Option<SharedError>;
    pub fn throw_if_aborted(&self) -> Result<(), SharedError>;       // the error is the reason itself
    /// Resolves with the reason once aborted; at once if already aborted. Replaces `addEventListener('abort')`.
    pub async fn cancelled(&self) -> SharedError;
    pub fn any(signals: impl IntoIterator<Item = AbortSignal>) -> AbortSignal;
    pub fn timeout(timeout_ms: f64) -> AbortSignal;
    pub fn from_token(token: tokio_util::sync::CancellationToken) -> AbortSignal;
}

pub struct AbortController { /* .. */ }                               // `Default`, Send + Sync
impl AbortController {
    pub fn new() -> Self;
    pub fn signal(&self) -> AbortSignal;
    pub fn abort(&self);                                              // reason: `AbortError::default()`
    pub fn abort_with(&self, reason: impl Into<BoxError>);            // a `SharedError` keeps its identity
}
```

Behaviour, each line with a test (paused tokio time where time is involved):

- A new controller's signal is not aborted: `reason()` is `None`, `throw_if_aborted()` is `Ok`.
- The first abort wins; later ones change nothing. `abort()` gives an `AbortError` with the text above.
- `reason()` returns the same value on every call (`ptr_eq`), `throw_if_aborted()` and `cancelled()` return that
  same value, and it is still the same after `?` into a `BoxError` and `SharedError::new` of that box.
- Forwarding keeps identity: `b.abort_with(a.signal().reason().unwrap())`, then the two reasons are `ptr_eq`.
- `cancelled()`: resolves for an already aborted signal; resolves for every waiter when aborted later.
- `any`: a source aborted at construction aborts the result at once with that reason; otherwise the result aborts
  with the reason of the source that aborts first in time (identity kept); an empty list never aborts; a source
  does not keep dropped results alive.
- `timeout`: not aborted before the delay, aborted after it with `DomTimeoutError`; the delay is truncated and
  floored at 1 ms like the crate's other timers.
- `from_token`: not aborted before the token is cancelled; after cancellation `aborted()` is true even if nobody
  was waiting, the reason is an `AbortError` with a stable identity, `cancelled()` wakes, and it works as a
  source of `any` together with a controller's signal.
- `timeout` and `from_token` must not leave a task or a timer alive once every clone of the signal is dropped and
  must not accumulate anything per call on a long-lived token. Say in `PORT_NOTES.md` how this is ensured.

The rest of the crate keeps its behaviour and its tests, moved to the new types: `deadline` (its reason stays a
`TimeoutReason`, now inside a `SharedError`), `idle_watchdog`, `clamp_timeout`. `timeout_of` takes what the source
takes: write it so that both a signal and a caught error can be classified (source:
`timeoutOf(x: AbortSignal | { reason?: unknown }, code?)`), for example `timeout_of(reason: Option<&SharedError>, ..)`
plus a documented way to pass a signal. Remove `AbortSignal::new`, `AbortSignal::abort` and the `Any`-based reason.
Reviewer's notes to settle in `PORT_NOTES.md`: `Deadline` cancels its timer on `Drop`, while in the source the timer
runs until `dispose()` (keep the Rust behaviour, state it).

## 3. `hoh-atomic-write`

a. `FileLockOptions.signal` becomes `Option<hoh_timeout::AbortSignal>`. At each of the three places where the source
   reacts to the signal (`index.ts:268`, `:284`, `:288`) return what the source throws there: `throwIfAborted()`
   throws the signal's reason itself (test: the returned error is `ptr_eq` to `signal.reason()`); the abortable wait
   of `node:timers/promises` rejects with Node's `AbortError` (`The operation was aborted`, `cause` = the reason).

b. Review finding (resource release): dropping the future of `with_file_lock` after the lock is taken leaves
   `<file>.lock` holding this process's PID; takeover refuses the own PID, so later callers in this process wait and
   fail with `FILE_LOCK_TIMEOUT`. A JavaScript promise cannot be dropped; a Rust future can (`select!`, a timeout, a
   task aborted by the kernel). Add a guard whose `Drop` removes the lock file synchronously if it still holds this
   holder's record. Test: `tokio::time::timeout(small, with_file_lock(.., a body that never finishes))`, then a
   second `with_file_lock` on the same file succeeds at once and the lock file is gone afterwards.

c. Review finding (missing tests): the real `rustix` probe is tested only for `ESRCH`. Add: pid 1 probed by a
   non-root test is not "proven gone" (skip with a printed reason when the test runs as root); the test's own parent
   process is alive. Rename retries are tested with injected `io::ErrorKind` values, so the mapping of real error
   numbers in `transient()` is untested: test it with `io::Error::from_raw_os_error` for the codes the source names
   (`EACCES`, `EBUSY`, `EPERM`) and for a code that is not transient.

d. Review note: `transient()` is wider than the source's three codes on Windows (any `PermissionDenied` or
   `ResourceBusy`). Make the Unix branch exactly the source's three codes; for Windows state in `PORT_NOTES.md`
   what is mapped and that it has not been run there.

## Done when

For the three crates: `cargo test -p <crate>`, `cargo clippy -p <crate> --all-targets -- -D warnings` and
`cargo fmt -p <crate> -- --check` are clean. Then run `cargo test` for `hoh-output-retention` and `hoh-fs-patch`
too and report the result lines: they must still pass. `PORT_NOTES.md` of the three crates is updated.

Final report: per crate the test result line and one line per item above saying what changed; any deviation from
the API given here with its reason; anything unfinished.
