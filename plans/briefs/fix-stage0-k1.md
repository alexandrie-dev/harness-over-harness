# Fix round for writer k1: `hoh-kernel` and `hoh-kernel-macros` after independent review

Same commands and boundaries as your task `plans/briefs/stage0-k1-kernel.md`
(`CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-k1`; you own only
`crates/hoh-kernel` and `crates/hoh-kernel-macros`). Your delivery is accepted as a base: the lead verified fmt,
clippy with `-D warnings` and 166 tests. Reviewers then read it against the specification and the source; the
lead checked the central findings against the code and merged everything below.

Two boundaries change. The ban on depending on other crates of the workspace was the lead's mistake: the
kernel may now depend on `hoh-util-values` (JavaScript number and string forms, `js_json_stringify`), and on the
workspace dependency `jiff`. Read `docs/porting/PORTING.md` section 4 again: three bullets were added (JSON
text, recursion, exact integers), and section 6 now allows one kind of process-wide state, an id counter.

**How to work through the findings.** For each one, write the test first and see it fail on the present code;
then fix. If you cannot make a test fail because the finding is wrong, do not change the code: report the
finding as disputed, with the reason. Do not weaken an existing test to make room for a change.

## A. Decisions on your questions and deviations

A1. **`Realm::new()`**: allocate from a process-wide `AtomicU64` counter (strictly unique, `ROOT` excluded);
    remove the `RandomState` code. This is the exception PORTING section 6 now names.
A2. **`dispose()` keeps `Output = ()`**; a self-wait is logged and the call returns. Accepted. Extend the
    self-wait guard beyond the `apply` task (finding C5).
A3. **`PluginDef::of()` stays infallible**; an unsupported schema keyword is reported at the first validation.
    Accepted.
A4. **Source tests over the specification** for an ordinary `emit` that lets a listener's panic unwind (your
    deviation 4), concurrent update handlers of a retained instance (5) and a provider reading its own record
    while loading (6): accepted; the lead amends R40, R42 and R26 in `docs/kernel.md`. Keep the kernel's own
    events isolated as they are.
A5. **Added items**: `Listen::on_update`, public `ConsoleConfig`, `Volatile: Default` and `Fiber::retain_raw_config`
    are accepted. `View` must not be constructible outside the kernel (finding D2).
A6. **Aggregate errors** are flattened at the typed boundary, as you did. Accepted.
A7. **Console time** is printed in the local time zone, as the source does (`vendor/logger-console/src/shared.ts:79`
    formats with `Time.template`, which reads the local clock): use `jiff`. The formatting function takes the
    zone as a parameter so that tests pass a fixed zone and never depend on the machine's setting.

## B. Configuration checker (`src/config.rs`)

B1. **bug — ordinary `schemars` output is rejected.** The closed keyword list (`config.rs:274-305`) lacks
    keywords that `schemars` 1.2.3 emits: `uniqueItems` (`HashSet`, `BTreeSet`), `patternProperties` (maps with
    integer keys; such a map also carries `additionalProperties: false`, so its valid keys are rejected too),
    `not` (`NonZero*`), `readOnly`/`writeOnly`, and others. A recursive root type produces `"$ref": "#"`, which
    `inspect_schema` (`config.rs:333-352`) reports as unsupported although `walk` handles it. Every activation of
    such a plugin fails. Establish the complete set from the source of `schemars` 1.2.3 in
    `~/.cargo/registry/src` (every keyword its `JsonSchema` implementations and its derive can emit under the
    settings this project uses), implement each keyword's check, and list the set in `PORT_NOTES.md` with the
    `schemars` file each came from. Tests must use derived types, not hand-written JSON: a `Config` with
    `BTreeSet<String>`, `HashMap<u32, String>`, `NonZeroU32`, `Option<T>`, an enum of each serde representation
    (external, internal, adjacent, untagged), a flattened struct, `deny_unknown_fields`, a recursive struct
    (`children: Vec<Self>`), a tuple and a newtype; each loads with a valid value and is rejected with a pathed
    message for an invalid one.
