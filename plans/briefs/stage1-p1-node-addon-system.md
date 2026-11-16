# Writer p1: port `native/system` of DeepSeek Harness into the crate `hoh-node-addon-system`

You are writing one Rust crate in the workspace `/home/save/Alexandrie/HarnessOverHarness` (Rust 1.97.1,
edition 2024). A TypeScript agent harness (DeepSeek Harness, "dsh") is being ported to Rust. Its package
`@deepseek-ai/node-addon-system` gives two things written in C: `landlock-run`, a small launcher that confines
the command it starts with a Linux Landlock ruleset (the harness's sandbox for shell commands is built on it),
and a non-blocking `flock(2)` used for session file locks. Both become Rust. The launcher is a security
boundary: what it grants and what it refuses must equal the source exactly.

## Read first

- `docs/porting/PORTING.md` — the project's rules. No `unsafe`; no `unwrap`, `expect` or `panic!` outside tests
  except for a proven invariant with a one-line comment; no global state; the format of `PORT_NOTES.md`
  (section 12); the report (section 13).
- The source, read-only, never build or run anything there:
  `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/native/system` —
  `docs/cli-contract.md` (the launcher's observable behaviour: grammar, exit codes, report lines, confinement;
  this is the contract you implement), `docs/flock-contract.md`, `docs/support-matrix.md`,
  `packages/entry/src/main.c` (the launcher), `packages/entry/src/flock.c` and `flock.ts`,
  `packages/entry/src/index.ts` (the entry: constants, `grantArgs`, `probe`, `launcherPath`),
  `test/launcher.test.js`, `test/flock.test.js`, `test/entry.test.js`.
- The users, for context: `packages/sandbox/sandbox-local/src/index.ts` and `profiles.ts`,
  `packages/session/session-persistence-jsonl/src` (`lease.ts` treats `EAGAIN`/`EWOULDBLOCK` as "busy").
- The Rust libraries you build on, in `~/.cargo/registry/src`: `landlock` 0.4.7 and `rustix` 1.1.6 (feature `fs`).

## What to build, in `crates/hoh-node-addon-system` (the crate exists with a manifest and an empty `lib.rs`)

1. **The launcher as a function and as a binary.** `pub fn run_launcher(args, stdout, stderr) -> u8` (choose
   exact parameter types; it must be testable without a process) implements the whole of `main.c`: the grammar
   `landlock-run [--ro <path>]... [--rw <path>]... -- <argv>...` and `landlock-run --probe`; every usage error;
   the access rights requested for each grant and for each Landlock ABI from 1 to 5 exactly as `main.c`
   negotiates them (which rights exist at which ABI, which rights a grant on a non-directory keeps, read plus
   execute for `--ro`, everything governed for `--rw`); `no_new_privs`; the "partial enforcement" line on an
   older ABI; the probe lines; the fatal lines prefixed `landlock-run: ` with the source's texts; exit code 125
   for every launcher-level failure; and finally replacing the process with the command through
   `std::os::unix::process::CommandExt::exec`, environment unchanged, resolved through `PATH` like `execvp`.
   A binary target `landlock-run` (`src/bin/landlock-run.rs`) is a three-line `main` that calls it.
   Use the safe API of the `landlock` crate. Where its notion of full or partial enforcement differs from what
   `main.c` computes, the source's rule wins: say in the notes how you mapped one to the other.
2. **The entry**: `LAUNCHER_BIN`, `LAUNCHER_FAILURE_EXIT`, `LandlockEnforcement` (`Full`, `Partial`, `Unusable`),
   `LauncherGrants`, `grant_args`, `probe(launcher, timeout)` with the source's default of 2000 ms and its rule
   for reading the output, and `launcher_path`. The source finds the launcher inside an npm platform package;
   that mechanism does not exist here. `launcher_path` returns the file named `landlock-run` next to the
   current executable; state this difference in the notes.
3. **`try_lock_exclusive`**: `flock(fd, LOCK_EX | LOCK_NB)` through `rustix::fs::flock`, with the errors the
   contract names (`EAGAIN`/`EWOULDBLOCK` for a held lock; other failures passed through with their code). The
   source is asynchronous only because of its native binding; the call never blocks, so the Rust function is
   an ordinary function. The file descriptor is borrowed (`AsFd`).

Linux only: on other platforms the source's entry reports what `docs/support-matrix.md` says; reproduce that
for non-Linux targets behind `cfg` without testing it, and list it under "Не проверено".

## Tests

Port `test/launcher.test.js`, `test/flock.test.js` and `test/entry.test.js`, keeping case names in snake_case.
They start real processes: run the launcher binary cargo builds (`env!("CARGO_BIN_EXE_landlock-run")`) with
ordinary system commands (`/bin/sh`, `cat`, `touch`), and use temporary directories from `tempfile`. Landlock is
applied only inside those child processes, never to the test process itself. This machine's kernel has
Landlock enabled. `test/package-matrix.test.js` and `test/release-packing.test.js` concern npm packaging and
are not ported; say so.

Add your own tests for: every usage error of the grammar and its exact stderr line; a read-only grant allows
reading and refuses writing; a read-write grant allows both; a path outside every grant is refused; a grant
on a file (`--rw /dev/null`); a grant root that does not exist (exit 125, command not run); a command that does
not exist (exit 125); a command that itself exits 125 (passed through, no `landlock-run: ` line); a child of
the confined command is confined too; a lock held by another process makes `try_lock_exclusive` fail with the
"busy" error and succeeds after release.

Your own sandbox may itself be built on Landlock; the kernel allows nested rulesets, so the tests should
still run. If a test cannot run inside your sandbox, do not weaken or skip it silently: keep it, and report
the exact error; the lead runs the suite outside the sandbox.

## Boundaries

- You own only `crates/hoh-node-addon-system`. Do not create, edit or delete anything outside it: not the
  root `Cargo.toml`, not other crates, not documents. Other writers are working in this repository right now.
- Dependencies: `landlock`, `rustix` and (for tests) `tempfile` are in the manifest. Other crates only from
  `[workspace.dependencies]` of the root `Cargo.toml`, as `name.workspace = true`.
- Copy the source's `LICENSE` (BSD-3-Clause, `native/system/LICENSE`) into the crate unchanged and say so in
  your report.
- Allowed commands, always with the environment
  `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-p1`:
  `cargo fmt -p hoh-node-addon-system`, `cargo build -p hoh-node-addon-system`,
  `cargo clippy -p hoh-node-addon-system --all-targets -- -D warnings`, `cargo test -p hoh-node-addon-system`.
  Read-only shell commands are fine. Nothing else: no network, no new tools or packages, no Node, no servers,
  no git command that changes state, and never run the launcher against anything but the temporary
  directories of your tests.

## Done when

`cargo test -p hoh-node-addon-system` passes (or the report names exactly which tests your sandbox prevented
and why); `cargo clippy -p hoh-node-addon-system --all-targets -- -D warnings` and
`cargo fmt -p hoh-node-addon-system -- --check` are clean; `PORT_NOTES.md` is written in the format of PORTING.md
section 12 (in Russian), with a table of the access rights per ABI as you implemented them next to the lines
of `main.c` they come from.

Work until it is done; do not stop with a partial result and a plan. Your last message is the report: the test
command and its result line, source cases ported and not ported, how full and partial enforcement are
decided, anything that differs from this task, open questions. At most 350 words.
