# Fix round for writer c1 after the independent review

Same boundaries and commands as in your first task (`CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-c1`).
The lead has triaged the review; do exactly what is listed here. `docs/porting/PORTING.md` sections 4 and 7 were
updated with the decisions below — read those two sections again first.

## 1. `hoh-error` — redesign (decision of the lead)

Reason: about 44 source files subclass `HarnessError`, and the source recognises a coded error dynamically
(`error instanceof HarnessError ? { name: error.name, code: error.code } : undefined` in `core/tools/src/pipeline.ts:705`,
the same in `llm/llm/src/adapter-failure.ts:107`, `subprocess/subprocess/src/error.ts:71`). With per-crate wrapper types
behind `Box<dyn Error>` that check cannot be written. So there is one concrete type and no wrappers.

Public API of the crate, complete (nothing else is public):

```rust
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

#[derive(Debug, Clone)]
pub struct HarnessError { /* private fields */ }

impl HarnessError {
    /// `new HarnessError(message, code)`; name is "HarnessError".
    pub fn new(message: impl Into<String>, code: impl Into<String>) -> Self;
    /// A subclass instance: `name` is the source class name.
    pub fn named(name: &'static str, message: impl Into<String>, code: impl Into<String>) -> Self;
    pub fn with_cause(self, cause: impl Into<BoxError>) -> Self;       // accepts a concrete error and an already boxed one
    pub fn with_detail<T: std::any::Any + Send + Sync>(self, detail: T) -> Self;  // extra fields of a subclass
    pub fn name(&self) -> &str;
    pub fn code(&self) -> &str;
    pub fn message(&self) -> &str;
    pub fn cause(&self) -> Option<&(dyn std::error::Error + Send + Sync + 'static)>;
    pub fn detail<T: std::any::Any>(&self) -> Option<&T>;
    pub fn is_named(&self, name: &str) -> bool;                         // `instanceof Subclass`
    /// `error instanceof HarnessError` for a dynamic error: a downcast of `error` itself, never of its sources.
    pub fn of<'a>(error: &'a (dyn std::error::Error + 'static)) -> Option<&'a HarnessError>;
}
impl std::fmt::Display for HarnessError   // prints the message only, like `error.message`
impl std::error::Error for HarnessError   // `source()` is the cause
```

`Clone` is required (an error is delivered to several waiters), so the cause and the detail are held in `Arc`.
Remove `CodedError`, `with_name` and the public fields. Tests: port the source cases; the cause test must recover the
concrete cause by downcast, not compare strings; add tests for `of` (positive at depth 0; negative for a plain error
whose `source()` is a `HarnessError`; positive through `BoxError`), `is_named`, `detail` (present, absent, wrong type),
`with_cause` given a `BoxError`, and `Clone` keeping cause and detail. PORT_NOTES.md: describe the design and show the
subclass pattern of PORTING.md section 7 with one example.

## 2. `hoh-brand`

- `branded_string!`: add `as_str(&self) -> &str`, `Borrow<str>`, `PartialEq<str>` and `PartialEq<&str>`, so that
  `map.get("x")` and `id == "x"` can be written.
- `branded_number!`: derive `Eq`, `Ord`, `Hash` as well (ids are integers; the macro is for integer representations —
  say so in its doc comment), so that the type can be a map key and can be sorted (`core/session/src/tool-history.ts:10`,
  `seq-ranges.ts:10`).
- Add the module `hoh_brand::ids` holding the ids that the source declares in more than one package, each declared once
  with the macro: find them by grepping the packages being ported for `Branded<'…'>` and for brand declarations, and
  include exactly those brand names that appear in two or more packages (the reviewer found `SessionId`,
  `ApprovalRequestId`, `session-request-id`, `file-upload-receipt-id`, `SessionTitleProviderId`; verify and complete the
  list). List them in PORT_NOTES.md with the packages that use each.
- Tests for each addition, including two modules sharing `ids::SessionId`.

## 3. `hoh-chunked-list`

- Bug: `chunked_list_schema` accepts an array where the source's `z.object` rejects it (`json!([[0]])` returns `Ok`),
  at the top level and at every `previous` level. Reject everything the source rejects; add the tests.
- Depth: a list is one nesting level per chunk, so a long list is a deep `previous` chain. Validation, restore,
  iteration and `Drop` must not recurse over `previous` (write them as loops; give the chain an iterative `Drop`).
  Add a test that builds a list of 200 000 values programmatically, validates, iterates and drops it.
  Parsing such a checkpoint from text is the caller's concern (serde_json refuses nesting deeper than 128 by default):
  state that truthfully in PORT_NOTES.md in place of the current "как и образец".
- Make the parameter types of `iterate_chunked_list` and `append` consistent with each other.

## 4. `hoh-home-paths`

Make these equal to Node's behaviour in the source, each with a test:
- `join('/a', 'b/', '')` gives `/a/b/` (empty segments are ignored when deciding the trailing separator);
- the default homes are normalised like `path.join(homedir(), '.dsh')`: `HOME=/home//u` gives `/home/u/.dsh`;
- `HOME=""`: follow what `os.homedir()` does in Node (it returns the empty string when `HOME` is set and empty; it falls
  back to the passwd entry only when `HOME` is unset) and what the source then does with it;
- `dsh_home_display`: the source compares strings (`index.ts:121`), so `/home/u/.dsh/` with a trailing slash is not the
  default home; compare as the source does;
- `canonicalize_watch_path`: in the source the `opendir` is inside the `try` (`index.ts:47-56`), so a failure there
  climbs one level instead of propagating.
Also add the missing tests: the `env: None` branch (through a child process with a controlled environment, never the
real home), `~/` and `~\` with an empty rest, and `Some("")` as the configured path.

## 5. `hoh-anonymous-user-id`

- The helper test `process_environment_child` must do nothing unless a marker variable set by its parent test is
  present, so that `cargo test -- --ignored` can never write into the real home directory.
- PORT_NOTES.md: say that the function does blocking file I/O under a lock, as the source is synchronous, and that an
  async caller must call it through `spawn_blocking`.

## 6. `hoh-util-crypto`

- `random_uuid()` returns `Uuid`, not `Result`: the operating system's random source failing is not an expected
  failure. Panic with a one-line comment stating the invariant (PORTING.md section 7 allows exactly this case).
- `mints_without_platform_random_uuid` duplicates the first test: replace it with a test of what the source case
  actually establishes (the UUID is built from random bytes with the version and variant bits set), or remove it and
  record why in PORT_NOTES.md.

## 7. `hoh-atomic-write`

Only what is needed to compile and keep behaviour with the new `hoh-error` API: errors become `HarnessError` values
built by a subclass constructor as in PORTING.md section 7 (same names, codes and texts as the source). Do this last.
Another reviewer is reading this crate right now, so do not restructure it.

## Done when

For all eight crates: `cargo test -p <crate>`, `cargo clippy -p <crate> --all-targets -- -D warnings` and
`cargo fmt -p <crate> -- --check` are clean; every PORT_NOTES.md statement the review found wrong or incomplete is
corrected (`hoh-error`, `hoh-brand`, `hoh-chunked-list`, `hoh-home-paths`, `hoh-anonymous-user-id`).

Final report: per crate the test command result line and one line per item above saying what changed; anything unfinished.
