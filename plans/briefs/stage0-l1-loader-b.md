# Writer l1, part B: `hoh-loader` on the kernel

You wrote part A of `crates/hoh-loader` (YAML, patches and layers, expressions, profile) and fixed it after
review; it is accepted. The kernel now exists: `crates/hoh-kernel` and `crates/hoh-kernel-macros` are written,
reviewed and fixed. Part B mounts the loader on it. Working directory, toolchain, commands and boundaries are
those of part A (`plans/briefs/stage0-l1-loader-a.md`): you own only `crates/hoh-loader`, environment
`CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-l1`. The manifest now lists
`hoh-kernel` as a dependency.

## Read first

- `docs/kernel.md`: section 2.9 (the loader's public API — everything there that part A left out is yours now),
  section 5 (the loader: format, row, group, inclusion, recomposition, dump and start-up check; rules `L1`…),
  section 3.8 (the local changes of dsh and their cases `V1`…), sections 8.2–8.4 (tests), section 10 and 10.1
  (decisions; 10.1 is newer than the rule texts and wins where they differ). Sections 1–2 and 6 for the kernel
  itself.
- `docs/porting/PORTING.md`, in full again: section 4 and section 6 changed since part A.
- `crates/hoh-kernel/src/lib.rs` (the public API as built) and `crates/hoh-kernel/PORT_NOTES.md`; your own
  `PORT_NOTES.md`, section "Часть B".
- The source, read-only, under `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness`: `vendor/loader/src`,
  `vendor/include/src`, `vendor/group/src`, `packages/boot/app-boot/src` (`index.ts`, `startup-diagnostics.ts`,
  `profile-sanitize.ts`, `config-schema/`), `packages/boot/config-editor/src`, with their tests and the
  harness's kernel tests in `scripts/` named below.

## What to build

1. **`PluginRegistry`** (`add`, `alias`, `carrier`, `resource_dir`, `resolve`) as section 2.9 gives it.
2. **Rows as fibers.** A row of the composed tree is mounted as a fiber of the plugin its `name` resolves to:
   `disabled` (value or expression), `inject`, `isolate`, `intercept`, raw configuration with expressions
   resolved through the kernel's configuration resolver, the row's status (`Disabled`, `Unresolved`, `Mounted`).
   `Expr::eval` with the signature of section 2.9, over the evaluator of part A (your notes say how).
3. **`Group`** and the built-in plugins `cordis:group` and `cordis:include` (with their dsh aliases), carriers (L7).
4. **`Loader`** as a service under the key `loader`: `entries`, `resolve`, `create`, `remove`, `settled`,
   `recompose` (the differences between two compositions applied to the live tree: what is created, removed,
   updated in place, committed as a volatile change, rebuilt — section 5.3), `audit`, `dump`; and `boot`.
5. **Configuration reload and start-up diagnostics** of `app-boot` (`index.ts`, `startup-diagnostics.ts`), and the
   part of `profile-sanitize.ts` that part A recorded as waiting for the tree.
6. **`ConfigEditor`** of `packages/boot/config-editor` as a module `config_editor` (a service; 155 lines of source).
   What it writes back to a file is governed by decision O4 (the journal of inclusions is not ported): port
   what does not depend on the journal and list the rest in the notes.
7. **Duplicate explicit ids**, left open by part A: do what the source's tree does with them
   (`vendor/loader/src/config/tree.ts`) and say what that is in the notes.

Not part of this task: the `cmdline` module and the executable `apps/hoh` (a separate task after this one,
built on the crate `hoh-commander`); the extraction of the YAML code into its own crate.

## Tests

Port, keeping case names in snake_case, with expected texts copied exactly:

- `scripts/loader-volatile-update.spec.ts` → `tests/volatile_update.rs` (25 cases; section 8.2 names the ones
  that are not ported and why);
- the cases of `scripts/cordis-entry-commit.spec.ts` that part A left for the tree;
- from `packages/boot/app-boot/tests`: `app-boot`, `config-reload`, `startup-diagnostics`, `profile-sanitize`,
  the remaining cases of `config-dump`, `user-patches` and `profile` that needed a mounted tree, and the cases
  of `config-schema`, `schemastery-json-schema`, `loader-shape.compat` that concern the schema of the
  composition itself (`tests/config_schema.rs`; the projection of schemastery is replaced by `schemars`);
- `tests/shipped_yaml.rs`: each shipped profile of the fixtures composes without L10 warnings, and — new in
  part B — mounts on a kernel with a registry of empty stand-in plugins registered under every name the
  composition uses, reaching `settled` with an empty `audit`.

Tests run on `#[tokio::test(start_paused = true)]`; where a test's subject is concurrency (recomposition while
rows are loading, removal during activation), add a variant on a multi-threaded runtime with a bound and a
timeout. List in `PORT_NOTES.md`, per source test file, the cases ported and the cases not ported with reasons.

## If the kernel lacks something

Do not edit `crates/hoh-kernel`. If a rule of the specification cannot be implemented with the kernel's API as
built, or the kernel behaves differently from its specification, stop work on that point, write the smallest
failing test that shows it under `tests/kernel_gaps.rs` marked `#[ignore = "<reason>"]`, describe it under
"Вопросы ведущему", and go on with everything else.

## Done when

`cargo test -p hoh-loader`, `cargo clippy -p hoh-loader --all-targets -- -D warnings` and
`cargo fmt -p hoh-loader -- --check` are clean; every item of section 2.9 exists with the signature given
there or the difference is listed in the notes; `PORT_NOTES.md` no longer has an open "Часть B" list. Work until
it is done; do not stop with a partial result and a plan.

Final report (your last message): the test command and its result line; cases ported and not ported per
source file, in numbers; differences from section 2.9; what the kernel lacks; open questions. At most 450 words.
