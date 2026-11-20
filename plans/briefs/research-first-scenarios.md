# Research: the shortest path to the first dsh scenarios passing through our Rust `hoh`

HoH ports DeepSeek Harness (dsh, TypeScript) to Rust package by package. The first milestone the founder will see:
**our built `hoh` binary holds a session, runs tools and passes the first recorded dsh scenarios** through
`crates/hoh-conformance` (read its `README.md`: how a scenario is run, which suites, what is compared). Read-only:
do not build, run or edit anything; your report (final message) is the content of `research/first-scenarios.md`.

Inputs:
- Source clone (read-only): `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness`; scenarios under `snapshots/`
  (`snapshot.yml`, profiles/configs each scenario boots, the recorded logs).
- Our inventory: `docs/porting/MAP.tsv` (status column 8: `done`, `review`, `writing:<w>`, `todo`), the v1 scope
  `research/v1-engine-scope.codex.md` and its wave table, `research/v1-unstarted.tsv`.
- `crates/hoh-conformance/conformance/EXPECTED.tsv` and `RESULTS.tsv` if present.

Answer:
1. **Which scenarios first.** Rank the 242 scenarios by how few not-yet-done crates they need (use the profile and
   plugins each scenario actually loads, and the tools its recording calls). Name the cheapest 5–10, preferring
   `session/text-turn`, `sdk/text-turn` and scenarios with one tool call (read/write/bash). For each: the exact set
   of crates it needs that are not `done` yet, with source lines.
2. **The critical path.** The union of those sets, ordered by dependencies into waves; mark which crates are
   already being written (`writing:`) or in review. What is the longest chain (crate → crate) and its total source
   lines. What can be cut from these packages for the first scenarios without breaking their byte-for-byte
   comparison (e.g. a provider not loaded in that profile).
3. **What is not needed for the founder's example** (`docs/window.md` section 3: a Telegram-analysis tool written by
   the model and run in a sandbox, web fetch of open books, a "virtual psychoanalyst" employee from the window,
   local Qwen model) and only matters for full compatibility: list the v1 rows that can go last.
4. A dependency-ordered queue for writers: first the critical path, then the founder's example, then the rest.

Cite file:line for every claim; mark guesses «не проверено». Russian, at most 1 500 words plus tables.
