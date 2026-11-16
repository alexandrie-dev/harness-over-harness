# Second fix round for `hoh-native-command`: what an aborted command returns

The reviewer confirmed that all fourteen items of the first fix round are closed. Four things remain; two
of them come from a wrong sentence in the lead's previous brief. Only `crates/hoh-native-command`; same
boundaries, commands and environment as before
(`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w1`).

The facts marked "Node:" were measured by the lead on Node 22.23.1 with
`execFile('/bin/sh', ['-c', script], { encoding: 'utf8', signal })`, aborted after 300 ms.

## 1. An aborted command returns at once, and returns the runner's own error

The previous brief said the runner returns the signal's reason. That is wrong for the runner
(`runNativeCommand`, `runner.ts`); it is right only for the functions that call `signal.throwIfAborted()`
in their `catch` (`runExplorer` and the others in `path-opener.ts`, and wherever else the source does it).

Node, for `trap "" TERM; echo started; sleep 3` (a child that ignores TERM) and equally for a child that
dies on TERM:
- the callback fires about 10 ms after the abort — Node sends SIGTERM once and does not wait for the child
  to exit or for its pipes to close;
- the error `execFile` gives is `AbortError`, message `The operation was aborted`, `code: 'ABORT_ERR'`,
  `cause` is the very reason passed to `abort(reason)` (without a reason: an `AbortError` with the text
  `This operation was aborted`), `stdout: 'started\n'`, `stderr: ''` — what was read so far.

So the source's runner rejects with `new Error('The operation was aborted', { cause: <that AbortError> })`
carrying `code: 'ABORT_ERR'` and the partial `stdout` and `stderr`; its own test asserts exactly the code
(`native-command.spec.ts`, "terminates the child when the signal aborts": `code` is `'ABORT_ERR'`).

The port today (`runner.rs`) returns the bare reason, and after cancelling it waits for the child's exit
and for the end of both pipes, so `sh -c 'trap "" TERM; sleep 60'` hangs the call for a minute.

Decided:
- on cancellation the runner sends SIGTERM once, as Node does, and returns immediately; it does not wait
  for exit or end of output, and it does not send SIGKILL (the child is reaped in the background so that
  no zombie is left; say in the notes that a child ignoring TERM lives on, as in Node);
- it returns the crate's command error with message `The operation was aborted`, code `ABORT_ERR`, the
  output read so far, and the signal's reason reachable as the cause with its identity kept;
- the functions of the source that call `signal.throwIfAborted()` before looking at the error keep
  returning the signal's reason itself — check each call site of the runner in the source for whether it
  does (`path-opener.ts`, `file-applications*.ts`), and do the same.

Tests: the source's case with its own assertion (code `ABORT_ERR`); a child that ignores TERM and prints
before sleeping — the call returns within a second of the abort with the partial output attached and the
reason as the cause (same allocation); an opener function aborted in the same way returns the reason
itself. The other consumers of the runner are `packages/api/workspace-controller`,
`packages/host/directory-picker-native` and `packages/host/open-in-app`: name in the notes what each gets.

## 2. Eight more paths in the Explorer fixture

`tests/fixtures/explorer_target.jsonl` now has 144 lines. The lead added what the reviewer asked about:
`\\?\unc\…` and `\\?\Unc\…` (Node: only the upper-case `UNC` is the prefix; the others give
`file:///unc/server/share/file.txt`), `\\?\c:\…`, a host that needs punycode
(`\\сервер\share\файл.txt` → `file://xn--b1afb6bcb/share/файл.txt`), a host with dots and capitals, `Z:\`,
`c:/`, and `C:\w\a\\..\b`. Make all 144 pass and update the count in the test. For the host: the `url`
crate is in `[workspace.dependencies]` and converts a host as the URL standard says; if it cannot be used
for this without changing other lines, say what is missing.

## 3. The note about a file without an interpreter line

`PORT_NOTES.md` says Rust's `Command` refuses such a file (`ENOEXEC`) where Node runs it through `/bin/sh`.
The Node half is measured (it printed the script's output). The Rust half is a guess: the standard library
may use `posix_spawnp`, whose behaviour here depends on the C library. Write a test that runs a file with
mode 755 and no `#!` line through the real runner and asserts whatever really happens on this system, and
make the note say that (with the C library it was observed on).

## Done when

`cargo test -p hoh-native-command`, `cargo clippy -p hoh-native-command --all-targets -- -D warnings` and
`cargo fmt -p hoh-native-command -- --check` pass. Write the tests of item 1 first and see them fail.

Your last message is the report: the result line of each command, one line per item. At most 150 words.
