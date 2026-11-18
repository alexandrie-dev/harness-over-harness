# Research: which packages of dsh the first version needs

Working directory `/home/save/Alexandrie/HarnessOverHarness`. You only read and you write nothing to disk; your
answer is your printed reply. No builds, no installs, no network.

Read `plans/briefs/v1-requirements.md` first. The engine of HoH is a port of DeepSeek Harness (dsh) to Rust:
`docs/porting/MAP.tsv` lists 205 packages to port by stage (columns: stage, decision, dsh_path, dsh_name,
hoh_crate, src_lines, test_lines, status, note); 33 are done. The source is under
`/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness` (`packages/*`, bundles in `packages/bundle/*/cordis.patch.yml`,
profiles in `packages/util/official-bundles/src/bundles.ts`). Earlier research: `research/deepseek-harness.md`
(a thin composition of 63 packages is proposed there) and `research/task-plugin-design-input.md`.

The founder has now said what the first version must do. Establish from the source, with paths:

1. For each capability, which dsh packages provide it today and what is missing in dsh:
   a. a session with a model (one adapter for an OpenAI-compatible local server), the agent loop, the session
      log, tools for files and shell, web search and fetch, permissions and sandbox, context compaction;
   b. skills: creating one, storing it, running it; what "skill creator" or similar exists;
   c. agents with roles: presets, instructions, sub-agents, agent teams — what a "virtual employee" (a named
      role with rules, skills and its own memory) could be built from;
   d. the user's own commands (slash commands, buttons) defined without code;
   e. schedules and timed or background jobs (`jobs-*`, `goal-*`, timers, anything cron-like);
   f. previews of results for a window (attachments, documents, images, charts), and what the SDK protocol
      (`packages/sdk/*`) sends a client;
   g. the external protocol a window would speak (SDK JSON-RPC), and what in it is code-specific.
2. From that, the smallest set of packages of `MAP.tsv` whose port gives the first version: list them by crate
   name in dependency order (read the real imports in each package's `package.json` and `src`), with source
   line counts, split into waves that can be written in parallel. Mark which of them are already `done` or
   `review`. Give the total of lines against the full 205.
3. The packages of `MAP.tsv` that the first version does not need, in groups, with the reason in a few words
   (they stay in the plan for later; nothing is dropped).
4. What is new, not in dsh at all, and where it would attach: virtual employees, "a task is a plugin" with
   acceptance by the user's "yes", schedules if dsh has none.

Mark every statement you did not verify in the source as "not checked". At most 1800 words.
