# Closure check 4: hoh-tools fix4 (items 1 and 3)

You only read. Do not run builds, tests, servers or anything that writes; do not edit files. The lead runs the tests.
Previous check: `/mnt/toshiba/alexandrie/hoh/logs/rc-tools4.report.md`. TS source: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/core/tools`.
Writer s7c claims: result fields `pub(crate)` with accessor methods (Frozen removed, no &mut access outside); `CanonicalSeal`
has a `frozen` flag — `marked()` freezes (create_success_result, final with_content), `marked_editable()` only records the
owner (normalized failure, wrapper success, deferred contexts, Block/Accept/value replacement in post-execute), mapped to
pipeline.ts 320, 373, 515–541, 581, 594, 605; post-execute changes go through `PostToolDecision` because `event.result` is an
Arc copy. Item 3: `dispatch_inner` uses hoh-kernel `waterfall_with_start`; the scheduler in batch.rs waits only for that
start signal (plus a safety signal on dispatch completion); timer and yield counting removed; tests with paused clock.
For items 1 and 3: closed / not closed, file:line, test, fails on old code? Check each freeze/editable site against the TS
lines; is the "safety signal on completion" able to let pre(B) overtake in any input where TS would not? Any API break for
other crates beyond the 3 lines in hoh-tool-bash tests? New defects. ≤400 words, Russian.
