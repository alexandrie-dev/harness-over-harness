You only read. Do not run builds, tests, servers, browsers or any command that writes; do not edit or create files. Write in Russian.

Project HoH, working tree /home/save/Alexandrie/HarnessOverHarness (uncommitted vs HEAD). Subject: new engine notification
`session.assistant-stream` by writer x7 (task: /mnt/toshiba/alexandrie/hoh/logs/lead1118/x7-assistant-stream.txt; report:
/mnt/toshiba/alexandrie/hoh/logs/x7.astream.report.md). Areas: crates/hoh-protocol (registry, notification type, fixture, schema),
crates/hoh-sdk-jsonrpc-server (subscription to the kernel event `agent/assistant-stream` and forwarding), crates/hoh-agent-loop if touched,
protocol/schema/*, protocol/core.ts, ui/src/protocol.ts (generated). QA finding A-02: /mnt/toshiba/alexandrie/hoh/logs/qa-window-chat.A.run2.report.md.
CEO approved this as a deliberate difference from dsh; PORT_NOTES must record it.

Check:
1. Frames forwarded only for the session they belong to (no cross-session leakage), only while the session is open; subscription
   cleaned up on session close/engine shutdown (no leaks, no frames after close).
2. Order relative to `session.event`: chunk frames before the final assistant/message event; `end` after it; on session/cancel the
   already-sent chunks stay and `end.outcome` is "abandoned"; no duplicate or lost chunk under backpressure (how is the channel
   bounded? can a slow client block the agent loop or drop frames silently?).
3. JSON field names/types match the window consumer (ui/src/screens/chat/projection.mjs liveFrame and controller.mjs):
   frame.type start/chunk/end, attemptId, revision, index, time, chunk.type "text-delta", chunk.text, outcome.
4. Protocol validation accepts every legitimate frame and rejects malformed ones; schema/TS regenerated, not hand-written; the field
   `details` on HostApprovalRequest added by another writer is preserved.
5. Secrets: assistant text is model output, but verify no other data (e.g. tool args, keys) is forwarded through this channel.
6. PORT_NOTES record the difference from dsh. Tests prove what is claimed and would fail on the old code.
Note: 39 tests of `hoh` could not run in the writer's sandbox (EPERM on bind) — the lead runs them separately; not a finding.
Each finding: scenario, deviation, evidence file:line, level (blocker / error / note). Taste is not a finding.
The LAST line must be exactly 'ВЕРДИКТ: ЧИСТО' or 'ВЕРДИКТ: ЗАМЕЧАНИЯ'.
