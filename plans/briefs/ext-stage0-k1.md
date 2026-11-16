# Kernel, fourth round: what the first consumers need

The kernel is accepted (commit `fb7d160`). The first packages written on it found things the specification
left out. This round adds them. Nothing that exists changes its behaviour: every existing test passes
unchanged. Same boundaries and commands as before: you own only `crates/hoh-kernel` and
`crates/hoh-kernel-macros`; environment
`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-k1`.
You may read `crates/hoh-scope` and `crates/hoh-credentials` (the two consumers that asked); do not edit them.
Other writers build against the kernel while you work: keep the crate compiling between your steps as far as
you can.

## A. Contained dispatch — `events.dispatch('emit', …)` of the source

`docs/kernel.md` section 8 set the source's `dispatch()` aside as deprecated. That was wrong: dsh keeps it
(`vendor/cordis/src/events.ts:176-185`, "DSH retains callback enumeration for per-listener error
containment") and fifteen places of the packages being ported use it
(`grep -rn "events\.dispatch(" packages vendor --include=*.ts` under the source, without tests): agent,
agent-loop, session, tools, llm, skill, subagent, commands, permission-presets, workflow, web, credentials,
authorization, and cordis itself (`vendor/cordis/src/fiber.ts:150-165`). All have one shape:

```ts
for (const callback of ctx.events.dispatch('emit', args)) {
  try {
    const returned = callback(...args)
    void Promise.resolve(returned).catch(error => { ctx.logger.warn(`… listener rejected: ${error}`) })
  } catch (error) {
    ctx.logger.warn(`… listener threw: ${error}`)
  }
}
```

and one variation: the session package takes the snapshot first, changes its own state, and calls the
listeners afterwards (`packages/core/session/src/index.ts:895-907`, `:978-985`). The two warnings have
different texts at most places, so the emitter must learn whether a listener failed while it was called or
later through the promise it returned.

The API, decided by the lead (names may change only if they collide with something):

```rust
pub struct Contained;                         // sixth mode; `Dispatch::Emit` for `on_dispatch` observers

impl Listen<'_> {
    /// A listener that is not `async` in the source. `Err` is what it threw.
    pub fn on_contained<E>(self, f: impl Fn(&E) -> Result<(), Error> + Send + Sync + 'static)
        -> Result<Disposer, KernelError> where E: Event<Mode = Contained>;
    /// An `async` listener of the source: its failure is always a rejection.
    pub fn on_contained_async<E, Fut>(self, f: impl Fn(Arc<E>) -> Fut + Send + Sync + 'static)
        -> Result<Disposer, KernelError>
    where E: Event<Mode = Contained>, Fut: Future<Output = Result<(), Error>> + Send + 'static;
}
// the same two on Context, like the other `on_*`

pub enum ListenerFailure { Threw(Error), Rejected(Error) }

impl Context {
    /// The listeners an emit of this event would call at this moment.
    pub fn dispatch<E: Event<Mode = Contained>>(&self, event: impl Into<Arc<E>>) -> Callbacks<E>;
}
impl<E: Event<Mode = Contained>> Callbacks<E> {
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    /// Calls every listener of the snapshot, in order. The failure of one never stops the others.
    pub fn invoke(self, report: impl Fn(ListenerFailure) + Send + Sync + 'static);
}
```

Rules:
- the snapshot is what `emit` would call: order, `prepend`, `global`, the filter of the dispatching context,
  only listeners that are alive; `on_dispatch` observers are told at `dispatch()`, as `_resolve` tells them;
- what happens to a listener that is removed between `dispatch()` and `invoke()` is what the source does
  with the array it returned: establish it from `events.ts` and say which;
- `invoke` calls the listeners on the caller's task, one after another. A listener's `Err` goes to
  `report(Threw(..))` before the next listener is called. A panic of a listener is contained as the kernel
  contains listeners of its own events (R61) and reported as `Threw`;
- the future of an `async` listener runs as a task of the listener's fiber and is cancelled with it (nothing
  is reported then); its `Err` goes to `report(Rejected(..))`, possibly from another task;
