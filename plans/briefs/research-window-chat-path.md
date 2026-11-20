# Research: critical path to "window-chat" and what of milestone A can be deferred

You only read. Do not run builds, tests, servers or anything that writes; do not edit files except the one output
file `research/window-chat-path.md` (Russian, plain words). Repository: /home/save/Alexandrie/HarnessOverHarness.
A parallel reviewer writes `research/window-chat.md` (what is needed on top of A) — do not duplicate it; you answer the
questions below.

## Milestone condition (CEO 18.11, founder's words)
The founder starts the HoH desktop app; the window looks per `docs/window.md`; on first start he enters the model address
and API key (any OpenAI-protocol provider: cloud or local Qwen), the key is stored through `hoh-credentials`; he writes to
the Assistant, gets a streamed answer from a real model; the agent runs shell commands and works with files, steps are
visible in the feed, dangerous actions are confirmed.

## Questions
1. Of the 50 crates of milestone A (`research/milestones.md` section A, statuses in `docs/porting/MAP.tsv` col 8), which
   are NOT needed for this milestone if the model is live (not replayed) and new sessions are written only in the current
   journal format? Check concretely: `hoh-llm-replay`; session-format migrations v0-to-v1 … v3-to-v4 and
   `session-format-catalog` (does writing/reading a new V4 journal through session-persistence-jsonl import the
   migration chain by value? cite import lines); `compaction-image-offload`; `hoh-conformance`. For each: needed / can be
   deferred / cannot be deferred, with file:line evidence from `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/...`.
2. The critical path: the dependency chain (longest) from what is accepted now to the milestone, as an ordered list of
   crates/new pieces, each with source lines and current status. Current work: see `plans/session-state.md` (last 60
   lines) for who writes what.
3. How many source lines of remaining work (unaccepted) the deferrable crates represent, so the lead can estimate days saved.

## Output
`research/window-chat-path.md`: answers 1–3 with evidence. At most 900 words. Say what you did not check.