B2. **bug — comparison that ignores volatile fields stops at `$ref`.** `compare` returns "different" for any
    `$ref` once the values differ (`config.rs:673-675`) and `volatile_paths` never follows references
    (`:713-773`), while `inspect_schema` does. With `struct Inner { v: Volatile<u32>, n: u32 }` and
    `struct Cfg { inner: Inner }` a change of `inner.v` alone restarts the plugin instead of committing the
    volatile value (R53, R54 exempt only recursive references). The test hides it with `#[schemars(inline)]`
    (`tests/volatile_config.rs:24`). Follow non-recursive references in both functions; add the `Inner`/`Cfg`
    case without `inline`: `equal_except_volatile` is true and `volatile_paths` is `["inner.v"]`.
B3. **rule violations in texts and numbers** (one test each):
    - `config.rs:129` `js_number`: replace with `hoh_util_values::js_number_string`; integers beyond 2^53 must
      print as JavaScript prints the nearest double.
    - `config.rs:481` with `fiber.rs:59`: type `integer` accepts `2.0` and `1e30`, after which typed
      deserialization fails with a serde text that has no path. An integral number that fits the target type
      must load; one that does not fit is a validation issue with its path, in the wording of R49.
    - numeric bounds (`minimum`, `maximum`, `multipleOf`) are compared through `f64`: integers beyond 2^53 must
      compare exactly.
    - `config.rs:419-421`: the message for `additionalProperties: false` ("expected nullable but got …") is not
      one of R49's; use what the source prints for an unknown key
      (`vendor/schemastery/src/index.ts`).
    - `config.rs:493` and `:197`: when `type` is an array (`["string", "null"]`) the message prints raw JSON and
      `description()` yields "unknown". Print the forms R49 gives.
    - `config.rs:632-640`: a `pattern` the `regex` crate cannot compile (look-ahead, back-reference) rejects
      every string. That is an error of the schema, not of the value: report it as such, naming the pattern.
    - `config.rs:170-177` `json_text`: use `hoh_util_values::js_json_stringify` (key order, number forms).
    - `config.rs:406`: the depth limit of 128 counts reference hops, so valid values nested about 64 levels
      under a recursive schema fail. Count the nesting of the value; the limit is that of
      `hoh_util_values::JS_JSON_MAX_DEPTH`.
    - `config.rs:505`, `:598`: `const` and `enum` reject `1.0` against `1`, and the volatile comparison
      treats them as different. In JavaScript they are one number: compare numbers by value everywhere.
B4. **bug — unreferenced definitions are never inspected** (`config.rs:306`): an unsupported keyword inside a
    member of `$defs` that nothing references passes. Inspect every definition.
B5. **bug — a nullable object is turned into `{}`** (`config.rs:245`). For a root schema with `properties` and
    `type: ["object", "null"]` (what `schemars` gives for an optional inline struct) a valid `null` is replaced
    by an empty object and then fails on a missing field. `null` must stay `null`. Test: `Option<InlineStruct>`
    with a required field and the input `null`.
B6. **bug — unknown fields are lost below the top level** (`fiber.rs:65`). For `{"nested":{"known":1,"extra":2}}`
    the merge replaces the whole nested object by its typed serialization, dropping `extra`; the source keeps
    unknown fields at every depth (R50, R51). Merge recursively. Test raw against effective configuration at
    two levels of nesting and inside an array of structs.

## C. Lifecycle (`src/fiber.rs`, `src/effects.rs`, `src/context.rs`)

