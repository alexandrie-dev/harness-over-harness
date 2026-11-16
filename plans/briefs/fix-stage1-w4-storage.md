# Fix round for `hoh-storage` after an independent review

Only `crates/hoh-storage`; `hoh-credentials` still waits for the kernel. Same boundaries and commands as
`plans/briefs/stage1-writer.md`; environment
`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w4`.

## 1. `StorageError` goes back to the crate's own type — the lead's instruction last round was wrong

The source declares `export class StorageError extends Error` (`src/error.ts:21`): a plain `Error` that
happens to carry a `code`. Your first delivery had it right (a `thiserror` struct with code, message and
cause). The rule you asked about was ambiguous and the lead resolved it the wrong way; it is now rewritten —
re-read `docs/porting/PORTING.md` section 7, first two bullets: the base class in the source decides, and a
class that extends plain `Error` is the crate's own error type even when it has a `code`. The reason is
concrete: generic handlers of the source tell `instanceof HarnessError` from everything else
(`packages/core/tools/src/pipeline.ts:705` gives a model `{ name, code }` only for a `HarnessError`), so a
`StorageError` built as a `HarnessError` would be reported differently from the source.

Restore the own type (name `StorageError`, `StorageErrorCode` with `as_str()`, message, cause). `hoh-error`
stays a dependency only if something else still needs it. Add a test that a boxed `StorageError` is not
recognised by `HarnessError::of` and is found by `downcast_ref::<StorageError>()`. Correct the sentence in
`PORT_NOTES.md` that cites section 7 for the opposite.

## 2. The shared backend contract lost what four scenarios prove

`tests/contract.rs`, `commits_the_current_stamp_with_its_data` (four operations): the source's preamble
(`tests/contract.ts`, the scenarios "commits the current stamp …") makes a write fail, closes the writer,
reopens the unit with the old descriptor and checks that it still opens and still holds the old data — a
failed write at the new version did not restamp the unit. The port dropped the whole preamble because the
source triggers the failure with a throwing `toJSON`, which a `serde_json::Value` cannot do. The property
does not depend on that trigger. Keep it with a write the backend must reject for a reason that exists in
Rust (the source's own rejections: a table the descriptor does not declare, a key that is unsafe for a
per-record layout — choose what every backend of the contract must reject, and say which), then the same
reopen with the old descriptor and the same assertions.

The fifth scenario, "restores the open snapshot after serialization failure", was omitted whole. Port it
with the same substitute trigger: after the rejected write `load_all` is unchanged, a retry on the same unit
succeeds, and the state after reopening is the retried one.

In `PORT_NOTES.md` say exactly what the substitute proves and what it cannot (a failure in the middle of
serialisation has no counterpart while inputs are `Value`).

## 3. One test per scenario

The source has one `it` per scenario; `run_kv_backend_contract` runs all of them inside one test, so the
first failure hides the rest, and a backend crate that includes the file with `#[path]` also inherits this
file's two self-tests. Give backend crates a way to get one `#[tokio::test]` per scenario, named as the
scenario (a `macro_rules!` that takes the harness constructor is enough), keep the scenario functions
public, and move the two self-tests where an including crate does not pick them up. Show in the notes the
three lines a backend crate writes.

## 4. A note for backend writers

`KvSnapshot` keeps tables and records in insertion order. In the source `loadAll` returns plain objects,
and a JavaScript object lists integer-like keys first, in ascending order (`"2"`, `"10"`, then `"b"`). A
backend whose file or listing order is visible must produce that order (`hoh_util_values::js_json_stringify`
already writes objects so). One paragraph in `PORT_NOTES.md`, section for consumers; no code.

## Done when

`cargo test -p hoh-storage`, `cargo clippy -p hoh-storage --all-targets -- -D warnings` and
`cargo fmt -p hoh-storage -- --check` pass.

Your last message is the report: the result line of each command, one line per item, anything you decided
differently and why. At most 200 words.
