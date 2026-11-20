# Review: hoh-user-approval against its TypeScript source

You only read. Do not run builds, tests, servers or any command that writes; do not edit files.
Read `docs/porting/PORTING.md` (§14 checklist). Source: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/interaction/user-approval`
(src/, tests/, locales). Port: `crates/hoh-user-approval` (src/, tests/, PORT_NOTES.md).
Window-chat milestone: dangerous actions (write, bash) are confirmed by a human — focus on safety: any path where an action
proceeds without an explicit allow (timeouts, cancellation, missing/garbled answer, `policy: null` → `ask` shim, scoped
dispatch, waterfall answer normalised into `ApprovalOutcome` — what unknown values become); audit records; locale texts
(all eight files byte-identical); cancellation ordering. Sample two test files in full against their TS specs.
Triage: type-system divergences unobservable → note; bug = accepted input with different result/order/error/text;
any fail-open is P1.
Output: findings most severe first: kind, Rust file:line, TS file:line, input with both outputs, confirming test. ≤700 words.
Answer in Russian.
