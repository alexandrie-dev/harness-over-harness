# Shared helpers for what Node and JavaScript do by themselves

Working directory: `/home/save/Alexandrie/HarnessOverHarness` (a cargo workspace, Rust 1.97.1, edition 2024).
You are writer `nc1`; your build directory is `/mnt/toshiba/alexandrie/hoh/target-nc1`.

HoH is a port of DeepSeek Harness (TypeScript on Node 22) to Rust. Recorded sessions of the source are
replayed against the port and compared byte for byte, so the port must reproduce what Node itself puts
into texts: how a string is trimmed, how a path is joined, what a failed `readFile` says. The first reviews
found each writer redoing these by hand, each a little differently (three private path normalisers exist
already). This task adds them once, in three existing shared crates. Sixty packages will use them.

Read `docs/porting/PORTING.md` first (in Russian; the binding rules — sections 5, 7, 10 and 12 matter here).

The definition of correct behaviour is not this text and not your memory of Node: it is the fixtures the
lead recorded from Node 22.23.1. You have no Node source and need none; do not run Node. Write each
function from the behaviour the fixture shows, and test it against every line of the fixture.

## 1. `crates/hoh-util-values`: JavaScript white space

Fixture `tests/fixtures/js_white_space.json`: `whiteSpace` — every code point that JavaScript's `\s`
matches (25 of them); `trims` — inputs with what `trim()`, `trimStart()`, `trimEnd()` return. JavaScript and
Rust disagree: U+FEFF is white space only for JavaScript, U+0085 and U+001C–U+001F only for Rust's
`char::is_whitespace`.

New file `src/js_text.rs`, re-exported from `src/lib.rs`:

```rust
/// What `\s` matches and `trim()` removes in JavaScript.
pub fn is_js_white_space(ch: char) -> bool;
pub fn js_trim(text: &str) -> &str;
pub fn js_trim_start(text: &str) -> &str;
pub fn js_trim_end(text: &str) -> &str;
/// The same set as the inside of a character class of the `regex` crate:
/// `format!("[{JS_WHITE_SPACE_SET}]+")` is JavaScript's `\s+`, `format!("[^{JS_WHITE_SPACE_SET}]")` its `\S`.
pub const JS_WHITE_SPACE_SET: &str;
```

Tests (`tests/js_text.rs`): `is_js_white_space` agrees with the fixture for every `char` (all of them, not a
sample); every `trims` entry; a `regex` built from `JS_WHITE_SPACE_SET` matches exactly the fixture's set
(`regex` as a dev-dependency).

## 2. `crates/hoh-error`: the texts of Node's system errors

Fixture `tests/fixtures/node_system_errors.json`:
- `table`: Node's `util.getSystemErrorMap()` on Linux — `errno` (negative), `code` (`ENOENT`), `description`
  (`no such file or directory`; libuv's text, not the C library's);
- `fsMessages`: what `fs/promises` functions really threw for common failures, e.g.
  `ENOENT: no such file or directory, open '<root>/missing'`,
  `ENOENT: no such file or directory, rename '<root>/missing' -> '<root>/other'`,
  `EISDIR: illegal operation on a directory, read` (`<root>` stands for the scratch directory);
- `spawnMessages`: what `child_process.execFile` reported. Node has two shapes: for `EACCES`, `EAGAIN`,
  `EMFILE`, `ENFILE`, `ENOENT` the message is `spawn <command> <CODE>`; for every other code it is
  `spawn <CODE>` without the command.

New file `src/node.rs`, re-exported from `src/lib.rs`:

```rust
/// The name Node gives the operating system's error: "ENOENT". `None` when the error carries no OS error
/// number or libuv has no name for it.
pub fn node_error_code(error: &std::io::Error) -> Option<&'static str>;
/// libuv's text for a code: `node_error_description("ENOENT") == Some("no such file or directory")`.
pub fn node_error_description(code: &str) -> Option<&'static str>;
/// The message of the error a Node `fs` function throws:
/// `<CODE>: <description>, <syscall>`, then ` '<path>'` when a path is given, then ` -> '<dest>'`.
pub fn node_fs_error_message(error: &std::io::Error, syscall: &str, path: Option<&str>, dest: Option<&str>) -> String;
/// The message of a failed `child_process` start (the two shapes above).
pub fn node_spawn_error_message(error: &std::io::Error, command: &str) -> String;
```

