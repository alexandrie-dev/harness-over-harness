# Fix round for `hoh-node-addon-system` after an independent review

Your crate is accepted as a base: the lead compared `launcher.rs` with `main.c` line by line and the reviewer
confirmed from the source of `landlock` 0.4.7 that the handled rights, the rights of each grant and the
full/partial rule equal the source for every kernel ABI. What follows is what the review found missing.
The boundaries and allowed commands of `plans/briefs/stage1-p1-node-addon-system.md` still hold: you own only
`crates/hoh-node-addon-system`.

Rule for every item that asks for a test: write the test first, see it fail for the reason named (or, where
the code is already right, break the code temporarily to see the test fail, then restore it), then finish.
If you disagree with an item, do not change the code for it: say so in the report with your evidence.

## A. The launcher is shipped as a static executable (decided by the lead; already measured)

An ordinary build of `landlock-run` starts through the dynamic loader. Measured on this machine:
`LD_PRELOAD=/nonexistent-preload.so target/debug/landlock-run --probe` prints
`ERROR: ld.so: object ... cannot be preloaded`, so the loader acts on the variable before the launcher
restricts itself; a library that does exist would run unconfined. The source's launcher is a static musl
executable and its contract says "no environment-variable inputs". The lead added
`scripts/build-landlock-run.sh` (do not edit it): it builds the binary with
`-C target-feature=+crt-static`, checks that the file has no loader and no shared libraries, and runs
`cargo test -p hoh-node-addon-system --test launcher --test entry -- --include-ignored` with the
environment variable `HOH_LANDLOCK_RUN` set to the static file. A static build with the same flag printed
nothing for the same variable. Your part:

1. Every test in `tests/launcher.rs` and `tests/entry.rs` that runs the real launcher takes its path from
   `HOH_LANDLOCK_RUN` when that variable is set, and from `env!("CARGO_BIN_EXE_landlock-run")` otherwise.
   One helper per test file; no other behaviour changes with the variable.
2. A new test `static_launcher_ignores_loader_variables` in `tests/launcher.rs`, marked
   `#[ignore = "passes only for the static launcher: run scripts/build-landlock-run.sh"]`: run the launcher
   with `--probe` and the variables `LD_PRELOAD=/nonexistent-hoh-preload.so`,
   `LD_LIBRARY_PATH=/nonexistent-hoh-libs`, `LD_AUDIT=/nonexistent-hoh-audit.so`; stderr must be exactly
   empty, stdout exactly one of the two probe lines, exit status 0. (Use `--probe`, not a command: a command
   such as `/bin/true` is itself dynamic and its own loader would print the message after confinement.)
   You may run `bash scripts/build-landlock-run.sh` to see it pass; set
   `HOH_LANDLOCK_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-p1-static` for that run.
3. `PORT_NOTES.md`: a section on linkage. The shipped launcher is the static file built by the script;
   the binary cargo builds for tests is dynamic and is not to be shipped. Record what still differs from the
   source's musl build, all of it after the ruleset is in force: `execvp` of glibc runs a file without an
   interpreter line through `/bin/sh` (measured: the command ran and exited 0) where musl fails with
   `Exec format error` and the launcher would exit 125; the default search path when `PATH` is unset; the
   texts of a few rare error numbers; glibc reads `GLIBC_TUNABLES` even in a static executable.

## B. Tests that prove what is confined (all in `tests/launcher.rs`)

Today every refusal the tests check is one that the write-file right of ABI 1 already gives, so the suite
would pass if the ruleset handled only the ABI 1 rights. Each test below must fail if its right were not
handled; say in a one-line comment which right it proves. They need a kernel with Landlock ABI 5 or newer:
if the probe does not report `Full`, fail with a message that says so — never pass silently. All files live
in a `tempfile` directory. The lead measured each expected result on this machine.

