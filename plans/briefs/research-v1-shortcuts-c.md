# Research: how to cut the 65 not-started rows of the v1 engine scope

HoH ports DeepSeek Harness (dsh, TypeScript) to Rust package by package. The v1 scope is
`research/v1-engine-scope.codex.md`; the 65 rows not started yet (about 69,000 source lines) are listed in
`research/v1-unstarted.tsv` (crate, source lines, source path, status). Source clone (read-only):
`/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness`. Library choices already made: `docs/porting/DEPS.md`.
How the port is accepted: `docs/porting/PORTING.md` (Russian) and `crates/hoh-conformance/README.md` — recorded dsh
scenarios (242) are replayed through our binary and compared byte for byte; that is the proof of equivalence.

The founder wants speed and asked whether ready Rust libraries exist. For each row or group of rows, find the
cheapest path:
- **(a)** a ready Rust crate covers most of it (as already done in `DEPS.md` for libraries);
- **(b)** a ready part of an open Rust agent engine — at least OpenAI Codex CLI (`codex-rs`, github.com/openai/codex)
  and Block's Goose (github.com/block/goose); check licences, and that the parts can be taken separately: sandbox,
  process launching, MCP client, patch application, file tools, shell, session storage, compaction. Find on
  crates.io what else exists for these tasks (e.g. the official MCP Rust SDK `rmcp`);
- **(c)** keep the dsh TypeScript package as it is, as a separate Node process next to the Rust engine, port later.
  The cost: Node in the product, and how many npm dependencies each such package pulls (count from the package's
  `package.json` and the clone's lock file, transitively, excluding dev dependencies); how it would talk to the
  Rust engine;
- **(d)** port by hand as now.

For (a) and (b) say plainly what is lost: where behaviour stops matching dsh, which recorded scenarios would stop
being proof (name them by directory under `snapshots/`), and what extra tests would replace them.

## Your part
Option (c) for all 65 rows, from the local clone only (no network): for each package, its runtime npm
dependencies (direct and transitive, from `package.json` files and the clone's lock file, excluding dev
dependencies and workspace packages already ported), which other dsh packages it needs at runtime that are
ported to Rust (so the TS side would need a bridge), and whether it can run as a separate Node process at all
(does it need in-process access to the kernel, the session, tools?). Then the fixed cost of Node in the product.
Your report (final message) is the file content of `research/v1-shortcuts-c.md`: a table row by row, then the
findings. Another researcher does (a) and (b) in parallel; do not duplicate it.

## Rules
Read-only for this repository: do not edit or create files in it except the one output file named below. No builds,
no installs, no cloning into the repository (if you need to read a remote repository, read it on the web; a
shallow clone into `/mnt/toshiba/alexandrie/hoh/refs/` is allowed for (b)). Cite each claim: a URL with version or
commit, or a file:line. Mark anything not checked as «не проверено». Output in Russian.
