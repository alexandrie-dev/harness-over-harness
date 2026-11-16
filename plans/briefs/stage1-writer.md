# Stage 1 writer: port DeepSeek Harness packages to Rust crates

You are porting TypeScript packages of DeepSeek Harness (dsh) to Rust crates of the HoH project.
Working directory: `/home/save/Alexandrie/HarnessOverHarness` (a cargo workspace, Rust 1.97.1, edition 2024).
Your assignment — your writer id, your build directory and your packages with their crates — is in the message
that sent you here. Everything below applies to every package of that assignment.

## Read first, in full

- `docs/porting/PORTING.md` — the binding porting rules (written in Russian). Every rule applies to you.
- `docs/kernel.md` — the kernel your plugins run on: section 1 (concepts), section 2.1–2.8 (the public API of
  `hoh-kernel` and its macros), section 6 (three worked examples: a service provider, a consumer with injected
  services, a tool with an event listener), section 7 (how each TypeScript construct of dsh is written in Rust)
  and section 10 (decisions: the cancellation type, the error type). Section 3 holds the exact rules of
  behaviour; consult it when a question of ordering or lifecycle arises.
- For every crate your package depends on: its public API and its `PORT_NOTES.md`. Build on them as they are.

## For each package

The source is read-only: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/<path from your assignment>`. Read
its `src/`, `tests/` and `README.md`, then:

1. port everything in `src/` into the crate (the crate exists with a manifest that lists the crates it
   depends on and an empty `src/lib.rs`);
2. port every `tests/*.spec.ts` into `tests/*.rs`, keeping the case names in snake_case (PORTING.md section 10);
3. copy `locale/` unchanged if the source has it;
4. write `PORT_NOTES.md` in the format of PORTING.md section 12, in Russian like the other crates' notes.

What must stay exactly as in the source, because recorded sessions of dsh are replayed against the result and
compared byte for byte: the plugin's row name, service names, event names, configuration field names and
defaults, every text a model or a user sees (prompts, tool descriptions and schemas, tool results, error
messages), the shape and key order of everything written to the session log or sent over a protocol.

A plugin of the source (`name`, `inject`, `Config`, `apply`) becomes one plugin as section 2.1 and 2.8 of
`docs/kernel.md` describe. A service type or an event that the source package declares belongs to your crate; a
service or an event declared by another package is imported from that package's crate. Before writing, check
which exports of the package are used by packages that are being ported (not `packages/client`, `api`,
`typert`, `host`, `experimental`, `apps/web`, `apps/desktop*`): port everything, and list in the notes the
exports no ported package uses.

## Boundaries

- You own only the crates of your assignment. Do not create, edit or delete anything else: not the root
  `Cargo.toml`, not `Cargo.lock` by hand, not documents, not other crates. Other writers are working in this
  repository on other crates at the same time.
- If a crate you depend on lacks something you need, do not change it and do not copy its code: finish what
  can be finished, and describe exactly what is missing under "Вопросы ведущему" in your notes and in your report.
- Dependencies: the crates already in your manifest, plus crates named in `[workspace.dependencies]` of the root
  `Cargo.toml`, written as `name.workspace = true` (dev-dependencies may add features, for example
  `tokio = { workspace = true, features = ["test-util"] }`). Nothing else; if the source uses a library that has
  no entry in `docs/porting/DEPS.md`, stop and report it.
- Allowed commands, always with the environment `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=<your build directory>`:
  `cargo fmt -p <crate>`, `cargo build -p <crate>`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
  `cargo test -p <crate>`. Read-only shell commands are fine. Nothing else: no network, no new tools or packages,
  no Node, no servers, no git command that changes state.
- If cargo fails because the manifest of a crate that is not yours is momentarily broken, wait a few seconds
  and retry (up to three times); do not touch that crate.
- Do not improve on the source. Port behaviour faithfully; record doubts in `PORT_NOTES.md`.

## Done when

For every crate of your assignment: `cargo test -p <crate>` passes, `cargo clippy -p <crate> --all-targets -- -D
warnings` is clean, `cargo fmt -p <crate> -- --check` is clean, `PORT_NOTES.md` is complete, and there is no
`todo!()`, `unimplemented!()` or stub left. Work until it is done; do not stop with a partial result and a plan.

## Final report (your last message)

Per crate: source files ported; the exact test command and its result line (passed and failed counts); the
number of source test cases ported and not ported, with reasons; what differs from the source on purpose; what
is missing in crates you depend on; open questions for the lead. State plainly anything unfinished. At most
400 words.
