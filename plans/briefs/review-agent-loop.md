# Review: hoh-agent-loop against its TypeScript source

You only read. Do not run builds, tests, servers, browsers or any command that writes; do not edit files.
The lead runs the tests (writer s6 reports 135/0 with --include-ignored, 15 test binaries).

Read `docs/porting/PORTING.md` (§14 checklist). Source: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/core/agent-loop`
(src/, tests/). Port: `crates/hoh-agent-loop` (src/, tests/, PORT_NOTES.md — read the table of ported/deferred cases and the
declared differences first). This crate drives every turn of the desktop milestone: user message → model request → streamed
answer → tool calls (bash, files) with approval → results → next step; cancel and resume.

Priority:
1. Turn driver: order of session events (user message, assistant message, tool/call, tool/result, turn end), what goes into
   each model request (system prompt, history, tools; the user message must be present), the `agent_loop_request` marker.
2. Tool calls: batching, ordering, recovery of calls without results after a failure (ToolCallRecovery), errors from the
   executor (plain vs batch), approvals, cancel mid-tool.
3. Cancel and resume: a cancelled turn leaves a consistent session; resume from a persisted JSONL session continues correctly.
4. Locks: any std Mutex/RwLock guard held across `.await` or across a callback that may re-enter (one self-deadlock was just
   fixed in agent.rs:742 — look for others), any blocking call on the runtime thread.
5. Scope/resource lifecycle: teardown order (scope after maintenance work), no leaked tasks after dispose.
6. Deferred cases (`loop.spec` 19 of 66 ported, compaction cases not ported): is each deferral justified, and is any deferred
   case needed for the milestone path above?
Triage: unobservable type differences → note; bug = accepted input with different result/order/error/text, hang, leak.
Output: findings most severe first: kind, Rust file:line, TS file:line, input with both outputs, test to add. ≤800 words, Russian.
