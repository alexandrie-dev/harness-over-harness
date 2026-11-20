# Review: hoh-shell and hoh-tool-executor-native against their TypeScript source

You are an independent reviewer in a porting project (DeepSeek Harness, TypeScript → HoH, Rust). The code was
written by other models (Kimi, then Sonnet). You only read. Do not run builds, tests, servers or any command that
writes; do not edit or create any file.

## Read
- `docs/porting/PORTING.md` (Russian) — binding rules; checklist in section 14.
- `docs/kernel.md` sections 1–3, 6, 7.
- Source (read-only clone): `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/shell/shell` and
  `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/core/tool-executor-native` (`src/`, `tests/`, `locale/`, README).
- Port: `crates/hoh-shell`, `crates/hoh-tool-executor-native` (`src/`, `tests/`, `PORT_NOTES.md`). They use
  `crates/hoh-tools` (being written by another writer; it still has a `shim` module — report only uses that are
  wrong now, not the shim's existence).

## What to check
1. Every exported function, type, constant and service method against the source. The shell README mentions
   `SHELL_SETTINGS_NAMESPACE` exported from the package; the writer found no such export in `src/index.ts` —
   establish which is true and whether the port needs it.
2. `parseExitStatus`/render: the writer replaced the source regex with string operations; find any input where
   results differ (markers mid-string, non-ASCII digits, CRLF, empty, several markers, signal and code both).
3. JSON field names and key order a model or client sees (`exitCode` and the rest).
4. Executor: `describe`/`execute` order of effects, error propagation, batch semantics, plugin registration
   (`inject = tools`), locale strings byte for byte (8 files).
5. Tests: all 7 shell source tests ported with the source's expected values; whether the executor needs tests the
   source lacks only because TS types guaranteed something Rust does not.

## Triage rule
A divergence that follows from the Rust type system and cannot reach an observer is a `note`. A `bug` is an input
the ported API accepts that gives a different result, order of effects, error or text than the source.

## Output
Findings, most severe first: kind (`bug`, `rule violation`, `missing test`, `note`), Rust `file:line`,
TypeScript `file:line`, concrete input with both outputs, a confirming test. At most 700 words. Say what you did
not check.