C1. **hazard — records that are never released on a long-lived fiber.** `Activation.effects` only grows
    (`effects.rs:242`, `:294`): an effect disposed by hand stays in the list until the fiber unloads, and
    `Fiber::dispose()` of a child does not release the parent's record of `ctx.plugin()` (`fiber.rs:487-491`),
    whose undo keeps the child's data alive. At the root that is the lifetime of the kernel, and the loader
    creates and removes rows there. Test: 10,000 times `root.plugin(..)` then `dispose()`, holding a
    `Weak` to something the child owns; it must be released, and the list must not grow.
C2. **bug — cleanups that are lost.** (a) `Undos` is `Clone + 'static`: a `push` after the effect began to
    dispose or finished stores the undo and never runs it (`effects.rs:216-223`). Such an undo must run at
    once (the source disposes what an aborted asynchronous effect yields late), and the caller still learns
    that the effect was inactive. (b) `Undo::from(Disposer)` marks the disposer as adopted, so the fiber's
    unload skips it (`effects.rs:72-80`, `:200-209`, `:273`); if that `Undo` is then dropped without being
    pushed, the listener or provision is never removed. A listener or provision must never outlive its
    fiber: adoption takes effect only when the undo is actually handed to an effect. Tests: push through a
    cloned `Undos` after the effect was disposed and assert the undo ran; `drop(Undo::from(ctx.on::<Ping>(..)?))`,
    restart the fiber, emit, and assert the listener is gone.
C3. **hazard — raw configuration after a rejected update.** In `update` the raw configuration is replaced and
    the error cleared before the handler runs (`fiber.rs:736-744`, `:1184-1188`); if the handler rejects, raw
    stays at the rejected value while the effective configuration keeps the old one, and a later `restart`
    loads the rejected value. Establish from `vendor/cordis/src` what the source leaves in `rawConfig` in this
    case and do the same; report the `path:line`. Test: update A with a slow accepting handler, then update B
    with a fast rejecting handler; compare raw and effective configuration, then restart.
C4. **hazard — a fiber that has never loaded has an open activation.** `Create` builds the context with a
    fresh, unclosed `Activation` (`fiber.rs:632-635`), which is replaced at `:995` without unloading; R17 says
    an idle fiber has no live activation. A `ConfigResolver` called through `Fiber::resolve_config` on a
    pending fiber can register a listener on that context, and it then lives for ever. Test: such a
    resolver registers a listener; after load and unload the listener is gone (or the registration was
    refused as inactive).
C5. **hazard — waiting on oneself outside `apply`.** The ancestry mark is set only in the `apply` task
    (`fiber.rs:257-262`, `:307-314`, `:1116-1118`). A cleanup future that awaits its own fiber's `dispose()`
    never finishes, and the same holds for update handlers and `ctx.spawn` tasks. Set the mark in every task
    the kernel runs on behalf of a fiber. Test: a cleanup that awaits `ctx.fiber().dispose()` completes.
    Event dispatch loses the mark as well (`events.rs:495`): `serial`, `waterfall` and `parallel` run listeners
    in tasks of their own, so when `apply` awaits `ctx.serial(..)` and a listener awaits that fiber's
    `settled()`, both wait for ever. A task that dispatch creates inherits the mark of the task that called
    the dispatch. Test that indirect wait: it returns `SelfWait`.
C6. **hazard — a panic in a caller's closure that the actor runs.** The filter of a dispatch listener
    (`on_dispatch`) is called inside the actor outside any isolation; if it panics, the actor fails and the
    kernel closes (R62). Every closure of plugin code that the actor itself calls (filters, resolvers,
    exporters, hooks) must be isolated the way listeners of kernel events are. Test: a panicking filter does
    not close the kernel. And when the actor does fail, cancel the token of every live activation so that
    plugin tasks stop instead of running on in a closed kernel; extend the actor-failure test to assert it.
    A suspended update handler owns its reply sender outside the node's waiters (`fiber.rs:1205`, `:1322`), so
    after an actor failure the caller of that `update` waits until the handler ends, possibly for ever: every
    waiter must receive `Closed` (R62). Test: suspend a retained update, inject the fault, and require `Closed`
    before the handler is released.
