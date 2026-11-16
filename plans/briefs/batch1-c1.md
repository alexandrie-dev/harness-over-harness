# Batch 1, writer c1: port DeepSeek Harness packages to Rust crates

You are porting TypeScript packages of DeepSeek Harness (dsh) to Rust crates of the HoH project.
Working directory: `/home/save/Alexandrie/HarnessOverHarness` (a cargo workspace, Rust 1.97.1, edition 2024).

## Read first, in full
`docs/porting/PORTING.md` — the binding porting rules (written in Russian). Every rule applies to you.
These packages do not use the kernel; `docs/kernel.md` does not exist yet and you do not need it.

## Your packages, in this order

| source (read-only) | your crate (already created, empty) |
|---|---|
| `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/util/error` | `crates/hoh-error` |
| `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/util/brand` | `crates/hoh-brand` |
| `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/util/deque` | `crates/hoh-deque` |
| `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/util/chunked-list` | `crates/hoh-chunked-list` |
| `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/util/crypto` | `crates/hoh-util-crypto` |
| `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/util/home-paths` | `crates/hoh-home-paths` |
| `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/util/atomic-write` | `crates/hoh-atomic-write` |
| `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/identity/anonymous-user-id` | `crates/hoh-anonymous-user-id` |

For each package read `src/`, `tests/` and `README.md` of the source, then:
1. port everything in `src/` into the crate;
2. port every `tests/*.spec.ts` into `tests/*.rs` (PORTING.md section 10);
3. copy `locale/` unchanged if the source has it;
4. write `PORT_NOTES.md` in the format of PORTING.md section 12.

## Notes for these packages
- `hoh-error` and `hoh-brand` are the foundation every later crate builds on, so design them with their users in mind. Before writing them, grep the source tree (`packages/`, excluding `client`, `api`, `typert`, `host`, `experimental`) for `extends HarnessError`, `instanceof HarnessError`, `.code ===` and for uses of `Branded<` / the brand helpers, and choose the smallest Rust design that lets those call sites be ported naturally: for errors, uniform access to the stable `code`, the message and the cause chain across many crate-local error types; for ids, a one-line way for any crate to declare a branded id newtype over `String` with `#[serde(transparent)]`. Record the design and one usage example in each PORT_NOTES.md under "Отличия от образца".
- `hoh-chunked-list` uses zod only to validate a JSON checkpoint: use serde.
- `hoh-util-crypto`: random v4 UUID from `getrandom`, base64 from the `base64` crate; output formats must equal the source's.
- `hoh-atomic-write` depends on `hoh-error`; `hoh-anonymous-user-id` depends on `hoh-brand` and `hoh-home-paths`. Port in the order listed.

## Boundaries
- You own only the crate directories listed above. Do not create, edit or delete anything else, including the root `Cargo.toml`, `Cargo.lock` by hand, documents and other crates. Other writers are working in the same repository on other crates right now.
- Dependencies: only crates named in `[workspace.dependencies]` of the root `Cargo.toml`, written as `name.workspace = true` (dev-dependencies may add features, for example `tokio = { workspace = true, features = ["test-util"] }`), plus `path` dependencies on the crates of your own list. If you need anything else, do not add it: finish what you can and state the need in your report.
- Allowed commands, always with the environment `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-c1`: `cargo fmt -p <crate>`, `cargo build -p <crate>`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo test -p <crate>`. Read-only shell commands (`ls`, `cat`, `grep`, `rg`, `wc`) are fine. Nothing else: no installs, no network, no servers, no git command that changes state, nothing run or built inside the source tree.
- If cargo fails because a manifest of a crate that is not yours is momentarily broken, wait a few seconds and retry (up to three times); do not touch that crate.
- Do not improve on the source. Port behaviour faithfully; record doubts in `PORT_NOTES.md`.

## Done when
For every crate of your list: `cargo test -p <crate>` passes, `cargo clippy -p <crate> --all-targets -- -D warnings` is clean, `cargo fmt -p <crate> -- --check` is clean, `PORT_NOTES.md` is complete, and there is no `todo!()`, `unimplemented!()` or `unsafe`.

## Final report (your last message)
Per crate: source files ported; the exact test command and its result line (passed/failed counts); what is not ported and why; open questions for the lead. State plainly anything that is unfinished. Keep it under 400 words.
