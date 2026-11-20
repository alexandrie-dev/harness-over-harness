# l1: cmdline and the minimal `apps/hoh` (closing stage 0)

Your loader part B is in (verify running). The last piece of stage 0 is the launcher: PLAN.md stage 0 says
"`hoh --dump-config` prints the assembled profile tree". This is it. Re-read `docs/porting/PORTING.md`.

## First: three loader APIs that `hoh-settings` needs

w5 stopped porting `packages/settings/settings` because the loader lacks three things; read
`crates/hoh-settings/PORT_NOTES.md` (sections 1–3 under the questions) for the source lines:
1. A fallible `ConfigEditor` edit callback that can reject inside the transaction, after reload and before the
   write, passing the caller's error through unchanged (the source's `SettingsConflictError`).
2. The schema of a mounted entry and inherited-config resolution in that entry's context without the current
   fiber's config hooks.
3. Read-only access to the active profile's `home` and `name`.
Add them as the source's loader/runtime provides them (find the source side in `vendor/loader` and app-boot), with
tests, before the launcher work; report the API so w5 can continue.

## Sources (read-only clone `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness`)
- `packages/boot/cmdline` (`src/index.ts`, 239 lines; `tests/cmdline.spec.ts`) → into `crates/hoh-loader`, as
  MAP.tsv row 3 says (module `cmdline`), or a separate `crates/hoh-cmdline` if the dependency direction requires
  it — decide and record why.
- `apps/cli/src`: `args.ts`, `bin.ts`, `profile-boot.ts`, `dump-config.ts`, `dump-config-schema.ts`,
  `startup-diagnostics.ts`, `process-shutdown.ts`, `plugin.ts`, `installed-bin.ts` → `apps/hoh` (binary crate
  named `hoh`). Stage-0 scope: what is needed to parse the command line, resolve the config path and env
  (`resolveConfigPath`, `loadEnv`/`loadLayeredEnv`, `installFailLoud`, the bootstrap `.env` prohibitions), boot a
  profile with the loader, `--dump-config` and `--dump-config-schema`, startup diagnostics, shutdown on signals.
  Anything that needs packages not ported yet (sessions, agent, SDK server): leave out with a line in
  PORT_NOTES naming the missing crate.
- Tests: `tests/cmdline.spec.ts`; app-boot #1–33 that you assigned to this task in `crates/hoh-loader/PORT_NOTES.md`
  (lines ~460 on); from `apps/cli/tests`: `args.spec.ts`, `process-shutdown.spec.ts`, `startup-diagnostics.spec.ts`,
  `resolved-profile-boot.spec.ts`, `profile-initialization.spec.ts`, `dump-config-schema.expected.e2e.ts` (its
  `expected/` output byte for byte, for what the ported packages can produce), the rest only as far as they apply
  without missing crates — list each source test file as ported / partly (which cases) / not applicable (why).

## Boundaries
- You own `crates/hoh-loader`, `apps/hoh`, and (if chosen) `crates/hoh-cmdline`. The root `Cargo.toml`: you may
  change only the `members` line, to `["crates/*", "apps/hoh"]` (not a glob: `apps/hoh-desktop` is a separate workspace), and only once `apps/hoh/Cargo.toml` and
  `apps/hoh/src/main.rs` exist and build (other writers build the workspace at the same time).
- Dependencies: workspace crates and `[workspace.dependencies]` only; command-line parsing as the source does it
  (check whether a parser is in the workspace list; if you need one that is not, stop on that point and report).
- `CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-l1`. Running
  your binary is allowed with a temporary HOME and temporary directories only, never the real home.

## Done when
`cargo test`, `clippy --all-targets -D warnings`, `fmt --check` clean for your crates; `hoh --dump-config` with the
shipped headless profile prints the tree and `--dump-config-schema` matches the source's expected file as far as
the ported packages go (state the difference precisely); every source test case listed with its fate.
Report: result lines; the test table; what was left out and which crate it waits for. At most 500 words.
