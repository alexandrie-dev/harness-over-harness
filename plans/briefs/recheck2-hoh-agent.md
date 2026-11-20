# Closure check 2: hoh-agent fix2 (+ hoh-kernel dispose captures)

You only read. Do not run builds, tests, servers or anything that writes; do not edit files. The lead runs the tests.
Previous check: `/mnt/toshiba/alexandrie/hoh/logs/rc-agent.report.md` (item 1 partly: 3 P1 alternative unload paths; P2 capture
isolation, synchronous callbacks; P2 turnBoundary). Writer s16 reports all fixed, 408/0. Changed: hoh-kernel
src/dispose_capture.rs, src/dispose_capture_tests.rs (new), src/fiber.rs, src/effects.rs, src/lib.rs, tests/dispose_capture.rs,
PORT_NOTES.md; hoh-agent src/lib.rs, src/projection.rs, src/announce_window_tests.rs (new), tests/agent.rs,
tests/agent_initiator.rs, PORT_NOTES.md.
For each previous finding: closed / not closed, file:line, test, fails on old code? In particular: can any path still
self-deadlock (second dispose, Kernel::shutdown, fiber.disposer().dispose(), dispose from inside a teardown)? Is the "live"
capture set and `next_dispose_captures` wake-up race-free (a value appended after the drain decided to finish)? Isolation
by kernel address — can an address be reused after a kernel drops (stale tag)? Is the kernel still additive for code that
never registers a capture (ordering and timing unchanged)? Is the cfg(test) window test a real proof of atomicity? New defects.
≤500 words, Russian.
