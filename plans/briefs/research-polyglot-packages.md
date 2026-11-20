# Research: product packages in four kinds — Markdown skill, Rust, Go, TypeScript (card HOH-27)

You only read. Do not run builds, tests, servers or anything that writes; do not edit files except the one output file
`research/polyglot-packages.md` (Russian, plain words). You may search the web for current facts about runtimes and
libraries (cite URL and date). Repository: /home/save/Alexandrie/HarnessOverHarness.

## Founder's words (19.11 00:02)
«кстати еще я хотел, чтобы пакеты можно было писать на Go или TypeScript, условно было 3 языка на которых можно было
писать пакет. То есть MD (то есть пакет == скилл), Rust, Go, TypeScript».
CEO's reading: these are product extensions (a "plugin" in dsh terms; what the agent writes on request — "a task is a
plugin", card HOH-13), not engine internals. The engine stays Rust; the port does not change.
Done condition: four example packages with the same tool (skill in MD, Rust, Go, TypeScript) install and work in HoH; each is
visible in the window, gets permissions under the same protection rules, and is called by the agent; the way to connect
is described for a package author.

## Read first
`docs/kernel.md` (static composition today), `docs/window.md` (§4 protocol, §5 protection, §7 CEO decisions: model-written
programs in tasks allowed with sandbox, network allow-list, rights confirmed on save; no bundled Node), `PLAN.md`,
`research/v1-engine-scope.codex.md`, `research/supply-chain-ts.md`, `crates/hoh-skill`, `crates/hoh-tools` (tool
definitions), `crates/hoh-protocol`, and how dsh loads plugins/skills (`/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/`:
skill, skill-filesystem, tool-skill, boot/app-boot, mcp-client if present).

## Questions
1. One common mechanism for Rust/Go/TypeScript packages: (a) separate process speaking a protocol over stdio (MCP-like
   or our own JSON-RPC), (b) WebAssembly components (WASI 0.2/0.3, wasmtime), (c) other. For each: what it means for the
   sandbox and permissions (filesystem, network allow-list, process launch, our approval flow with MAC), startup cost,
   crash isolation, author experience, size of what we must embed.
2. How to run TypeScript without bundling Node: an embedded JS engine (e.g. QuickJS via rquickjs, Boa, deno_core/V8),
   compiling TS to WebAssembly (e.g. Javy/ComponentizeJS/StarlingMonkey), or an external runtime chosen by the user.
   Licenses, sizes, TS support (type stripping), API surface the package gets.
3. Go: TinyGo/Go wasip1/wasip2 status vs. native subprocess.
4. Fit with the kernel's static composition (`docs/kernel.md`): what a dynamic package registers (tools, skills, settings,
   events?) and through which boundary; fit with HOH-13 "task as plugin" (language of model-written programs not chosen).
5. What to lay into the kernel and protocol **now** (in the early milestone and window-chat work) so this does not force a
   redesign later — concrete interfaces/fields, as small as possible.

## Output
`research/polyglot-packages.md`: comparison table, recommendation, what goes into the kernel/protocol now, volume
estimate (marked as estimate) and order relative to the window-chat milestone. At most 1200 words. Say what you did not check.