C7. **note — `Context::interval` with a zero period** returns an empty stream (`context.rs:334`). The source's
    timer passes the delay to `setInterval`, where Node treats 0 as 1 ms. Do the same and test it with paused
    time. The stream also replays ticks nobody asked for (`context.rs:335`; tokio's interval bursts by
    default): after an idle period the next reads return the overdue ticks at once, while the source's timer
    (`vendor/timer/src/index.ts`) discards a tick when no read is pending, so a read after an idle period
    waits for the next tick. Follow the source. Test: create, advance several periods without reading, then
    read twice and assert the times.
C8. **note — smaller.** `Disposer::dispose` outside a tokio runtime panics in `tokio::spawn`
    (`effects.rs:165`): it must not panic (run what is synchronous, report the rest). The table of service
    types (`fiber.rs:812`) never shrinks, so a name keeps its first `TypeId` for the life of the kernel even
    after every provider and consumer of that name is gone: release the entry then, and test providing the
    same name with another type afterwards.
C9. **bug — a context kept by a caller keeps released services alive.** Unloading replaces the actor's
    context, but an old `Context` held elsewhere still owns its activation's `View` with the service values
    (`fiber.rs:673`, `effects.rs:237`): the dependency is logically released (R10) and physically retained, and
    the R35 warning then misleads. When an activation closes, its view must let go of the values. Test: keep
    only a consumer's context, dispose consumer and provider, check a `Weak` to the service value.
C10. **hazard — a superseded update still restarts newer work.** Update A suspends in its handler; update B
    completes in place; A then calls `next`, whose tail calls `restart()` unconditionally (`fiber.rs:1200`) and
    remounts the fiber although A is outdated. Establish from `vendor/cordis/src` and the harness test
    "publishes successful retained-instance updates and ignores stale or rejected completions" what an
    outdated delegation does in the source, and do the same; report the `path:line`. Test: record mounts across
    exactly this order of events.
C11. **rule violation — notifications about services cover only the visible table.** A provider that is still
    loading can register and withdraw a binding without any `InternalService` event (`fiber.rs:883`); R28
    requires a notification when a record disappears, and the source's `reflect.ts` notifies on withdrawal
    before activation too. Test: a provider held in `Loading` withdraws its own binding; the event arrives.
C12. **bug — `Listen::on_update` with prepend is not atomic** (`events.rs:117`): the handler is appended under
    one lock and moved under a second, so a concurrent update can take a snapshot of the intermediate order
    (R41). One lock acquisition.
C13. **bug — aggregate errors are flattened one level only** (`logger.rs:96`): a member that is itself an
    aggregate is logged as one record with the aggregate's text; the source flattens recursively (R57).
    Test an aggregate nested in an aggregate with two leaves.
C14. **note — nested continuations.** A `waterfall` or forwarding chain polls each listener's continuation
    inside the previous one (`events.rs:288`, `fiber.rs:1292`), so the stack grows with the number of listeners
    of one event. That number is bounded by what plugins register, not by input, so the nesting is allowed;
    state the bound in the notes and add a test with 2,000 forwarding listeners of one event on a thread with
    the default 2 MiB stack. If it overflows, run each continuation as a task of its own.

## D. Macros (`crates/hoh-kernel-macros/src/lib.rs`)

D1. **bug — generated code is not hygienic.** The expansion writes `Result<(), ::hoh_kernel::Error>` (`:159`) and
    `vec!` (`:57-58`) unqualified. With `type Result<T> = std::result::Result<T, hoh_kernel::Error>;` in scope —
    the usual alias in a crate — `#[plugin]` fails to compile. Qualify every path the macros emit
    (`::core::result::Result`, `::std::vec::Vec`, …). Add a trybuild pass case with that alias and with a local
    macro named `vec`. The generated parameters `ctx`, `deps` and `config` also shadow a plugin function that
    has one of those names: give generated identifiers names a caller cannot collide with, and add a pass
    case for a plugin function called `config`.

