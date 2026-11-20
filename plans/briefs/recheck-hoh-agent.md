# Closure check: hoh-agent fix round (+ additive hoh-kernel, hoh-user-approval)

You only read. Do not run builds, tests, servers or anything that writes; do not edit files. The lead runs the tests.
Original review: `/mnt/toshiba/alexandrie/hoh/logs/rv-agent.full.md`; fix brief: `plans/briefs/fix-hoh-agent.md`.
Writer s16 reports items 1, 2, 4–8 closed. Changed files: crates/hoh-agent/{src/lib.rs, tests/agent.rs,
tests/agent_initiator.rs, PORT_NOTES.md}; crates/hoh-kernel/{src/dispose_capture.rs (new), src/fiber.rs, src/lib.rs,
tests/dispose_capture.rs (new), PORT_NOTES.md}; crates/hoh-user-approval/{src/types.rs, tests/approval.rs}.

For each review item: closed / partly / not closed, file:line, proving test, would it fail on the old code. Special attention:
- item 1 (P1 self-deadlock when unloading from inside a boundary): the new kernel mechanism `Context::capture_on_dispose`,
  `Fiber::dispose()` capturing caller task-locals, `DISPOSE_CAPTURES` carried by `spawn_for`, `dispose_captures::<T>()`.
  Is the kernel change strictly additive (existing behaviour and ordering unchanged)? Paths not covered: `Kernel::shutdown()`,
  `Disposer::dispose()` — can a boundary still self-deadlock through them in the agent loop? Any leak of captured values
  (Arc kept after dispose), cross-kernel visibility, or captures reaching unrelated fibers?
- item 2 announce/detach atomicity: lock order (registry → entry) consistent everywhere, no new deadlock; the race test is
  probabilistic — is the guarantee also argued by construction?
- item 7 modelSelection schema vs `model-selection.ts:63,76`; the writer notes the same `Option` gap in `turnBoundary` —
  is that observable?
Report any new defect. ≤600 words, Russian.
