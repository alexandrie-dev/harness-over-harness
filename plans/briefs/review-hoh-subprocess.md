# Review: hoh-subprocess against its TypeScript source

You are an independent reviewer in a porting project (DeepSeek Harness, TypeScript → HoH, Rust). The code under review was written by a different model (Codex). You only read. Do not run builds, tests, servers or any command that writes; do not edit or create any file.

## Read
- `docs/porting/PORTING.md` (in Russian) — binding rules; your checklist is its section 14. Sections 4 (what Node does by itself: system error texts via `hoh_error::node_spawn_error_message`, `node_fs_error_message`, `node_error_code`), 6 (cancellation is `hoh_timeout::AbortSignal`) and 7 (the base class in the source decides the error type) matter most here.
- Source: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/subprocess/subprocess` (`src/control.ts`, `error.ts`, `index.ts`, `types.ts`, `tests/`, `README.md`).
- Port: `crates/hoh-subprocess` (`src/`, `tests/`, `PORT_NOTES.md`); the crates it uses: `crates/hoh-http-proxy`, `crates/hoh-error`, `crates/hoh-timeout`.

## What to check
1. Public types and their serialized shapes and key order (`types.ts`): launch options, environment with tombstones (the writer says he fixed JSON serialization of environment tombstones — check against the source), stdio, terminal handle, signals.
2. Errors (`error.ts`): types by base class, codes, texts. The writer reports a discrepancy: `hoh_error::node_spawn_error_message` includes the command for EAGAIN/EMFILE/ENFILE, "contrary to the assignment's wording". Establish from the source and from `crates/hoh-error/tests/fixtures/node_system_errors.json` (texts measured on Node 22) which is right, and say what the port shows a user or a model in that case.
3. The control channel (`control.ts`): framing, every control byte preserved, independent half-close, who owns and closes what.
4. Proxy environment handed to children (`index.ts` with `hoh-http-proxy`): which variables, precedence of the user's exported proxy and `ALL_PROXY`, what is cleared.
5. The service and its registration; process lifecycle "provider-owned, as in the source" — is it?
6. Tests: all 16 source cases have counterparts with assertions that are not weaker; the Rust child fixtures that replace Node children test the same thing.
7. Each entry in `PORT_NOTES.md` "Отличия".

## Triage rule
This is a port to Rust, not an emulation of JavaScript. A divergence that follows from the Rust type system and cannot reach an observer is a `note`, not a `bug`. A `bug` is a case where an input the ported API accepts gives a different result, order of effects, error or text than the source. Statements about Node or libraries made from memory must be checked against the fixtures or the clone, or marked "not checked".

## Output
A list of findings, most severe first. For each: kind (`bug`, `rule violation`, `missing test`, `note`), location in Rust (`file:line`), location in TypeScript (`file:line`), a concrete input with both outputs, and a test that would confirm it. If a category has no findings, say so in one line. Do not suggest improvements the source does not have. At most 800 words. Say plainly what you did not check.
