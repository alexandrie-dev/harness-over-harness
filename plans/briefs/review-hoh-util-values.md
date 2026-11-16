# Review: hoh-util-values against its TypeScript source

You are an independent reviewer in a porting project (DeepSeek Harness, TypeScript → HoH, Rust). You only read. Do not run builds, tests, servers or any command that writes; do not edit anything.

## Read
- `docs/porting/PORTING.md` (in Russian) — the binding porting rules; your checklist is its section 14.
- Source: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/util/values` (`src/`, `tests/`, `README.md`).
- Port: `crates/hoh-util-values` (`src/`, `tests/`, `PORT_NOTES.md`).

## What to check
1. Behavioural equality of `partial_json.rs` with `partial-json.ts`: every branch, boundary case and state transition (append, sealing, `settle`, `refresh`, change tracking, readers), offsets and lengths in UTF-16 units, handling of escapes, surrogate pairs split across fragments, numbers, nesting and invalid input.
2. `is_json_value`, `snapshot_json_value`, `deep_equal_json` against the source functions, including numbers (`-0`, non-finite, integer versus float), key order and duplicate keys.
3. Tests: every case of `tests/partial-json.spec.ts` has a Rust counterpart with assertions that are not weaker. List any case that is missing or weakened.
4. Each entry under "Не перенесено" and "Отличия от образца" in PORT_NOTES.md: is the stated reason true? In particular confirm or refute the claim that `WeakMapWithValues` is used only by browser-client packages (`packages/client`, `packages/experimental`, `apps/web`).
5. Anything in the port that goes beyond the source.

Context that may save you time: the only consumers of the partial-JSON reader in the source tree are browser-client packages, so API ergonomics do not matter here; equality of behaviour does.

## Output
A list of findings, most severe first. For each: kind (`bug`, `rule violation`, `missing test`, `note`), location in Rust (`file:line`), location in TypeScript (`file:line`), the observable divergence with a concrete input and the two outputs, and a test that would confirm it. If you find nothing in a category, say so in one line. Do not suggest improvements that the source does not have. At most 700 words.
