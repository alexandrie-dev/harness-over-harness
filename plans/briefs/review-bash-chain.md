# Review: hoh-bash-local, hoh-shell-env, hoh-tool-bash against their TypeScript sources

You only read. Do not run builds, tests, servers or any command that writes; do not edit files.
Read `docs/porting/PORTING.md` (§14 checklist). Sources under `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/`
(paths in `docs/porting/MAP.tsv`, column 3). Ports: `crates/hoh-bash-local`, `crates/hoh-shell-env`, `crates/hoh-tool-bash`
(src/, tests/, PORT_NOTES.md with shims and unported cases).
These crates run the agent's shell commands in the window-chat milestone — focus on behaviour a user sees and on safety:
command construction and quoting, cwd, environment passed to the child (what leaks from the parent: keys, tokens),
output limits and truncation, exit code / signal reporting, timeouts and cancellation, rendering of the result text.
Check the writer's claims: `ShellExecutor::resolve` panics + `catch_unwind` in tool-bash (is any panic path reachable from
model input?); `present_bash_result` reads `result.value` in TS — confirm the promotion flag is dead in the source;
`DSH_INSTALL_DIR` from the executable directory; unported cases (jobs, approval, sandbox-policy, integration.spec.ts) —
which of them hide behaviour the milestone needs. Sample two test files in full against their TS specs.
Triage: type-system divergences unobservable → note; bug = accepted input with different result/order/error/text.
Output: findings most severe first: kind, Rust file:line, TS file:line, input with both outputs, confirming test. ≤800 words.
Answer in Russian.
