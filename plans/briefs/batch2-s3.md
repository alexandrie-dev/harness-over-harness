# Batch 2, writer s3: port DeepSeek Harness packages to Rust crates

You are porting TypeScript packages of DeepSeek Harness (dsh) to Rust crates of the HoH project.
Working directory: `/home/save/Alexandrie/HarnessOverHarness` (a cargo workspace, Rust 1.97.1, edition 2024).

## Read first, in full
`docs/porting/PORTING.md` — the binding porting rules (written in Russian). Every rule applies to you.
These packages do not use the kernel; `docs/kernel.md` does not exist yet and you do not need it.

## Your packages, in this order

| source (read-only) | your crate (already created, empty) |
|---|---|
| `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/session/session-format` | `crates/hoh-session-format` |

For each package read `src/`, `tests/` and `README.md` of the source, then:
1. port everything in `src/` into the crate;
2. port every `tests/*.spec.ts` into `tests/*.rs` (PORTING.md section 10);
3. copy `locale/` unchanged if the source has it;
4. write `PORT_NOTES.md` in the format of PORTING.md section 12.

## Notes for these packages
- This is the machinery that reads a persisted session log of any released format generation and brings it to the current one through adjacent migrations. Every later session crate builds on it, and the conformance scenarios feed it real v0–v4 logs, so its error codes, error texts and the order in which rows are consumed must equal the source exactly.
- It depends on `crates/hoh-util-values` (already ported; add it as a `path` dependency). The source imports `snapshotJsonValue` and `deepFreeze` from it: `snapshot_json_value` exists in the Rust crate; `deepFreeze` has no Rust counterpart (values are immutable unless borrowed mutably), so drop the call and record it in PORT_NOTES.md.
- The source streams rows through stateful stages without copying. Keep the same ownership idea in Rust (stages consume and return owned values); do not introduce shared mutable state.
- This package is a plain library, not a plugin: no kernel, no runtime. Keep the public API synchronous wherever the source is synchronous; where the source is asynchronous (async iterables of rows), use `async fn` and the `Stream`-free shape that the source's own tests need, with tokio only as a dev-dependency for tests if the API itself can stay runtime-agnostic. Record the chosen shape in PORT_NOTES.md.
- JSON handling rules of PORTING.md section 4 (key order, absent versus null, integer formatting) are central here.

## Boundaries
- You own only the crate directories listed above. Do not create, edit or delete anything else, including the root `Cargo.toml`, `Cargo.lock` by hand, documents and other crates. Other writers are working in the same repository on other crates right now.
- Dependencies: only crates named in `[workspace.dependencies]` of the root `Cargo.toml`, written as `name.workspace = true` (dev-dependencies may add features, for example `tokio = { workspace = true, features = ["test-util"] }`), plus a `path` dependency on `crates/hoh-util-values`. If you need anything else, do not add it: finish what you can and state the need in your report.
- Allowed commands, always with the environment `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-s3`: `cargo fmt -p <crate>`, `cargo build -p <crate>`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo test -p <crate>`. Read-only shell commands (`ls`, `cat`, `grep`, `rg`, `wc`) are fine. Nothing else: no installs, no network, no servers, no git command that changes state, nothing run or built inside the source tree.
- If cargo fails because a manifest of a crate that is not yours is momentarily broken, wait a few seconds and retry (up to three times); do not touch that crate.
- Do not improve on the source. Port behaviour faithfully; record doubts in `PORT_NOTES.md`.

## Done when
For every crate of your list: `cargo test -p <crate>` passes, `cargo clippy -p <crate> --all-targets -- -D warnings` is clean, `cargo fmt -p <crate> -- --check` is clean, `PORT_NOTES.md` is complete, and there is no `todo!()`, `unimplemented!()` or `unsafe`.

## Final report (your last message)
Per crate: source files ported; the exact test command and its result line (passed/failed counts); what is not ported and why; open questions for the lead. State plainly anything that is unfinished. Keep it under 400 words.
