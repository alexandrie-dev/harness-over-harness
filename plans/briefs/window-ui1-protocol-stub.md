# Writer ui1: the window's protocol types and an engine stub

Working directory `/home/save/Alexandrie/HarnessOverHarness` (cargo workspace, Rust 1.97.1, edition 2024). The
window of HoH is designed in `docs/window.md` (Russian); read it in full first, especially section 4 (the
protocol between window and engine) and section 6 (work packages). You do work packages **2 and 3** of
section 6, in that order. Nothing here needs npm, and nothing may use it.

## Package 2: `crates/hoh-protocol`

- The types of every request, response and notification between window and engine: dsh's SDK protocol as the
  base (newline-delimited JSON-RPC 2.0 over stdio, `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/sdk/protocol/src/types.ts`
  and `transport.ts`, read-only) — names, field names, key order and optional fields exactly as the source,
  because the engine will speak the same protocol to dsh's own SDK clients — plus the methods section 4 of
  `docs/window.md` adds (`session/list`, `session/events`, `session/cancel`, `hoh/employee/*`, `hoh/task/*`,
  `hoh/button/*`, `hoh/schedule/*`, `hoh/artifact/*`, `hoh/model/*`, `hoh/settings/*`, `hoh/security/*`, and the
  approval path with its `mac`). Where section 4 leaves a field open, choose, and list your choices in the notes.
- The protocol must not be tied to stdio (decision of 2026-11-18: a server engine will connect later without changing the window): message types and framing are separate from the transport; stdio is the first transport.
- Serde types with `schemars` schemas; a JSON-RPC frame reader and writer for stdio (newline-delimited, as
  `transport.ts`: what it does with a blank line, an oversize line, invalid JSON, a response without a pending
  request).
- `protocol/fixtures/*.json`: at least one example per method and notification; a test reads every fixture,
  parses it to the type and writes it back byte for byte (key order kept).
- `scripts/gen-protocol-ts.mjs`: a plain Node script with **no packages** (Node 20 is on the machine) that
  reads the JSON Schemas the crate prints (a small binary or test of the crate writes them to
  `protocol/schema/*.json`) and writes `ui/src/protocol.ts` (TypeScript types only, no runtime code). Commit the
  generated file; a test fails when the script's output differs from it.

## Package 3: `crates/hoh-window-stub`

A binary that speaks the protocol over stdio like the engine will, for testing the window before the engine
can hold a session: it reads a recorded dsh session (`snapshots/sdk/<scenario>/session.v4.jsonl` in the source
clone) and plays it back as the notifications the real engine would send for `session/prompt`, in order;
reverse requests (`approval/request`, `user-question/ask`) are sent where the recording has them and wait for the
answer. For the new `hoh/*` methods it answers from `protocol/fixtures`. Tests: `text-turn`, `approval` and
`plan-review` are played back through the binary over a pipe and the frames compared with what dsh's SDK
recordings and `snapshots/sdk/sdk.snapshot.ts` expect a client to see (read how dsh's harness drives them;
`crates/hoh-conformance/README.md` section 5.1 describes it).

## Boundaries

- You own `crates/hoh-protocol`, `crates/hoh-window-stub`, `protocol/`, `scripts/gen-protocol-ts.mjs`,
  `ui/src/protocol.ts`. Nothing else: not the root `Cargo.toml` (members are `crates/*` already), not other
  crates, not documents.
- Dependencies: path crates of this workspace and entries of `[workspace.dependencies]` in the root
  `Cargo.toml`, written `name.workspace = true`. If you need a library that is not there, stop on that point and
  report it.
- Environment of every cargo command: `CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4
  CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-ui1`. Allowed: `cargo fmt/build/clippy/test -p <your
  crates>`, running your binaries, `node scripts/gen-protocol-ts.mjs`, read-only shell commands. No network, no
  npm or pnpm, no installs, no git command that changes state.
- Other writers work in the repository at the same time; if cargo fails inside a crate that is not yours, wait a
  minute and retry up to three times.

## Done when

For both crates: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt -- --check` are clean;
every fixture round-trips; the generated `ui/src/protocol.ts` matches the script's output; the three scenarios
play back through the stub binary. Each crate has `PORT_NOTES.md` (Russian): what comes from dsh's protocol
unchanged, what is new and where `docs/window.md` left it open, what you chose. Work until it is done.

Final report (your last message): test result lines per crate; the methods and notifications covered; your
choices where the design was open; anything the design got wrong. At most 400 words.