- The error number of an `io::Error` differs between operating systems (`EAGAIN` is 11 on Linux, 35 on
  macOS): map through named constants (`rustix::io::Errno`, add `rustix.workspace = true` under
  `[target.'cfg(unix)'.dependencies]`), never through the numbers of the fixture. The descriptions belong to
  the codes and are the same everywhere: the table of code → description covers every code of the fixture.
- When there is no code (no OS error number, an unknown one, or not Unix), the two message functions use
  the error's own `Display` text in place of `<CODE>: <description>`; say in the notes that this branch is
  not checked against Node.
- The syscall word is the caller's: `fsMessages` shows which word Node uses for which function
  (`readFile` → `open`, `readdir` → `scandir`, `copyFile` → `copyfile`, `rm` → `lstat` …). Put that
  list into the notes as a table, taken from the fixture, for the writers who will call you.

Tests (`tests/node.rs`, Linux): every row of `table` whose errno the operating system can produce maps back
to its code (build the `io::Error` with `from_raw_os_error(-errno)`), and every code has its description;
reproduce with `std::fs` in a temporary directory at least these `fsMessages` cases and compare the built
message with the fixture's after replacing the directory with `<root>`: readFile missing, readFile
directory, readFile under a file, writeFile exclusive onto file, mkdir existing, readdir missing, readdir
file, stat missing, rename missing, unlink directory, rmdir not empty, copyFile missing, link missing,
symlink onto file, chmod missing; reproduce with `std::process::Command` the four `spawnMessages` cases
`missing`, `not executable`, `directory`, `under a file`. `tempfile` and `serde_json` as dev-dependencies.

## 3. `crates/hoh-util-workspace-path`: Node's `path.posix`

Fixture `tests/fixtures/node_path.jsonl`, 8835 lines `{"fn","args","out"}` recorded from `path.posix`:
`normalize`, `dirname`, `basename`, `extname`, `isAbsolute`, `join` (0 to 3 parts), `resolve` (the first
argument is always the absolute working directory, where Node would ask the process), `relative` (recorded
as `[cwd, from, to]`: Node resolves both against the working directory first).

New file `src/node_path.rs`, public as the module `hoh_util_workspace_path::node_path`:

```rust
pub fn normalize(path: &str) -> String;
pub fn join(parts: &[&str]) -> String;
/// `cwd` is absolute; it takes the place of `process.cwd()`.
pub fn resolve(cwd: &str, parts: &[&str]) -> String;
pub fn relative(cwd: &str, from: &str, to: &str) -> String;
pub fn dirname(path: &str) -> String;
pub fn basename(path: &str) -> String;
pub fn extname(path: &str) -> String;
pub fn is_absolute(path: &str) -> bool;
```

Strings in, strings out, `/` only: these are Node's rules for POSIX paths whatever the host is.
`std::path` normalises differently (`a//b`, `..`, trailing `/`, empty parts) and must not be used inside.
Test (`tests/node_path.rs`): every line of the fixture.

## Boundaries

- You may create exactly the three source files and three test files named above, add the lines that
  export them to the three `src/lib.rs`, add the named dependencies to the three `Cargo.toml`, and append
  one section "Помощники Node (без пары в образце)" to each crate's `PORT_NOTES.md` (in Russian: every new
  public item, why it exists, what is not checked). Change nothing else in those crates — other code in
  them is accepted and other writers build against it right now — and nothing outside them. Do not edit
  the fixtures or the `gen-*.mjs` files next to them; if a fixture looks wrong, say so in the report.
- Do not replace the private helpers other crates have; the lead does that afterwards.
- Dependencies only from `[workspace.dependencies]` of the root `Cargo.toml`, written `name.workspace = true`.
- Allowed commands, always with
  `CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-nc1`:
  `cargo fmt -p <crate>`, `cargo build -p <crate>`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
  `cargo test -p <crate>`. Read-only shell commands are fine. No network, no Node, no new tools, no git
  command that changes state.
- No `unwrap`/`expect`/`panic!` outside tests; no `unsafe`.

## Done when

For `hoh-util-values`, `hoh-error` and `hoh-util-workspace-path`: `cargo test -p <crate>` passes with every
earlier test unchanged, clippy with `-D warnings` and `cargo fmt -- --check` are clean, the notes are
written. Work until it is done; do not stop with a partial result.

Your last message is the report: per crate the result line of each command; anything in a fixture you
could not reproduce and why; anything you decided differently from this task. At most 300 words.
