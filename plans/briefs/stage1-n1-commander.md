# Writer n1: port the used part of commander 15.0.0 into the crate `hoh-commander`

You are writing one Rust crate in the workspace `/home/save/Alexandrie/HarnessOverHarness` (Rust 1.97.1,
edition 2024). A TypeScript agent harness (DeepSeek Harness, "dsh") is being ported to Rust. Its executables
declare their command lines with the npm library `commander` 15.0.0. What that library prints — help screens,
error messages, the exit code — is seen by users and is compared byte for byte in recorded runs of the harness,
so the Rust port must print exactly what commander prints, for the part of commander the harness uses.

## Read first

- `docs/porting/PORTING.md` — the project's rules. Sections 1, 4, 5, 6, 7, 10, 12 apply (port behaviour, not text;
  texts a user sees are copied exactly; no `unsafe`; no global state; no `println!`; no `unwrap`, `expect` or
  `panic!` outside tests except for a proven invariant with a one-line comment; the format of `PORT_NOTES.md`).
- The library, read-only, never run or build anything there: `/mnt/toshiba/alexandrie/hoh/refs/commander`
  (tag v15.0.0): `lib/command.js`, `lib/option.js`, `lib/argument.js`, `lib/help.js`, `lib/error.js`,
  `lib/suggestSimilar.js`, and `tests/`.
- Every place the ported harness uses commander, under `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness`
  (read-only). These files define the part to port:
  `apps/cli/src/args.ts`, `packages/bundle/headless/src/startup.ts`, `packages/bundle/sdk-app/src/index.ts`,
  `packages/bundle/acp-app/src/index.ts`, `packages/bundle/web-app/src/startup.ts`,
  `packages/api/gateway/src/index.ts`, and `packages/boot/cmdline/src/index.ts` with its test
  `packages/boot/cmdline/tests/cmdline.spec.ts` (how a program is parsed and how its exit is intercepted).

## What to build, in `crates/hoh-commander` (the crate exists with a manifest and an empty `lib.rs`)

A `Command` builder, an `Opt` (commander's `Option`; the Rust name avoids the clash with `std::option::Option`),
an `Argument`, a `CommanderError` with commander's `code`, `exitCode` and `message`, and an `InvalidArgumentError`.
Method names are commander's in snake_case. Every call the files above make must be expressible one to one;
count the methods in those files before you start. At the time of writing they are: `name`, `description`,
`usage`, `version`, `option` (with and without a default, with and without a value parser), `requiredOption`,
`addOption` with `new Option(..)` and its `choices`, `default` and other modifiers used there, `argument`,
`command`, `action`, `helpOption` (including disabling it), `helpCommand`, `addHelpText`, `allowUnknownOption`,
`passThroughOptions`, `enablePositionalOptions`, `exitOverride`, `configureOutput`, `parse` with
`{ from: 'user' }`, `error`, `opts`, the processed arguments, and the `_actionHandler` test that
`cmdline/src/index.ts` performs (give it a public equivalent, for example `has_action`). If one of the files uses
something not in this list, port that too and name it in your report.

Design constraints:

- Parsed option values are JSON values (`serde_json::Value`) under commander's attribute names (the camelCase
  name commander derives from the long flag), in an order-preserving map, as `opts()` returns them. A value
  parser is a closure from the raw string and the previous value to a value or an `InvalidArgumentError`.
- An action is a closure that receives the processed arguments, the options and the command, and returns
  `Result<(), CommanderError>` or another error type of your choosing that can also carry a non-commander
  failure: `cmdline.spec.ts` distinguishes a commander exit from any other failure thrown by an action.
- Output goes only through the configured writers (`configureOutput`'s `writeOut` and `writeErr`); the default
  writers write to the process's stdout and stderr. Nothing else prints.
- `exitOverride`: with it, a help or version request and every parse error return a `CommanderError` from
  `parse` instead of ending the process; port that mode fully. Without it commander calls `process.exit`: provide
  the same through an explicit exit callback so that the library itself never ends the process.
- No terminal width detection and no colour: the harness's recorded outputs are produced without a terminal.
  Use the width commander uses when the output is not a terminal, and say so in the notes.

Port the algorithms step by step from the source — how flags are split and matched (`-abc`, `-x=1`, `--long=value`,
`--no-long`, optional and required values, variadic values and arguments, `--`), unknown options and excess
arguments, option defaults, required options, choices, suggestions for mistyped options and commands
(`suggestSimilar.js`), the order of checks and therefore which error wins, and the whole help formatter
(`help.js`: usage line, sections, column alignment, wrapping, default and choices annotations). Error texts and
codes come from the source exactly.

Not ported, with the reason in `PORT_NOTES.md`: everything of commander that none of the files above uses
(for example executable subcommands, hooks, option groups, `storeOptionsAsProperties`, environment variables for
options, `parseAsync` if no file uses it). List each such feature.

## Tests

- Port commander's own tests for the features you port: go through `tests/` of the library, and for each file
  decide by its subject; keep the case names in snake_case. Expected texts are copied exactly. List in
  `PORT_NOTES.md`, per source test file, the cases ported and the cases not ported with the reason.
- For each of the six harness programs above, a test that builds the same program with your API (copy the
  flags and descriptions exactly) and checks: the full help text is non-empty and contains every option, one
  successful parse with typical arguments, and one failing parse. The lead will afterwards compare these
  programs against the real library with an independent fixture, so keep the builders in a test support module
  (`tests/support/programs.rs`) that a second test file can reuse.

## Boundaries

- You own only `crates/hoh-commander`. Do not create, edit or delete anything outside it: not the root
  `Cargo.toml`, not other crates, not documents. Other writers are working in this repository right now.
  Dependencies: `indexmap` and `serde_json` are in the manifest; nothing else.
- Copy the library's `LICENSE` (MIT) into the crate unchanged and say so in your report.
- Allowed commands, always with the environment
  `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-n1`:
  `cargo fmt -p hoh-commander`, `cargo build -p hoh-commander`,
  `cargo clippy -p hoh-commander --all-targets -- -D warnings`, `cargo test -p hoh-commander`. Read-only shell
  commands are fine. Nothing else: no network, no new tools or packages, no Node, no servers, no git command
  that changes state.

## Done when

`cargo test -p hoh-commander` passes; `cargo clippy -p hoh-commander --all-targets -- -D warnings` and
`cargo fmt -p hoh-commander -- --check` are clean; `crates/hoh-commander/PORT_NOTES.md` is written in the format
of PORTING.md section 12 (in Russian, like the other crates' notes); no `todo!()`, `unimplemented!()` or stub is
left.

Work until it is done; do not stop with a partial result and a plan. Your last message is the report: the test
command and its result line, the number of source cases ported and not ported, the commander features not
ported, anything that differs from this task, open questions. At most 350 words.
