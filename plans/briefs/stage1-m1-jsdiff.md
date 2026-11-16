# Writer m1: port `structuredPatch` of jsdiff 9.0.0 into the crate `hoh-jsdiff`

You are writing one small Rust crate in the workspace `/home/save/Alexandrie/HarnessOverHarness`
(Rust 1.97.1, edition 2024). A TypeScript agent harness is being ported to Rust. Two of its packages call
`structuredPatch` from the npm library `diff` (jsdiff) 9.0.0, and the hunks it returns are stored in session logs
and shown to the model, so the Rust port must produce exactly the same hunks for every input.

## Read first

- `docs/porting/PORTING.md` — the project's rules. Sections 1, 4, 6, 7, 10, 12 apply (port behaviour, not text; no
  `unsafe`; no global state; no `println!`; no `unwrap`, `expect` or `panic!` outside tests except for a proven
  invariant with a one-line comment; the format of `PORT_NOTES.md`).
- The source, read-only, never run or build anything there: `/mnt/toshiba/alexandrie/hoh/refs/jsdiff` (tag v9.0.0).
  What you port: `src/patch/create.ts` (`structuredPatch`, `formatPatch`, `createTwoFilesPatch`, `createPatch`),
  `src/diff/line.ts` (`diffLines` and its tokenizer), `src/diff/base.ts` (the diff algorithm with its `timeout` and
  `maxEditLength` limits), and what they use from `src/util/`.
- How the harness calls it (for context only):
  `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/fs/tool-fs/src/diff.ts` and
  `packages/deliverables/workspace-changes/src/compare.ts`: `structuredPatch('', '', before, after, undefined,
  undefined, { context })` and the same with `timeout`, where an `undefined` result means the limit was hit.

## What to build, in `crates/hoh-jsdiff` (the crate exists with a manifest and an empty `lib.rs`)

```rust
pub struct StructuredPatch { pub old_file_name: String, pub new_file_name: String,
    pub old_header: Option<String>, pub new_header: Option<String>, pub hunks: Vec<Hunk> }
pub struct Hunk { pub old_start: usize, pub old_lines: usize, pub new_start: usize, pub new_lines: usize,
    pub lines: Vec<String> }
pub struct PatchOptions { pub context: usize, pub timeout_ms: Option<u64>, pub max_edit_length: Option<usize> }
// `Default`: context 4, no limits, as in the source.

/// `structuredPatch(..)` without a callback. `None` when a limit was hit (the source returns `undefined`).
pub fn structured_patch(old_file_name: &str, new_file_name: &str, old_str: &str, new_str: &str,
    old_header: Option<&str>, new_header: Option<&str>, options: &PatchOptions) -> Option<StructuredPatch>;
pub fn format_patch(patch: &StructuredPatch) -> String;                 // the default header form only
pub fn create_two_files_patch(/* as the source, with &PatchOptions */) -> Option<String>;
pub fn create_patch(/* as the source, with &PatchOptions */) -> Option<String>;
/// `diffLines(old, new)` with the same limits: the change objects in order.
pub fn diff_lines(old_str: &str, new_str: &str, options: &PatchOptions) -> Option<Vec<Change>>;
pub struct Change { pub value: String, pub added: bool, pub removed: bool, pub count: usize }
```

Not ported, with the reason written in `PORT_NOTES.md`: the callback (asynchronous) forms, `ignoreWhitespace`,
`stripTrailingCr`, `newlineIsToken`, `ignoreNewlineAtEof`, header options, and every other diff kind (words,
characters, sentences, CSS, JSON, arrays), patch parsing, applying and reversing. The harness uses none of them.

The essential requirement is identical output, which means the same edit path as jsdiff, not merely a shortest
one: among several shortest edit scripts jsdiff picks one by the order in which it extends diagonals and by how it
breaks ties, and the hunks depend on that choice. Port the algorithm of `base.ts` step for step (the diagonal
bookkeeping, `minDiagonalToConsider` and `maxDiagonalToConsider`, the preference between an insertion and a
removal, how equal runs are followed, how components are built and reversed). Do not replace it with another
diff algorithm or a library. The line tokenizer must split exactly as the source does, including a final line
without a newline, `\r\n`, empty input, and the "\ No newline at end of file" lines in hunks.

`timeout_ms` uses elapsed wall-clock time as the source uses `Date.now()`; `max_edit_length` is the deterministic
limit, use it in tests.

## Tests

Port the source's own tests as the oracle, keeping the case names (snake_case):

- `test/patch/create.js`: every case of `structuredPatch`, `createPatch`, `createTwoFilesPatch` and `formatPatch`
  that does not need a feature listed above as not ported. The expected patch texts in those cases are the proof
  that the hunks are right; copy them exactly.
- `test/diff/line.js`: every `diffLines` case that does not need a feature not ported.
- List in `PORT_NOTES.md`, per source test file, the cases ported and the cases not ported with the reason.

Add your own tests for: both texts empty; identical texts (no hunks); one side empty; only the final newline
differs, in both directions; `\r\n` texts; a change at the first and at the last line; two changes closer than
and farther apart than twice the context (one hunk versus two); context 0; `max_edit_length` hit (`None`); a
4,000-line text with scattered changes (it must finish quickly); non-ASCII text.

## Boundaries

- You own only `crates/hoh-jsdiff`. Do not create, edit or delete anything outside it: not the root `Cargo.toml`,
  not other crates, not documents. Other writers are working in this repository right now. No dependencies: the
  standard library is enough.
- Copy the source's `LICENSE` (BSD-3-Clause) into the crate unchanged and say so in your report.
- Allowed commands, always with the environment
  `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-m1`:
  `cargo fmt -p hoh-jsdiff`, `cargo build -p hoh-jsdiff`, `cargo clippy -p hoh-jsdiff --all-targets -- -D warnings`,
  `cargo test -p hoh-jsdiff`. Read-only shell commands are fine. Nothing else: no network, no new tools or
  packages, no Node, no servers, no git command that changes state.

## Done when

`cargo test -p hoh-jsdiff` passes with the cases above; `cargo clippy -p hoh-jsdiff --all-targets -- -D warnings`
and `cargo fmt -p hoh-jsdiff -- --check` are clean; `crates/hoh-jsdiff/PORT_NOTES.md` is written in the format of
PORTING.md section 12 (in Russian, like the other crates' notes).

Work until it is done; do not stop with a partial result and a plan. Your last message is the report: the test
command and its result line, the number of source cases ported and not ported, anything that differs from this
task, open questions. At most 300 words.
