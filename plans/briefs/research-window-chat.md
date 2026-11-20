# Research: what the "window-chat" milestone needs on top of the early milestone A

You only read. Do not run builds, tests, servers or anything that writes; do not edit files except the one output
file `research/window-chat.md` (Russian, plain words). Repository: /home/save/Alexandrie/HarnessOverHarness.

## The milestone (CEO 18.11)
The founder opens the desktop window (Tauri, T3 Code look, `docs/window.md`), writes to the Assistant; it answers through a
local model over the OpenAI protocol and, in front of him, reads a file, writes a file and runs a command; the feed shows
the work steps. No employees, tasks, buttons or schedule. Engine = our Rust port (early milestone A: 50 crates in
`research/milestones.md`, section A). Window ↔ engine protocol: `docs/window.md` §4, `crates/hoh-protocol`,
`crates/hoh-window-stub`, `apps/hoh-desktop`, `ui/`.

## Question
List every crate / new piece needed **on top of A** for this milestone, with evidence (file:line in the dsh source
`/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/...`, or in our docs). The CEO's starting list, to confirm or
correct: sdk-protocol, sdk-jsonrpc-server, the local OpenAI model adapter (`crates/hoh-llm-openai`, partly written),
permission approvals (user-approval; host approvals with MAC per `docs/window.md` §5), `apps/hoh-desktop` (bridge, method
filter, approval window — window package 4), chat screen (window package 5). Decide for each candidate whether it is
**needed** for this milestone (value import / mandatory inject / the protocol path actually used) or **not needed**
(e.g. session-query, commands, session-title, user-questions, sandbox-*, settings — say why). Check what
`sdk-jsonrpc-server` imports for real and which of its dependencies are already in A or done (`docs/porting/MAP.tsv` col 8).
Also: does the engine need `apps/hoh` (A7 one-shot driver) or a separate long-running server entry for the window?

## Output
`research/window-chat.md`: a table (crate/new piece; source lines; MAP status; needed/not; evidence), the minimal list,
line counts (src) for ports, and what is new code with no source. At most 900 words. Say what you did not check.
