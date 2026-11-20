# Research: package sets for the early milestone and for the founder's example

Read-only: no builds, runs or edits. Your report (final message) is the content of `research/milestones.md`.
Russian, at most 1 500 words plus tables. Cite file:line; mark guesses «не проверено».

HoH ports DeepSeek Harness (dsh, TypeScript; read-only clone `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness`,
`P/` = `packages/`) to Rust. Inventory and status: `docs/porting/MAP.tsv` (column 8). Previous analysis of the
full dsh profile: `research/first-scenarios.md` (read it first; it ranks scenarios and gives a critical path that
assumes the full standard profile). The CEO set the order of goals:

1. **Early milestone:** our `hoh` binary on **our own minimal profile** (not dsh's standard profile) holds a session
   with a persisted log, and in one headless run reads a file, writes a file and runs `bash`, with the model replayed
   from a recording (`hoh-llm-replay`, StreamChunk level). Proof: our tests, not byte comparison with dsh.
2. **The founder's example from the window:** `docs/window.md` section 3 (steps 1–13; Russian) on a local
   OpenAI-compatible Qwen model: a builder employee asks questions, fetches open texts from the web, writes and
   runs a small program in a sandbox with network allow-list, saves a task and a "virtual psychoanalyst" employee,
   the user runs it from the window and sees the result. New HoH parts are listed in `docs/window.md` §4 and §6
   and `research/v1-engine-scope.codex.md`.
3. Byte comparison with dsh on 242 scenarios — later.

Answer:
A. **Minimal set for goal 1.** The smallest set of dsh packages (as `hoh-*` crates) whose Rust ports make goal 1
   work, following real imports (not optional peers) — for each: why it is needed, source lines, status. What can
   be left out of a package for goal 1 (e.g. compression, Windows, providers not loaded). What our minimal profile
   must contain (plugins and config) — sketch it. Which packages in the full critical path of `first-scenarios.md`
   are not needed for goal 1.
B. **Set for goal 2** on top of A: dsh packages needed by the example's steps (web fetch, sandbox with network
   allow-list, questions to the user, approvals, presenting files, skills?) plus the new HoH crates. For each of the 13
   links of the critical path in `first-scenarios.md` (session → session-persistence → workspace → agent → commands →
   compaction → compaction-image-offload → session-format-catalog → session-query → subagent → schedule →
   sdk-jsonrpc-server → apps/hoh): needed for goal 2 or only for dsh scenarios, and why.
C. **Does goal 2 require the full dsh byte comparison?** Answer directly, with reasons.
D. **Order:** a dependency-ordered writer queue for goal 1, then goal 2; mark what is already `done`, `review` or
   `writing:`. Total source lines remaining for each goal.
E. **dsh scenarios that could run early** without the full standard profile (if any scenario's profile is small
   enough, or with a documented, comparison-neutral reduction) — or say there are none and why.
