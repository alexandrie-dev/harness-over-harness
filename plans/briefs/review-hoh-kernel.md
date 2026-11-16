# Independent review: `hoh-kernel` and `hoh-kernel-macros`

You are reviewing two Rust crates in `/home/save/Alexandrie/HarnessOverHarness` (Rust 1.97.1, edition 2024). A
TypeScript agent harness is being ported to Rust; these crates are its kernel (a port of Cordis: plugin fibers,
services, events, effects, configuration, logger), on which about 190 further crates will be built. An error here
is multiplied by everything above it, so the review must be thorough.

This is a read-only review. Do not create, edit or delete any file. Do not run builds, tests, servers or any
program other than read-only shell commands (`cat`, `grep`, `sed -n`, `wc`, `ls`, `git log`, `git show`). Read only
inside this repository, `/mnt/toshiba/alexandrie/hoh/refs` (reference sources) and `~/.cargo/registry/src`
(sources of the Rust dependencies). Another reviewer is working independently; you will not see their answer.

## What to read

- `docs/kernel.md` — the specification the writer worked from: section 2 is the public API, the numbered rules
  `R1`…`R62` are the behaviour, section 8 is the test plan (which source test files are ported and which are not),
  section 10 holds the lead's decisions. `docs/porting/PORTING.md` — the project's porting rules.
- `crates/hoh-kernel` (`src/` about 3,900 lines, `tests/` about 3,600 lines) and `crates/hoh-kernel-macros`, with
  their `PORT_NOTES.md`. The kernel's notes list twelve deviations from the specification ("Отличия от
  спецификации") and the source cases ported and not ported, file by file.
- The source: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/vendor/{cordis,timer,logger-console,schemastery}`
  and the harness's own kernel tests named in section 8.2 of the specification; upstream Cordis with its tests
  is at `/mnt/toshiba/alexandrie/hoh/refs/cordis`.

The lead has verified only that the crates build and that 166 tests pass under clippy with `-D warnings`.
Whether the code is right has not been checked.

## What to look for, most important first

1. **Lifecycle and concurrency** (`src/fiber.rs`, `src/effects.rs`, `src/context.rs`). The actor and its command
   order, epochs, superseded activations, disposal order (dependents before their provider), waiting on oneself.
   Look for: a lock held across an `.await`; lock-order inversions; a `std::sync` lock that a panic can poison
   and what happens to the kernel afterwards; lost wake-ups; futures that are not cancel-safe where a caller can
   drop them; tasks that outlive their fiber; queues that grow without bound; a stale completion that publishes
   after a newer one; an activation that runs with a configuration other than the latest.
2. **Rules against code.** Go through `R1`…`R62` and name every rule that is not implemented, or is implemented
   differently, beyond the twelve listed deviations.
3. **The twelve listed deviations.** For each, check against the source and its tests whether the stated reason
   holds and whether the chosen behaviour is safe. In particular: number 4 (an ordinary `emit` lets a listener's
   panic unwind into the caller — can that leave kernel state inconsistent?), number 5 (update handlers of a
   retained instance may complete concurrently — can an outdated one publish?), number 6 (a provider reads its own
   record while loading), number 7 and 8 (added public items — do they let a caller break an invariant?).
4. **Tests.** Compare ported cases with their source cases, in full for `fiber.spec.ts`, `update.spec.ts`,
   `service-dispose.spec.ts` and `events.spec.ts`, by sample for the rest. Report an assertion that was dropped or
   weakened, a test that would pass if the feature were removed, a case the notes claim that does not exist, and
   paused-time tests that never advance time past what they wait for.
5. **Configuration checker** (`src/config.rs`). Which JSON Schema keywords it accepts against what `schemars`
   1.2.3 produces for ordinary types (`Option`, enums of each representation, nested structs, maps, `$ref` and
   `$defs`, `oneOf`/`anyOf`, integer formats and bounds); error paths and message texts against the source;
   defaults; comparison that ignores volatile fields.
6. **Porting rules.** `unwrap`/`expect`/`panic!` outside tests without a proven invariant; global state;
   recursion over input of unbounded depth; `as` casts applied to JSON numbers; JSON text produced by
   `serde_json::to_string` where a model, a log or a file sees it. `src/config.rs:129` has its own `js_number`:
   name inputs for which it differs from JavaScript's `String(number)`.
7. **Macros.** What `#[plugin]` and the derives generate, hygiene, error spans, what the compile-fail tests cover.
8. **Public API** against section 2 of the specification: every signature that differs.

Statements about another library's behaviour (tokio, tokio-util, schemars, serde_json) must come from its source
in `~/.cargo/registry/src`; if you could not check one, mark it "unsure".

## Report

Numbered findings, most serious first. Each: kind (`bug`, `rule violation`, `hazard`, `missing test`, `false
note`, `note`), `path:line`, a concrete scenario (inputs or order of events, what the code does, what it should
do), the source or rule it rests on, and a test that would expose it. No style remarks, no praise. Then two
lists: "Checked with no findings" and "Not checked". At most 1,600 words.
