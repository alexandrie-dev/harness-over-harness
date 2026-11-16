# Stage 0, writer k1: implement the HoH kernel

You are implementing the kernel of HoH, a Rust agent harness. Working directory:
`/home/save/Alexandrie/HarnessOverHarness` (a cargo workspace, Rust 1.97.1, edition 2024).

## Read first, in full

1. `docs/kernel.md` — the binding specification: concepts (section 1), public API (section 2), rules R1–R62
   (section 3), the fiber state machine and the concurrency model (section 4), worked examples (section 6), the
   TypeScript-to-Rust idiom table (section 7), the test plan (section 8), the lead's decisions (section 10).
   Sections 2.9 and 5 describe the loader, which another writer implements; read them to know what the loader
   will ask of the kernel.
2. `docs/porting/PORTING.md` — the project's coding rules (sections 4, 6, 7, 8, 10, 12 apply to you).

## What you build

- `crates/hoh-kernel` — everything in sections 2.1–2.7 of the specification, obeying rules R1–R62.
- `crates/hoh-kernel-macros` — section 2.8: `#[derive(Inject)]` and `#[plugin]`, with the compile errors named there.

Both crates already exist with manifests and an empty `lib.rs`. The third-party crates you may use are already in
their `Cargo.toml`. The two crates may depend on each other as needed (for example `hoh-kernel` as a
dev-dependency of the macros crate for its UI tests). Do not depend on any other crate of this workspace:
`hoh_kernel::Error` is declared as
`Box<dyn std::error::Error + Send + Sync + 'static>`, which is the same type as `hoh_error::BoxError`.
The kernel's cancellation is `tokio_util::sync::CancellationToken` as specified; the reason-carrying
`AbortSignal` of section 10 lives in another crate and is not your concern.

The signatures of the specification are a contract: the loader and about 200 crates will be written against them.
Implement them as written. If a signature turns out to be impossible or wrong, implement the closest working form,
keep going, and record each such case in `crates/hoh-kernel/PORT_NOTES.md` under the heading
"Отличия от спецификации" with the reason. They will be reviewed.

## Order of work

1. First make the whole public API of sections 2.1–2.8 exist and compile (bodies may be minimal at this point):
   the loader writer needs to compile against it. Write the line "API compiles" with the time into
   `crates/hoh-kernel/PORT_NOTES.md` when this point is reached.
2. Then the behaviour: the kernel task and fiber lifecycle; effects; services and realms; events; configuration;
   logger and time; macros.
3. Port the tests as you go, not at the end.

## Tests (section 8 of the specification is the plan)

- Upstream Cordis tests: `/mnt/toshiba/alexandrie/hoh/refs/cordis/packages/core/tests/*.spec.ts`. Port the files and
  cases listed in 8.1 into `crates/hoh-kernel/tests/` under the module names given there.
- dsh tests: `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/scripts/*.spec.ts`. Port the files of 8.2 that map
  to `hoh-kernel/tests/`. The ones mapped to `hoh-loader/tests/` belong to the loader writer.
- Own tests of 8.4: `hoh-kernel-macros/tests/ui/*.rs` with `trybuild`, and `hoh-kernel/tests/concurrency.rs`.
- Every case the plan marks as not ported goes into `PORT_NOTES.md` with its reason. A case you cannot port for a
  reason the plan does not give is a finding: record it, never drop it silently.
- Keep the source's case names as test names (snake_case) so a reviewer can pair them.
- Tests run on `#[tokio::test(start_paused = true)]`, never touch the network, never depend on wall-clock time.
- The sources of the behaviour are `/mnt/toshiba/alexandrie/hoh/refs/cordis/packages/core/src` and, with dsh's
  local fixes, `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/vendor/{cordis,timer,logger-console,schemastery}`
  and `vendor/README.md`. Where the specification and a source test disagree, the test wins: follow the test and
  record the disagreement under "Отличия от спецификации".

## Boundaries

- You own only `crates/hoh-kernel` and `crates/hoh-kernel-macros`. Do not create, edit or delete anything else:
  not the root `Cargo.toml`, not documents, not other crates. Other writers are working in this repository now.
- The reference trees under `/mnt/toshiba/alexandrie/hoh/refs` are read-only. Never run or build anything there.
- No `unsafe` (the workspace lint denies it). No global mutable state. No `unwrap`, `expect` or `panic!` outside
  tests except for a proven invariant with a one-line comment. No `println!`.
- Allowed commands, always with the environment
  `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-k1`:
  `cargo fmt -p <crate>`, `cargo build -p <crate>`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
  `cargo test -p <crate>`. Read-only shell commands are fine. Nothing else: no network, no new tools or packages,
  no servers, no sub-agents, no git command that changes state.
- If cargo fails because the manifest of a crate that is not yours is momentarily broken, wait and retry.

## Done when

`cargo test -p hoh-kernel` and `cargo test -p hoh-kernel-macros` pass with every planned case ported;
`cargo clippy -p <crate> --all-targets -- -D warnings` and `cargo fmt -p <crate> -- --check` are clean for both;
`crates/hoh-kernel/PORT_NOTES.md` lists, per source test file, the cases ported and not ported with reasons, and
every deviation from the specification.

This is a large task. Work until it is done; do not stop with a partial result and a plan for the rest. If you
are forced to stop, say exactly what is finished, what is not, and which tests fail.

## Final report (your last message)

The test commands and their result lines (passed and failed counts per crate); the list of deviations from the
specification; cases not ported beyond those the plan names; open questions for the lead. At most 500 words.
