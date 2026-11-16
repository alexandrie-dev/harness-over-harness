# Stage 0, writer l1: the HoH loader, part A (everything that does not need a live kernel)

You are implementing the loader of HoH, a Rust agent harness. Working directory:
`/home/save/Alexandrie/HarnessOverHarness` (a cargo workspace, Rust 1.97.1, edition 2024).

## Read first, in full

1. `docs/kernel.md` — the binding specification. Yours are section 2.9 (the public API of `hoh-loader`),
   section 5 (the loader: YAML dialect, rows, groups, includes, layers, patches, recomposition, rules L1–L19, and
   the closed set of configuration expressions with the full list in 5.5), the loader rows of the test plan
   (8.2–8.4) and the lead's decisions (section 10). Read sections 1–2 too, to know the kernel types part B will use.
2. `docs/porting/PORTING.md` — the project's coding rules (sections 4, 6, 7, 8, 10, 12 apply to you).

## Sources (read-only: never run or build anything there)

Root: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness`.

- Code: `vendor/loader`, `vendor/include`, `vendor/group` (with the local fixes described in `vendor/README.md`),
  `packages/boot/app-boot/src` (`index.ts`, `profile.ts`, `profile-context.ts`, `bundle-platform.ts`,
  `profile-sanitize.ts`).
- Tests: `scripts/cordis-entry-yaml.spec.ts`, `scripts/cordis-entry-commit.spec.ts`, and
  `packages/boot/app-boot/tests/` (`profile`, `user-patches`, `config-dump`, `config-pattern`, `bundle-platform`,
  `profile-sanitize`).
- Real configuration the loader must read unchanged: `packages/bundle/*/cordis.patch.yml`,
  `packages/bundle/web-app/presets/*.patch.yml`, and every `*.yml` under `snapshots/`.

## Why "part A"

Another writer is implementing `crates/hoh-kernel` right now, and it does not work yet. Part A is everything in
`hoh-loader` that is pure: text in, data out. The manifest of your crate deliberately has no dependency on
`hoh-kernel`; do not add one. Part B (rows mounted as fibers, `Group`, `Loader`, `boot`, `recompose`, `audit`,
`PluginRegistry`, `cmdline`, `config-editor`) is a later task and is not yours now.

## Part A, in `crates/hoh-loader`

1. **YAML reading** with `saphyr-parser` (L1, L3, L4): `parse_entries`, `parse_patches`, `Entry`, `Isolate`, `Patch`,
   `LoadError` with file, line and column. The scalar rules of js-yaml 4 `JSON_SCHEMA` exactly; check the numeric
   forms against `node_modules/js-yaml/lib/type/{int,float}.js` in the reference tree if it is there, otherwise
   against the js-yaml documentation you know, and say in `PORT_NOTES.md` which it was. A `!!js` scalar becomes an
   expression node. An unknown row key is a shape error.
2. **YAML writing** (L2): the loader's own writer, used by `render_dump`.
3. **Patches and layers** (L9, L10, L18): `apply_patches`, `compose`, `Composition`, `Origin`, `Layer`, `LayerSource`,
   `render_dump`, with the warning texts of L10 byte for byte, and the anchoring of relative plugin names in
   inserted rows to the patch file's directory (L5, `anchorInsertedPluginNames`).
4. **Expressions** (5.5): `Expr::parse` over the closed grammar. Every expression listed in 5.5 must parse; any
   other text is an error carrying the position. Evaluation of every term in part A goes through one internal
   function that takes the `EvalEnv`, the base directory, and two lookups supplied by the caller: service data by
   name (`&dyn Fn(&str) -> Option<serde_json::Value>`, where `None` means the service is absent) and a package's
   resource directory by name. The public `Expr::eval` of section 2.9, which takes a kernel `Context` and a
   `PluginRegistry`, is part B and will call this function. JavaScript semantics where the expressions rely on
   them (`??`, `||`, `&&` returning operands, truthiness, `===`, `Number(..)`, `JSON.parse`, string `+`), as the
   table in 5.5 says. `crates/hoh-util-values` has `js_string` and `js_number_string`; you may depend on it by `path`.
5. **Profile** (L3, L9, decision O6 in section 10): `Profile`, `Bundle`, `BundleSource`, `SkippedBundle`,
   `Profile::load`, `EvalEnv`. dsh's profile format as it is (`package.json` with `dsh.profile.bundles`); a bundle
   that is missing, unreadable or not for this platform is skipped with a reason; no npm package resolution,
   version or compatibility logic.
6. **Tests**: `tests/entry_yaml.rs`; the patch cases of `tests/entry_commit.rs`; `tests/expr.rs` (every expression of
   5.5 parses; a table of values for a given `EvalEnv` and given lookups; text outside the grammar fails with its
   position); `tests/shipped_yaml.rs` (copy the shipped `cordis.patch.yml` files and presets into
   `tests/fixtures/dsh/`, keeping their relative paths, and check that each reads, that every expression in them
   and in the `snapshots/**/*.yml` files parses, and that the layers of a real profile (for example the bundles `base`
   and `headless`, in the order the reference tree declares them) compose over the base file the boot code uses
   without L10 warnings; if the reference tree has no such profile or base file on disk, say so in the report and
   compose the bundles over the rows they insert); and the pure cases of the `app-boot` test files named above, in files of the
   same names. A case that needs a live kernel is listed in `PORT_NOTES.md` under "Не перенесено" with the words
   "часть B". Keep the source's case names as test names (snake_case).

Everything public has the names and signatures of section 2.9. An item whose signature mentions a kernel type
(`Context`, `Fiber`, `Plugin`, `PluginDef`, `Service`, `KernelError`) is part B: leave it out and list it in
`PORT_NOTES.md` under "Часть B". If a signature turns out to be impossible or wrong, implement the closest working
form and record it under "Отличия от спецификации". If the shipped YAML contains an expression outside the
grammar of 5.5, do not widen the grammar on your own: list the expression with its file in your report.

## Boundaries

- You own only `crates/hoh-loader`. Do not create, edit or delete anything else. Other writers are working in
  this repository now.
- Dependencies: those already in your `Cargo.toml`; `hoh-util-values = { path = "../hoh-util-values" }`; any crate
  already named in `[workspace.dependencies]` of the root `Cargo.toml`, as `name.workspace = true`. Nothing else.
  If you need more, finish what you can and say so in the report.
- No `unsafe`, no global mutable state, no `println!`, no `unwrap`, `expect` or `panic!` outside tests except for a
  proven invariant with a one-line comment. Tests never read the real home directory or the real environment.
- Allowed commands, always with the environment
  `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-l1`:
  `cargo fmt -p hoh-loader`, `cargo build -p hoh-loader`, `cargo clippy -p hoh-loader --all-targets -- -D warnings`,
  `cargo test -p hoh-loader`. Read-only shell commands are fine. Nothing else: no network, no new tools or
  packages, no servers, no sub-agents, no git command that changes state.
- If cargo fails because the manifest of a crate that is not yours is momentarily broken, wait and retry.

## Done when

`cargo test -p hoh-loader`, clippy with `-D warnings` and `cargo fmt -p hoh-loader -- --check` are clean, and
`crates/hoh-loader/PORT_NOTES.md` lists what is ported, what waits for part B, and every deviation.

Finish part A in this run; do not hand back partial work with a plan for the rest. Final report (your last
message): the test command and its result line; what part B needs from the kernel; deviations; expressions outside
the grammar, if any; open questions. At most 400 words.
