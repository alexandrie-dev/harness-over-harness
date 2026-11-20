# Writer ui2: window base, WebKitGTK probe, supply-chain layers

Working directory `/home/save/Alexandrie/HarnessOverHarness`. Read `docs/window.md` in full (Russian) and
`research/supply-chain-ts.md`. You do work packages **1 and 8** of section 6: 8 first (the guard rails), then 1.
The founder allowed downloading the window's npm packages; this run has network access for that and for crates.io.
Nothing else may use the network.

## Rules for every download (package 8 makes them enforced, not just followed)
- pnpm (12.21 on the machine), versions pinned exactly as section 1 of `docs/window.md` lists (the 18 direct
  packages; `@tauri-apps/cli` and `@tauri-apps/api` of the Tauri 2.12.x line are added, pinned). No other direct
  package without a line in `docs/ts/DEPS.md` saying why.
- Install scripts off (`ignore-scripts`, and pnpm's allow-list of build scripts empty except a recorded
  exception); a release age of at least 7 days for every package in the lock (pnpm's `minimumReleaseAge` or what this pnpm version
  calls it — establish it from `pnpm help` and `pnpm config list`, not from memory); lock file with integrity hashes; builds only with `--frozen-lockfile --offline` after
  the first resolve.
- `fsevents` and other optional platform packages not installed. `@napi-rs/canvas` not installed.
- Rust side: Tauri 2.12.x crates with minimal features in `apps/hoh-desktop`, which is **its own workspace** (the
  root `Cargo.toml` already excludes it; it gets its own `Cargo.lock`). Build with `--locked` after the first
  resolve. `CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-ui2
  CARGO_HOME=/mnt/toshiba/alexandrie/hoh/cargo-home-ui2` (a separate cargo home: the shared one is not writable
  for you).
- pnpm store: `store-dir=/mnt/toshiba/alexandrie/hoh/pnpm-store` in `ui/.npmrc` (outside it you cannot write).

## Package 8: `scripts/ui-supply-check.sh`, `ui/.npmrc`, `ui/pnpm-workspace.yaml`, `docs/ts/DEPS.md`, `deny.toml`
As the table in section 6 says. The script fails on: a lock file changed without hashes, a version younger than
7 days, a package with an install script (except a recorded exception), a licence outside the set in
`docs/ts/DEPS.md`, a known vulnerability (`pnpm audit`; `cargo deny check` only if `cargo-deny` is installed —
it is not yet; make the script say "skipped: cargo-deny not installed" and not fail on that alone). Prove each
failure on a planted bad input (kept under `scripts/tests/ui-supply/`).

## Package 1: base, tokens, shell, WebKitGTK probe
Files as in the table. Done when: `pnpm install --frozen-lockfile --offline` then the build gives `ui/dist/` with
no external URLs and no `eval`; a snapshot of the empty shell in headless Chromium (`/usr/bin/google-chrome` or the
playwright headless shell in `~/.cache/ms-playwright`, through `playwright-core`) matches a committed reference
image; the Tauri window opens under `xvfb-run`, and the probe page records the WebKitGTK version (2.52.6 on this
machine) and whether `oklch()`, `color-mix()` and `@layer` work, into `docs/ts/webkitgtk-probe.md`. If WebKitGTK
fails on any of them, stop there and report precisely: that decides the shell.

## Boundaries
You own `ui/` (except `ui/src/protocol.ts`, which writer ui1 generates), `apps/hoh-desktop/`, `scripts/ui-supply-check.sh`,
`scripts/tests/ui-supply/`, `docs/ts/`, `deny.toml`. Not the root `Cargo.toml`, not `crates/`, not other documents.
No global installs (`npm -g`, `cargo install`, `apt`): everything local to `ui/` or the desktop workspace. Do not
run the window against the real home directory: `HOME` set to a temporary directory for every run of it.

Final report (last message): what was downloaded (counts of npm packages and crates in the locks), the probe
result, each check of package 8 with its planted failure, anything in `docs/window.md` that turned out wrong.
At most 400 words.
