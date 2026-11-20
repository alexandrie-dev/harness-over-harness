# Review: hoh-loader changes since fe1d8c5 and apps/hoh (cmdline + application)

You only read. Do not run builds, tests, servers, browsers or any command that writes; do not edit files.
The lead runs the full tests separately (hoh-loader + hoh).

Task given to the writer: `plans/briefs/stage0-l1-cmdline-app.md` (read it first). Changes to review:
- `git diff fe1d8c5 -- crates/hoh-loader` (14 files: fixes after the part-B review plus the config editor work);
- the new `apps/hoh` (src/, tests/, assets/, PORT_NOTES.md, SOURCE_TEST_CASES.md).
TypeScript source: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness` (the cmdline/app packages named in the brief and in
apps/hoh/PORT_NOTES.md; loader under packages/…/loader). Read `docs/porting/PORTING.md` §14.

Focus: command-line parsing and errors (exact texts, exit codes), profile/overlay resolution order, the config editor
(patch file writing, conflicts, reload with the frozen command-line overlay), boot and shutdown order, anything the app
prints to stdout/stderr. Compare SOURCE_TEST_CASES.md with the Rust tests: every source case present or justified.
Triage: unobservable type differences → note; bug = accepted input with different result/order/error/text, data loss in
config files. Output: findings most severe first: kind, Rust file:line, TS file:line, input with both outputs, test to add.
≤700 words, Russian.
