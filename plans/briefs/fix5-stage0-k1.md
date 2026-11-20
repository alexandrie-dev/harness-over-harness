# Kernel, round 7: what the closure check of round 6 found

The closure check of round 6 (read-only, Sonnet) closed R14, the setup-failure path and the round-5 leftovers.
Fix the following, each with a test in `crates/hoh-kernel/tests/fix5.rs`. Same environment and rules as your
earlier rounds (`CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-k1`, `CARGO_BUILD_JOBS=4`, offline, only
`crates/hoh-kernel` and `crates/hoh-kernel-macros`).

1. **Regression of round 6 in `equal_except_volatile` (`src/diff.rs:56-67`).** The `$ref` branch no longer
   applies the schema's `default` before comparing. Schema `nested: {"$ref":"#/$defs/Nested","default":{}}`,
   `Nested` an object with a volatile `count`; previous `{}`, next `{"nested":{"count":1}}`: the source
   (`diff.ts:17-32` with Schemastery defaults, `schemastery/src/index.ts:912`) says equal, the port says not
   equal. The untracked `crates/hoh-loader/tests/kernel_gaps.rs:18-27` (l1's, read only) reproduces it.
2. **Recursive `$ref`.** `diff.ts:19` descends into a `$ref` once and compares raw only on the second encounter
   on the same path; the port compares raw on the first (`diff.rs:57`). Tree schema with volatile `value` and a
   self-referencing `child`, previous `{"value":"old"}`, next `{"value":"new"}`: source true, port false. Follow
   the source; change `config_diff.rs` (`compares_recursive_schemas_conservatively…`) and the PORT_NOTES §10.1
   decision accordingly, saying why. While there, check `diff.rs:68` (absent `type` counted as object; the source
   needs `type === 'object'`) and `diff.rs:53` (`nullable_choice` unwraps a nullable union, `diff.ts` does not):
   fix each, or record in PORT_NOTES the input showing it cannot reach an observer.
3. **L19, the consumer's scope.** `startup-diagnostics.ts:90` reads `fiber.ctx[Context.isolate][service]` from
   the consumer fiber. In the port a caller holding only a `Fiber` (the loader's entry fibers) cannot get that
   fiber's realm for a service: `Fiber` has no path to its Context or realms, `FiberInfo`/`ProvisionInfo` carry
   none. Add the smallest public way to get it, and a test with an isolated consumer whose realm differs from the
   root's (same-scope and other-scope cases).
4. Tests without code changes unless they fail: `Command::Shutdown` over Pending and Failed fibers (they now go
   through Unloading, `fiber.rs:1020-1033`); a Failed fiber that still owns effects; a synchronous cleanup error
   after an earlier-registered async undo; a non-string panic payload in cleanup; a failing cleanup after a
   panicking setup.

5. **Schemastery's rule for null, at every depth.** `vendor/schemastery/src/index.ts:533-542`: a nullable
   value (`null`/`undefined`) where the schema is not `lazy`: `required` → error `missing required value`;
   otherwise the schema's `default` (descending into the first member of an `intersect` while the default is
   nullish) is cloned in; with no default the value stays as it is. The port's `config/schema.rs:95`
   (`normalized`) only turns a top-level null into `{}`. Reviews found it in three packages: `{"suffix":null}` in
   `hoh-persona` is rejected by the port (`expected string but got null`) while the source defaults it to `""`;
   same for `includeHarnessIdentity: null` in `hoh-system-prompt`. Apply the rule in validation/normalization at
   every depth so packages need nothing of their own; tests for a nested field with a default, without one, and
   a required one.

6. **Stop on a listener's error, for callers that need it.** `hoh-session` (`crates/hoh-session/PORT_NOTES.md`, its
   ignored test with two raw `on_contained::<SessionCreated>` listeners) needs the source's behaviour
   `index.ts:1378-1390`: a synchronous throw from a listener stops the loop. The kernel offers only continue-on-error
   `Callbacks::invoke` (`events.rs:150`) with a private snapshot. Add the smallest public way to invoke listeners
   stopping at the first error (as cordis does for that dispatch kind — establish which from `vendor/cordis`), and
   un-ignore that test (edit only its attribute).

After item 5: remove the `#[ignore = "kernel round 7: schemastery null rule"]` marks in
`crates/hoh-system-prompt/tests/review_fixes.rs` and the loader's ignored `$ref`-default test
(`crates/hoh-loader/tests/kernel_gaps.rs`) — you may edit only those attributes there — and run those crates' tests.

Done when `cargo test`, `clippy --all-targets -D warnings` and `fmt --check` for both crates are clean and
PORT_NOTES records every change. Report: result lines, per item what changed, any new public API. At most 300 words.
