# Port notes: `hoh-subagent-fork-in-process`

Source: `packages/subagent/subagent-fork-in-process` at DeepSeek Harness reference
revision `39163dd3`.

## Mapping

- `src/index.ts` maps to `src/lib.rs`: the `fork` provider, configuration,
  capabilities, parent-context inheritance, completed-turn snapshot, and plugin
  lifetime registration are preserved.
- The eight locale JSON files are copied without content changes.
- All 13 source test declarations are represented by 19 Rust tests in
  `tests/fork.rs` and `tests/runtime.rs`. The latter drives the production
  `AgentLoop` + `SubagentRuntime` with a scripted model rather than calling the
  provider seam directly.
- Package metadata and the English/Chinese READMEs are represented by
  `Cargo.toml`, crate-level documentation, and this file.

## Intentional Rust adaptations

- The TypeScript provider object is a `SubagentProvider` implementation and the
  loader extension is a typed kernel `Plugin`.
- `Promise.resolve(snapshot)` becomes a boxed future. The session snapshot is
  deliberately taken before constructing that future, so events appended before
  the caller polls it cannot leak into the child seed.
- The registry's `ProviderRegistration` is owned by a kernel effect; disposing
  the plugin drops it and unregisters the provider, preserving the source HMR
  cleanup behavior.
- An empty completed-turn prefix is represented as `seed: None`, matching an
  omitted TypeScript `seed` property.

## Shared-runtime boundary

Child creation, structured-output capture, start-event publication, and scanning
only the child's post-seed events are implemented by `hoh-subagent`, as they are
by the shared subagent runtime in the source project. This crate advertises the
same capabilities and supplies the exact balanced seed at that existing seam; it
does not duplicate the runtime. The integration suite nevertheless exercises that
boundary end to end and pins all source-observable outcomes:

- `subagent/start` sees the exact already-published seeded child;
- a parent without a completed turn creates a fresh child with `isSeeded=false`,
  inherited count zero, and exactly one child-owned `turn/end`;
- a seeded child's `isSeeded`, `inheritedEventCount`, `parentSession`, balanced
  prefix, and child-owned output are checked on the real session;
- an open parent turn is excluded while the child's resulting log still contains
  one inherited and one child-owned completed turn;
- structured output `{answer: 9}` is captured and the scoped capture tool is
  removed after settlement;
- an empty child `stop` returns `output=[]` rather than borrowing the seeded
  parent's last assistant message;
- one parent completes a fresh spawn child, then a seeded fork child, and then a
  second parent turn, with distinct lineage-correct child sessions.

The registration test deliberately mounts only `Subagents`, without an
`AgentRegistry`, and proves the provider publishes and unloads at that minimal
dependency boundary. Re-registering after dispose is owned by the common typed
provider registry and is not a distinct package behavior.

## Not applicable

The package has no network, process, platform, persistence, or environment-
variable behavior. No compatibility shim or new external dependency is needed.
The TypeScript assertion that the module has no stray JavaScript `default` export
has no Rust analogue; the Rust replacement checks the plugin type, `NAME`,
`INJECT`, and default configuration.

## Verification

- `timeout 300s cargo test -p hoh-subagent-fork-in-process --offline`: 19 passed,
  0 failed (unit 0, integration 19, doc-tests 0).
- `timeout 240s cargo clippy -p hoh-subagent-fork-in-process --all-targets
  --no-deps --offline -- -D warnings`.
- `timeout 120s cargo fmt -p hoh-subagent-fork-in-process -- --check`.

All commands use `CARGO_BUILD_JOBS=2`, the assigned `CARGO_TARGET_DIR`, offline
mode, and an external timeout.
