# Review: the scenario runner hoh-conformance

You are an independent reviewer in a porting project (DeepSeek Harness, TypeScript → HoH, Rust). The code under review was written by a different model (Codex). You only read. Do not run builds, tests, servers or any command that writes; do not edit or create any file. Your answer is your printed reply.

The runner replays the 242 recorded scenarios of dsh through our built binary and decides pass or fail. Its one job is to count honestly: a scenario must never pass when dsh's own harness would fail it, and a correct run must not fail because of the runner.

## Read
- The specification: `crates/hoh-conformance/README.md` sections 1–6 (written from the source; it cites the source files) and the task the writer was given: `plans/briefs/stage0-cf1-conformance-runner.md`.
- Source, read-only, `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness`: `packages/test-support/session-snapshot/src` (session-files, manifest, harness, launcher, normalization and comparison), `snapshots/session/headless.snapshot.ts`, `snapshots/sdk/sdk.snapshot.ts`, `snapshots/acp/*.ts`, `scripts/snapshot-*.ts`, and a few scenario directories under `snapshots/`.
- Port: `crates/hoh-conformance` (`src/`, `tests/`, `PORT_NOTES.md`).

## What to check
1. Normalization (README section 2): is every rule implemented exactly — nothing dsh leaves literal is masked (tool result texts, `usage` numbers, event references), nothing dsh tokenizes is left raw? For each rule: Rust location, source location, same or different.
2. Comparison (section 3): the session log (event types, order, count, field values), `request/header` and the satellites (system prompts, tool schemas compared byte for byte), the working tree, stdout/stderr/exit code for headless, frames for ACP, notifications for SDK. Anything skipped, loosened or compared only partially?
3. Selection of inputs (1.1) and launch (1.2): file-name rules, newest generation per role, patches in argv order per suite, environment variables (each one of the table), the working directory rules (192-character cwd, outside-temp parents, named setups).
4. Pass/fail logic: can any path produce `pass` when a comparison did not run (an empty selection, a missing file read as empty, an unimplemented check, a caught error)? The writer says unimplemented checks fail with `check <name> not implemented`: list them from the code and say whether every scenario that needs one really reaches that failure.
5. The identity and sensitivity tests: does the fake process exercise the same code path as a real binary would, or does it bypass part of the comparison? Are the six mutations detected in the right phase for the right reason?
6. Runner errors against scenario failures: what is counted where; exit codes.
7. `PORT_NOTES.md`: the answers the writer established to README open questions 5 (inherited prefix of a child) and 6 (ACP telemetry) — check them against the source.

## Triage rule
A `bug` is a case where the runner's verdict differs from the verdict of dsh's harness for the same process output, in either direction; say which direction. A `weakening` is a comparison that is looser than dsh's. Statements about libraries or Node made from memory must be checked in the clone or marked "not checked".

## Output
A list of findings, most severe first. For each: kind (`bug`, `weakening`, `missing test`, `note`), location in Rust (`file:line`), location in TypeScript (`file:line`), a concrete process output with both verdicts, and a test that would confirm it. If a category has no findings, say so in one line. At most 900 words. Say plainly what you did not check.
