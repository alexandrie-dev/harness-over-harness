# Batch 1, writer s1: port DeepSeek Harness packages to Rust crates

You are porting TypeScript packages of DeepSeek Harness (dsh) to Rust crates of the HoH project.
Working directory: `/home/save/Alexandrie/HarnessOverHarness` (a cargo workspace, Rust 1.97.1, edition 2024).

## Read first, in full
`docs/porting/PORTING.md` — the binding porting rules (written in Russian). Every rule applies to you.
These packages do not use the kernel; `docs/kernel.md` does not exist yet and you do not need it.

## Your packages, in this order

| source (read-only) | your crate (already created, empty) |
|---|---|
| `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/util/values` | `crates/hoh-util-values` |

For each package read `src/`, `tests/` and `README.md` of the source, then:
1. port everything in `src/` into the crate;
2. port every `tests/*.spec.ts` into `tests/*.rs` (PORTING.md section 10);
3. copy `locale/` unchanged if the source has it;
4. write `PORT_NOTES.md` in the format of PORTING.md section 12.

## Notes for these packages
- This package mixes real logic with helpers that exist only because of JavaScript. Decide per export: `JsonValue` maps to `serde_json::Value` (re-export or alias, no new type); `assertNever` and `deepFreeze` have no Rust counterpart (exhaustive `match`, immutability by default) and are listed under "Не перенесено"; `WeakMapWithValues` relies on garbage-collector weak references, so first grep the source tree for its users and port it only in the form those users need, otherwise list it under "Не перенесено" with the list of users you found. `partial-json.ts` (parsing of incomplete JSON from a model stream) is real logic and must be ported in full with every test.
- Number formatting and key order matter here: read PORTING.md section 4 before starting.

## Boundaries
- You own only the crate directories listed above. Do not create, edit or delete anything else, including the root `Cargo.toml`, `Cargo.lock` by hand, documents and other crates. Other writers are working in the same repository on other crates right now.
- Dependencies: only crates named in `[workspace.dependencies]` of the root `Cargo.toml`, written as `name.workspace = true` (dev-dependencies may add features, for example `tokio = { workspace = true, features = ["test-util"] }`), plus `path` dependencies on the crates of your own list. If you need anything else, do not add it: finish what you can and state the need in your report.
- Allowed commands, always with the environment `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-s1`: `cargo fmt -p <crate>`, `cargo build -p <crate>`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo test -p <crate>`. Read-only shell commands (`ls`, `cat`, `grep`, `rg`, `wc`) are fine. Nothing else: no installs, no network, no servers, no git command that changes state, nothing run or built inside the source tree.
- If cargo fails because a manifest of a crate that is not yours is momentarily broken, wait a few seconds and retry (up to three times); do not touch that crate.
- Do not improve on the source. Port behaviour faithfully; record doubts in `PORT_NOTES.md`.

## Done when
For every crate of your list: `cargo test -p <crate>` passes, `cargo clippy -p <crate> --all-targets -- -D warnings` is clean, `cargo fmt -p <crate> -- --check` is clean, `PORT_NOTES.md` is complete, and there is no `todo!()`, `unimplemented!()` or `unsafe`.

## Final report (your last message)
Per crate: source files ported; the exact test command and its result line (passed/failed counts); what is not ported and why; open questions for the lead. State plainly anything that is unfinished. Keep it under 400 words.
