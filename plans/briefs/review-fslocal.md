# Review: hoh-fs-local against the TypeScript source

You are an independent reviewer in a porting project (DeepSeek Harness, TypeScript → HoH, Rust). The code was written by
another model (Codex). You only read. Do not run builds, tests, servers or any command that writes; do not edit files.

Read: `docs/porting/PORTING.md` (Russian; binding rules, checklist §14, §6 flush before sync_all), `docs/kernel.md` §1–3, 6, 7.
Source (read-only): `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/fs/fs-local` (src/, tests/, locale/). Port: crates named
above (src/, tests/, PORT_NOTES.md). Linux only: Windows-only cases are legitimately excluded.

Check: every exported function, type, service method and event against the source; error kinds, codes and texts (locale
byte for byte); order of effects; path handling (symlinks, .., roots), atomic writes and durability (flush before sync,
error on close), locking, file watching. Excluded source cases: confirm each is unobservable on Linux or justified in
PORT_NOTES. Sample at least two test files in full against source expected values. Also give your view on the writer's
open questions listed in PORT_NOTES (localeCompare, notify vs chokidar grouping, close errors) — is the divergence
observable, and what is the smallest faithful fix?

Triage: a divergence that follows from Rust's type system and cannot reach an observer is a `note`; a `bug` is an
accepted input with a different result, order of effects, error or text than the source.
Output: findings, most severe first: kind, Rust file:line, TS file:line, concrete input with both outputs, a confirming
test. At most 800 words. Say what you did not check.
