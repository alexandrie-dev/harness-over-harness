# Batch 1, writer s2: port DeepSeek Harness packages to Rust crates

You are porting TypeScript packages of DeepSeek Harness (dsh) to Rust crates of the HoH project.
Working directory: `/home/save/Alexandrie/HarnessOverHarness` (a cargo workspace, Rust 1.97.1, edition 2024).

## Read first, in full
`docs/porting/PORTING.md` — the binding porting rules (written in Russian). Every rule applies to you.
These packages do not use the kernel; `docs/kernel.md` does not exist yet and you do not need it.

## Your packages, in this order

| source (read-only) | your crate (already created, empty) |
|---|---|
| `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/util/time` | `crates/hoh-util-time` |

For each package read `src/`, `tests/` and `README.md` of the source, then:
1. port everything in `src/` into the crate;
2. port every `tests/*.spec.ts` into `tests/*.rs` (PORTING.md section 10);
3. copy `locale/` unchanged if the source has it;
4. write `PORT_NOTES.md` in the format of PORTING.md section 12.

## Notes for these packages
- The source uses the Temporal polyfill and the `rrule` npm library. The crate manifest already has `jiff` and `rrule`. Use `jiff` for instants, civil dates and IANA zones. For recurrence, first read how `rrule.ts` uses the npm library and check whether the Rust `rrule` crate gives identical occurrences for every test case; if it cannot, implement the needed iteration in this crate and remove the dependency. Either way the ported tests decide.
- Error texts and codes of the strict parsers are part of the behaviour: equal to the source byte for byte.
- All arithmetic is in integer milliseconds as in the source; no floating point where the source has integers.

## Boundaries
- You own only the crate directories listed above. Do not create, edit or delete anything else, including the root `Cargo.toml`, `Cargo.lock` by hand, documents and other crates. Other writers are working in the same repository on other crates right now.
- Dependencies: only crates named in `[workspace.dependencies]` of the root `Cargo.toml`, written as `name.workspace = true` (dev-dependencies may add features, for example `tokio = { workspace = true, features = ["test-util"] }`), plus `path` dependencies on the crates of your own list. If you need anything else, do not add it: finish what you can and state the need in your report.
- Allowed commands, always with the environment `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-s2`: `cargo fmt -p <crate>`, `cargo build -p <crate>`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo test -p <crate>`. Read-only shell commands (`ls`, `cat`, `grep`, `rg`, `wc`) are fine. Nothing else: no installs, no network, no servers, no git command that changes state, nothing run or built inside the source tree.
- If cargo fails because a manifest of a crate that is not yours is momentarily broken, wait a few seconds and retry (up to three times); do not touch that crate.
- Do not improve on the source. Port behaviour faithfully; record doubts in `PORT_NOTES.md`.

## Done when
For every crate of your list: `cargo test -p <crate>` passes, `cargo clippy -p <crate> --all-targets -- -D warnings` is clean, `cargo fmt -p <crate> -- --check` is clean, `PORT_NOTES.md` is complete, and there is no `todo!()`, `unimplemented!()` or `unsafe`.

## Final report (your last message)
Per crate: source files ported; the exact test command and its result line (passed/failed counts); what is not ported and why; open questions for the lead. State plainly anything that is unfinished. Keep it under 400 words.