D2. **`View`**: `View::default().service::<K>()` and `Inject::resolve(&View::default())` panic through the
    `expect` at `hoh-kernel/src/lib.rs:105-118`, and `View::default` is public. Make an empty `View`
    impossible to construct outside the kernel, so that the `expect` rests on a real invariant.

## E. Tests

E1. **No test runs on a multi-threaded runtime**, although the model relies on locks and on commands sent
    from callers' threads. Add tests with `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]`, each
    bounded in iterations and guarded by a timeout: creating a child while its parent is disposed from
    another task (`fiber.rs:487-495`); `Effect::begin` against `finish_setup` (`effects.rs:127-189`);
    `reserve_effect` against `close`; concurrent `update` calls from several tasks. Each asserts that nothing
    hangs and that no fiber or listener is left behind.
E2. **Weakened or missing assertions**, source line → port line. Restore each:
    - `fiber.spec.ts:89-90` (the listener of a failed fiber is removed, its callback called once) —
      `tests/fiber.rs:75-93` registers no listener;
    - `fiber.spec.ts:469-472` (the second load sees `extra: 1`) — `fiber.rs:365-366` checks only the call log;
    - `fiber.spec.ts:444` — `fiber.rs:348` checks only `["resolved"]`;
    - the inertia cases assert only through `settled()` and the load log: assert the `Active` state and the
      provided value as the source does;
    - `update.spec.ts:42` (no update has settled early) — missing in `tests/update.rs:31-40`;
    - `update.spec.ts:116-117` (state `Active`, identity of the configuration) — missing at `update.rs:95`;
    - `update.spec.ts:146` (the superseded failure is logged once) — missing at `update.rs:110`;
    - `update.spec.ts:179` (the message `latest load failed`) — `update.rs:126` compares the two messages with
      each other, not with the expected text;
    - `service-dispose.spec.ts`, the case of unrelated providers and sibling effects cleaning up concurrently —
      the port moves the sibling off the blocked fiber, so it no longer shows what the source shows;
    - `events.spec.ts`, empty buckets removed after disposal — `tests/events.rs:19-31` passes with or without
      the removal because the bus is private: assert it through something observable, or move the case to a
      unit test beside the bus;
    - `cordis-timer.spec.ts` — `tests/timer.rs` never advances time, never checks the period, asserts nothing
      for operation 0 and replaces the source's three concurrent reads by one reader; the case the notes
      claim (`PORT_NOTES.md:128`, pending reads settled on return, throw and dispose) creates no pending read
      and never throws, and its second branch tests a restart instead. Port it as written; if one reader is
      all the Rust stream allows, say so in the notes and still assert the period and each operation;
    - `update.spec.ts`, the superseded generator (`tests/update.rs:137`): nothing observable follows the `push`,
      so the test passes if the `Inactive` behaviour is removed. Add a marker after the push and assert it;
    - `service-dispose.spec.ts`, composed provider effects (`tests/service_dispose.rs:342`): the port shuts the
      kernel down where the source disposes the owner's subtree. Dispose the subtree and keep the source's
      assertions made before the release.

## F. Notes

Update `PORT_NOTES.md` of both crates: the decisions of part A, the keyword set of B1, every limit, what
changed in the lists of ported cases, and the statement that a kernel lives until `shutdown()` — dropping
every handle does not stop its actor (the lead records the same in the specification).

## Done when

`cargo test`, `cargo clippy --all-targets -- -D warnings` and `cargo fmt -- --check` are clean for both crates,
with the new tests. Final report: the result lines; one line per item (A1…F) saying what changed, with the
source `path:line` you relied on for C3 and C10; every finding you dispute, with the reason; anything
unfinished. At most 600 words.
