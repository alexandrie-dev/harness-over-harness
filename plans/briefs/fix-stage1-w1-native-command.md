# Fix round for `hoh-native-command` after the lead's decisions and an independent review

Only `crates/hoh-native-command`. Same boundaries and commands as `plans/briefs/stage1-writer.md`; environment
`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w1`.

What the reviewer checked and found identical to the source stays as it is: the JXA and C# texts, the
PowerShell script and its arguments, the osascript arguments, the macOS de-duplication, the `gio` and `env`
commands, the icon search order, the WSL branches, the `BROWSER` and `defaults` fallbacks, the buffer limit.

Since your first delivery the project gained shared helpers for what Node and JavaScript do by themselves,
each tested against Node 22 itself. Re-read `docs/porting/PORTING.md` section 4 (the new bullet "То, что
Node и JavaScript делают сами") and section 7 (first two bullets, rewritten), and read the notes of the
three crates before you start: `crates/hoh-util-values/PORT_NOTES.md` (`js_trim`, `JS_WHITE_SPACE_SET`),
`crates/hoh-error/PORT_NOTES.md` (`node_fs_error_message`, `node_spawn_error_message`, `node_error_code`,
the table of syscall words), `crates/hoh-util-workspace-path/PORT_NOTES.md` (`node_path::*`).
Add to your manifest: `hoh-timeout`, `hoh-error`, `hoh-util-values`, `hoh-util-workspace-path` (path
dependencies; the corrected Dependencies bullet of the stage brief allows the shared crates).

The facts below marked "Node:" were measured by the lead on Node 22.23.1.

## A. Project rules

1. **Cancellation is `hoh_timeout::AbortSignal`** (`PORTING.md` section 6). The crate's own `CommandSignal`
   trait goes away; the public functions take the shared signal, and its reason is what an aborted call
   returns, with identity kept. The lead's launch message misled you here; the brief is corrected.
2. **Helper output is parsed with `hoh_util_values::js_json_parse`**, not `serde_json::from_str`
   (`JSON.parse` in the source; the two differ on duplicate keys, big numbers and error texts).
3. **`std::env::vars()` panics on a variable that is not valid Unicode** (`host_environment.rs:49`,
   `path_opener.rs:25`); every public entry point reaches it. `vars_os()` with `to_string_lossy()`, as
   `crates/hoh-launch-environment` does. Test through the crate's seam for the environment, not by setting
   variables of the test process.
4. **Blocking calls inside async code** (`file_applications_windows.rs`: `tempdir()` and the recursive
   removal on `close()`): move them off the async thread.

## B. Differences from the source

5. **White space.** `desktop_entry.rs:15`, `:27`, `:28` and `file_applications_linux.rs:59`, `:74`, `:82`,
   `:83` use Rust's `trim`, `split_whitespace` and `char::is_whitespace`. Node: `'\uFEFF[Desktop
   Entry]'.trim()` is `'[Desktop Entry]'`, so a desktop file that starts with a byte order mark has its main
   section recognised by the source and missed by the port (`readFile(…, 'utf8')` keeps the mark); U+0085
   is trimmed by Rust and not by JavaScript. Use the shared functions everywhere the source uses `trim()`
   or `\s`. `file_applications.rs:44` already adds U+FEFF by hand — replace that too. Test:
   `"\u{feff}[Desktop Entry]\nName=A"` yields `Name`.
6. **The two expressions over `gio mime` output** (`file-applications-linux.ts:44`, `:46`) are re-done by
   hand and differ. Node:
   - `/^Default application.*:\s*(.+\.desktop)\s*$/m` on `Default application for 'a:b': x.desktop` captures
     `x.desktop` (the `.*:` is greedy, the last colon wins; the port splits at the first);
   - on `Default application for x: .desktop` it captures ` .desktop`, with the leading space;
   - `/^\s+.*\.desktop\s*$/` is true for `"\t.desktop"`, `"\ta.desktop"`, `" .desktop "` and false for
     `".desktop"` and `"a.desktop"`.
   Write both with the `regex` crate as the source has them: `\s` as `[{JS_WHITE_SPACE_SET}]`, `.` as
   `[^\n\r\u{2028}\u{2029}]`, and the `m` flag only where the source has it. Tests with these five inputs.
7. **Order of `readdir`.** Node: `readdir`, with and without `withFileTypes`, returns names sorted by bytes
   on Unix; `read_dir` returns them in the order of the file system. `desktopFile`
   (`file-applications-linux.ts:11-27`) walks that sorted list twice (files, then directories), so with
   `a/b-c.desktop` and `a-b/c.desktop` both giving the id `a-b-c.desktop` the source always finds
   `a/b-c.desktop`. Sort the entries by the bytes of their names before the two passes. Test: create the two
   files in both orders of creation and get the same winner.
8. **Paths.** Node: `dirname('/')` is `/`, `dirname('/a/.')` is `/a`, `join('/x//y', 'applications')` is
   `/x/y/applications`, `join('./x', 'applications')` is `x/applications`. The port uses `Path::parent`
   (`path_opener.rs:309-312`: `/` gives `.`) and `Path::join` (`desktop_entry.rs`, `file_applications_linux.rs`),
   and the results are passed on (`xdg-open <dir>`, the application id given to `gio launch`). Every
   `join`, `dirname`, `relative`, `isAbsolute` of the source becomes the `node_path` function of the same
   name. Tests: reveal of `/` runs `xdg-open /`; `XDG_DATA_HOME=<root>//` gives the same id as `<root>`.
9. **The target handed to Explorer** (`explorerTarget`, `path-opener.ts:109-116`; port
   `path_opener.rs:82-141`). The lead recorded what the source's function returns for 136 Windows paths:
   `tests/fixtures/explorer_target.jsonl`, lines `{"path","target"}` (lead-owned, with its generator; do not
   edit). The port differs on many: Node percent-encodes `"`, `<`, `>`, `` ` ``, `{`, `}`, `^`, `|` and the
   control characters, lower-cases a UNC host (`\\SERVER\Share\File.TXT` → `file://server/Share/File.TXT`),
   resolves `C:\..\file.txt` to `file:///C:/file.txt`, and gives `file:///C://` for `C:\`. Make the port
   match every line; a test reads the fixture and checks all 136 through the public opener with the
   Windows platform. The fixture has only absolute paths with a drive or a UNC share: for anything else
   Node consults the process's directory. Say in the notes which paths the callers can pass
   (`wslpath -w` output and Windows absolute paths) and that relative ones are not checked.
10. **Texts of failures** (`runner.ts`: `new Error(error.message, { cause: error })` with `code`, `stdout`,
    `stderr`). Node:
    - a command that is missing, not executable or a directory: message `spawn <command> ENOENT` /
      `spawn <command> EACCES`, `code` the same word, `stdout` and `stderr` empty strings;
    - any other failure to start (for example `ENOTDIR` for `/file/x`): `execFile` throws at once
      `spawn ENOTDIR` — no command in the text — with `code: 'ENOTDIR'`, and since the throw happens inside
      the source's promise executor, the caller gets that error as it is, without `stdout`/`stderr`;
    - exit status 3: message `Command failed: /bin/sh -c echo out; echo err >&2; exit 3\nerr\n` (command and
      arguments joined by spaces, a line break, then stderr), `code: 3`;
    - killed by a signal: `code: null`, `signal: 'SIGTERM'`, message `Command failed: <command line>\n`.
    The port writes `spawn X failed` for everything but a missing command and numeric strings for other
    errors (`runner.rs:117-124`, `types.rs`). Build these with `hoh_error::node_spawn_error_message` and
    `node_error_code`; the fixture `crates/hoh-error/tests/fixtures/node_system_errors.json`, key
    `spawnMessages`, has the exact texts. Reading a desktop file that vanished
    (`file-applications-linux.ts:58`) fails in Node with `ENOENT: no such file or directory, open '<path>'`:
    `hoh_error::node_fs_error_message(&error, "open", Some(path), None)`, not the `Display` of `io::Error`.
    Tests: a non-executable file, a path under a file, exit status 3, a vanished desktop file — exact texts.
    In the notes, as deliberate differences: a file without an interpreter line is run through `/bin/sh` by
    Node (`execvp`) and refused by Rust's `Command` (`ENOEXEC`) — it matters only for a `BROWSER` that is such
    a script; standard input of the child is closed in the port and an open pipe that is never written in
    Node.

## C. Tests as strong as the source's

11. `path_opener.rs` tests: `falls_back_to_the_default_application…` asserts only `calls()[1]` where the
    source asserts the whole list of calls, the `defaults read` call included; `honors_browser_on_linux…`
    uses `BROWSER=""` instead of an unset variable and asserts the command without its arguments
    (`['/w/page.html']`); `preserves_a_non_delegate_explorer_failure` checks only the code where the source
    checks that the very same error comes back. Assert what the source asserts.
12. `uses_the_native_runner_for_a_file_manager_handoff…`, `keeps_explorer_windows_visible…` and
    `runs_the_command_adapter_without_a_shell…` run on mocks and assert fields the test built itself. The
    source verifies the adapter over `execFile`. Run the real runner on real small commands (`/bin/sh -c`):
    arguments reach the child unsplit and without a shell (an argument with spaces, quotes and `$`), output
    is captured as UTF-8, the abort signal ends a sleeping child and returns the signal's reason.
13. The WSL and file-applications tests dropped the source's assertion that the signal passed to the runner
    is the caller's own — restore it with the shared signal's identity. In `file_applications.rs` tests
    `copy()` always writes explicit `null` for `bundle` and `version`, so the source's cases where the field
    is absent (`script-a`, `V`, `N`) never run: build them as the source does.
    `PathOpenerInternals::default().run.is_none()` proves nothing — remove or replace.

## D. Notes

14. `PORT_NOTES.md`: "none omitted" is true by count (38 + 13 + 4 + 2 + 4, two skipped by platform); after
    this round say which assertions, if any, are still weaker than the source's and why.

## Done when

`cargo test -p hoh-native-command`, `cargo clippy -p hoh-native-command --all-targets -- -D warnings` and
`cargo fmt -p hoh-native-command -- --check` pass, including the test over all 136 lines of the fixture.
For items 5–10 write the test first and see it fail. If you disagree with an item, leave the code and say
why with the source's `file:line`.

Work until it is done. Your last message is the report: the result line of each command, one line per
item, anything that differs from this task. At most 350 words.
