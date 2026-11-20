# Sonnet writer for the early milestone

Goal 1 (CEO, 18.11): our `hoh` on a minimal profile holds a persisted session and runs read/write/bash with the model
replayed from a recording. The package set and why each is needed: `research/milestones.md` part A (read it).

Read and follow, in full: `plans/briefs/stage1-writer.md`, `plans/briefs/interface-first.md` (writing ahead of
dependencies: current APIs, shims marked `SHIM(<dep>)`, `CARGO_BUILD_JOBS=2`, `cargo check` while writing, tests
once at the end), `docs/porting/PORTING.md` (Russian; binding rules), `docs/kernel.md` §1–3, 6, 7. Source clone
(read-only): `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness` (`P/` = `packages/`).

Rules: edit only the crates named in your task (skeletons exist under `crates/`); you may add path dependencies on
other workspace crates and `name.workspace = true` entries to your crates' `Cargo.toml`; no other files. No network,
no installs, no git commands that change state. Every cargo command with `CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2
CARGO_TARGET_DIR=<given>`. Many writers work in parallel; if a build fails inside a crate that is not yours, wait a
few minutes and retry, never touch it. Run cargo in the foreground. If a library is missing from
`[workspace.dependencies]`, stop on that point and report it, do the rest.

Done when, for each crate: every source file and test ported (source expected values), `cargo test`,
`clippy --all-targets -D warnings`, `fmt --check` clean, PORT_NOTES.md (Russian) with shims listed. Final report:
per crate result lines, tests ported/not ported (why), shims, questions. At most 400 words.