- `emit`, `bail` and the others are not available for a `Contained` event, and `dispatch` is not available
  for the other modes: the type of the event says how it is sent;
- the kernel's own contained events (`fiber.ts:150-165`) may be rebuilt on this or stay as they are.

Tests: the two `dispatch()` cases of `events.spec.ts` that section 8 left out; one failing synchronous
listener, one failing `async` listener and one that succeeds — order of calls and the kind of each report;
a listener added and one removed between `dispatch()` and `invoke()`; the filter is respected; an `async`
listener whose fiber is disposed reports nothing.

## B. What `hoh-scope` needs

Read `crates/hoh-scope/PORT_NOTES.md` (sections "Отличия от образца" and "Вопросы ведущему"), its `src/lib.rs`,
and the source `packages/core/scope/src/index.ts:100-196`. Fourteen packages build on scopes (every agent,
session and tool is created in one), so the port must not carry adaptations that change behaviour. The
writer of that crate could not reach equivalence in five places. For each, find the smallest addition to
the kernel that lets `hoh-scope` do exactly what the source does, implement it, test it in the kernel, and
describe it so that the scope's writer can use it:

1. **`createScope` is synchronous** (`index.ts:136-147`): `ctx.plugin(scope)` returns a fiber whose `ctx` is
   usable at once, before any activation completes; the port had to make creation asynchronous, which
   changes the order of everything created in a scope. A context of a child fiber, obtained synchronously,
   whose effects belong to that child and are undone with it.
2. **`fiber.dispose` as a value** (`rawDispose`, `index.ts:143`): the exact disposer of the child fiber, usable
   as the undo of a composite effect of the parent, and a way to wait for the child's teardown to the end,
   including what the source calls `inertia` (`quiesceFiber`, `index.ts:115-118`).
3. **A value carried by a context and read by a filter** (`fiber.ctx.extend({ [kScope]: key })`,
   `scopeOf(ctx)`, `index.ts:140`, `:150`): the filter of a dispatch reads the tag of the *listener's* context.
4. **Composition with a filter that is already there** (`scopeTarget`, `index.ts:170-183`): the carrier's
   filter first asks the base's filter. `Context::with_filter` exists; say what is missing and add it.
5. **Services as the creator sees them**: the writer reports that a scoped context does not inherit the
   path by which committed dependencies of the fiber that created the scope are resolved, including at
   teardown. Establish from the source what a scope's context sees, and make the kernel give the same.

The writer also asks whether an event must carry a typed key of its receiver (the source's `this` of a
dispatch, the carrier). Answer it in the notes with the source lines; add API only if a listener in the
source reads that receiver (look at how `Scoped<T>` carriers are used by `packages/core/agent/src/dispatch.ts`,
`packages/core/tools/src/pipeline.ts:426-429` and `packages/core/session/src/index.ts:895-899`).

If one of the five turns out not to be a gap (the kernel already allows it), say how instead of adding.

## C. Notes

`crates/hoh-kernel/PORT_NOTES.md` gets a section "Четвёртый круг: добавленный API": every new public item with
its signature, the rule it follows written like the rules of `docs/kernel.md` section 3 (one paragraph each,
with the source `path:line`), and a short example for each of A and B.1–B.5. The lead moves this text into
`docs/kernel.md`; write it so that it can be moved as it is.

## Done when

`cargo test -p hoh-kernel` and `cargo test -p hoh-kernel-macros` pass with every earlier test unchanged;
`cargo clippy -p hoh-kernel -p hoh-kernel-macros --all-targets -- -D warnings` and
`cargo fmt -p hoh-kernel -p hoh-kernel-macros -- --check` are clean.

Work until it is done. Your last message is the report: the result line of each command; the list of new
public items; for B.1–B.5 one line each (added what, or already possible how); anything you decided
differently from this task and why. At most 400 words.