4. **Refer (ABI 2).** With `--ro /` and `--rw W`: a hard link from `W/a/f` to `W/b/g` (different directories
   inside the grant) succeeds; a hard link from a file in a read-only directory into `W` fails with
   `EXDEV`. Use `link(2)` or `rename(2)` directly (`std::fs::hard_link`, `std::fs::rename`, or `ln`), not
   `mv`: `mv` falls back to copying when the kernel refuses. The success case is the proof: this right is
   refused by default unless the ruleset handles it and a rule grants it.
5. **Truncate (ABI 3).** With `--ro /`: `truncate(2)` BY PATH on a file fails with `EACCES` and the file
   keeps its length; with `--rw` on its directory it succeeds. It must be the path form of the call: opening
   for writing and then truncating is refused by the write-file right alone and proves nothing. Neither the
   standard library nor a shell redirection makes that call; `python3 -I -c 'import os; os.truncate(p, 0)'`
   and `perl -e 'truncate($ARGV[0], 0) or die'` do, and both tools are on this machine. If the tool you
   choose is missing, the test fails with a message naming it.
6. **Ioctl on devices (ABI 5).** With `--ro /` only: opening `/dev/zero` for reading and calling the
   `FIONREAD` ioctl on it fails with `EACCES`; with `--ro / --rw /dev/zero` the same call fails with
   `ENOTTY`, which is what it gives unconfined. (`rustix::io::ioctl_fionread` from a child that is the test
   executable itself, the way `tests/flock.rs` already starts a helper, or `python3` with `fcntl.ioctl`.)
7. **Full or partial, strictly** (the source's `launcher.test.js:90-104`): run the probe, then a confined
   `true`; the line `landlock-run: partial enforcement (older Landlock ABI)` is present on stderr exactly
   when the probe reported partial, and stderr is otherwise empty. The helper `notice` that accepts either
   state is replaced by this rule wherever a test checks stderr of a successful confined run.
8. **A grant on a regular file.** `--rw FILE`: the command can read, overwrite and truncate FILE and cannot
   create a sibling in the same directory. `--ro FILE` (with `--ro` on what the command itself needs, not
   on the directory of FILE): it can read FILE and cannot write it.
9. **A command that cannot be executed**: a regular file without the execute bit, and a directory. Each:
   exit 125, stderr is exactly `landlock-run: exec failed: Permission denied` plus the notice rule of item 7.
10. **Signals as the command sees them**: the confined command reads `SigIgn` from `/proc/self/status`;
    the bit of `SIGPIPE` (signal 13) is clear. The Rust runtime ignores that signal at start-up and the
    standard library restores the default before `execvp`; this test pins that a pipeline inside the
    sandbox still dies of a broken pipe.
11. In `tests/entry.rs`: `grant_args` with a path that is not valid UTF-8 returns it unchanged.

## C. Smaller things

12. `src/flock.rs`: remove the condition on the processor architecture. It reproduces which prebuilt npm
    packages exist, not a behaviour: `try_lock_exclusive` calls `flock` on every Unix target, and only a
    non-Unix target returns the unsupported error.
13. `PORT_NOTES.md`: the test counts are those `cargo test` prints, with a note that `flock_child` is the
    entry point of a helper process and not a case; "all behavioural cases of `launcher.test.js`" becomes
    true with item 7. Add under the differences: the Rust runtime opens `/dev/null` on a standard
    descriptor that was closed at start-up (the source passes the closed descriptor on); the signal mask
    is inherited unchanged; after a timeout `probe` sends `SIGTERM` and waits for the child without a limit,
    as `spawnSync` does. Under "Не проверено": the refusal on a kernel without Landlock (no kernel to try
    it on), kernels with ABI 1 to 4, non-Linux targets.

## Done when

`cargo test -p hoh-node-addon-system` passes with exactly one ignored test (item 2);
`bash scripts/build-landlock-run.sh` passes (it runs that test too);
`cargo clippy -p hoh-node-addon-system --all-targets -- -D warnings` and
`cargo fmt -p hoh-node-addon-system -- --check` are clean.

Work until it is done. Your last message is the report: the result line of each command, one line per item
(done, or disputed with evidence), for items 4 to 6 how you saw each test fail before it passed, and
anything that differs from this task. At most 350 words.
