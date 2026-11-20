# Review: hoh-subprocess-local against its TypeScript source

You only read. Do not run builds, tests, servers, browsers or any command that writes; do not edit files.
The lead already ran the full suite outside the sandbox: 231 passed / 0 failed (`/mnt/toshiba/alexandrie/hoh/logs/w5.lead-test.txt`);
clippy -D warnings and fmt --check are clean (w5 report `/mnt/toshiba/alexandrie/hoh/logs/w5.subproc6.report.md`).

Read `docs/porting/PORTING.md` (§14 checklist). Source: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/subprocess/subprocess-local`
(src/, tests/, locale). Port: `crates/hoh-subprocess-local` (src/, tests/, PORT_NOTES.md — read its «Отличия от образца» first;
declared divergences are not findings unless the note is wrong or the divergence is observable and unjustified).

Priority (this crate runs every shell command of the agent in the desktop milestone):
1. `spawn.rs` / `managed_owner.rs` / `control_spawn.rs`: exit result vs range exit, TERM → grace → KILL timing, abort,
   cleanup after both results, no leaked processes or fds on cancellation paths.
2. `output.rs`: byte tail, offsets, UTF-8 replacement at chunk boundaries, spill file mode/limits, error texts.
3. `spawn_runner.rs` / `runner_protocol.rs` / `bin.rs`: JSON request strictness, env/cwd/argv passing, ENOEXEC/PATH,
   fd 7 control channel, helper-missing fallback (PORT_NOTES §3 — check the SID weakening is the only difference).
4. `linux_scope*.rs`, `process_inspector.rs`, `terminal.rs`, `shell_activity.rs`: sample against the TS, error texts.
Locale files byte-identical. Compare `spawn.spec.ts` and `output.spec.ts` with their Rust tests in full: every TS case
present or listed as deferred in PORT_NOTES.

Triage: type-system divergences unobservable → note; bug = accepted input with different result/order/error/text, or a
resource leak. Output: findings most severe first: kind, Rust file:line, TS file:line, input with both outputs,
confirming test to add. ≤700 words. Russian.
