# Writer cf1: the scenario runner `hoh-conformance`

The proof of the port is the recorded scenarios of DeepSeek Harness (dsh) replayed through our own built
binary: 242 scenarios (`session` 192, `sdk` 41, `acp` 9). You write the program that runs them and counts
honestly. Working directory `/home/save/Alexandrie/HarnessOverHarness` (cargo workspace, Rust 1.97.1,
edition 2024). The source is read-only under `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness`.

## Read first, in full

- `crates/hoh-conformance/README.md` — how dsh replays and compares (written in Russian from the source; it
  cites the source files: read those too wherever you implement a rule). Sections 1–6 are your
  specification; section 7 lists the open questions, answered below.
- `conformance/EXPECTED.tsv` — one row per scenario (suite, scenario, profile, composition, platform,
  session_format, plugins, status, reason).
- `docs/porting/PORTING.md` (binding rules) and the public API and `PORT_NOTES.md` of the crates you use:
  `hoh-session-format`, `hoh-session-format-v0-to-v1`, `hoh-util-values` (`js_json_*`), `hoh-jsdiff`
  (readable diffs), `hoh-loader` (its YAML reader `src/yaml.rs`: `snapshot.yml` is read with js-yaml
  `JSON_SCHEMA`, which part A of the loader reproduces; if no public function fits, say so and add nothing to
  that crate).

## Answers to README section 7 (decided by the lead)

1. `hoh-loader` keeps the row `id`s and understands the `@deepseek-ai/dsh-*` names through its alias table;
   the runner does **not** rewrite patches. It materializes them as dsh does (README 1.2); a relative plugin
   path (`./x.mjs`, `../../scripts/x.ts`) becomes the name `local:<path from the clone root>`, the same form
   as the `plugins` column of `EXPECTED.tsv`. The binary resolves such names to Rust stand-in plugins; until a
   stand-in exists the binary fails to start and the scenario fails with that reason.
2. `!!js` is evaluated by `hoh-loader`; the runner passes patches through.
3. Stand-in plugins (`local:`) are written later, as the packages they imitate are ported; not your task.
4. Inputs in V0–V3: migrate with what exists (`hoh-session-format-v0-to-v1`); a scenario whose input needs a
   migration that is not ported yet fails with reason `migration vN->vM not ported`. Never mark it passing.
5–6. Follow the source; record what you established in the notes.
7. Environment: as dsh (README 1.2) plus isolation of data: every run gets its own directory under the root
   given on the command line; `HOME` is inherited as dsh does. Tools the host must have: list them.
8. Order of parallel children: follow the source's binding rule (first model call); nothing more.
11. Yes: the runner speaks the SDK JSON-RPC and ACP as a client, as dsh's harnesses do (README section 5);
    the protocols are kept unchanged.
12. Timeouts may be longer; make them a command-line option with dsh's values as defaults.

## What to build

A binary `hoh-conformance` in this crate (`src/main.rs` plus library modules):

```
hoh-conformance --hoh <path to built hoh> --refs <dsh clone> --runs <dir for run data>
                [--suite session|sdk|acp]... [--scenario NAME]... [--jobs N] [--results <tsv>]
```

For every selected scenario: choose input files (1.1), prepare the working directory and its setups (1.2;
the named `workspace.setup`s, the 192-character cwd, outside-temp parents), set arguments and environment
(1.2), feed input (1.3: task for headless, actions for SDK, the step script for ACP), run the process with
its timeout, collect what it wrote (sessions, stdout, stderr, exit code, frames, the working tree), normalize
exactly as dsh does (section 2, no more and no less) and compare (section 3, by suite; parsed JSON values,
strings and numbers exact — 6.2). Write one line per scenario to the results file (default
`conformance/RESULTS.tsv`): `suite, scenario, status, phase, reason` with status `pass` or `fail`, the phase
where it stopped (`input`, `setup`, `launch`, `exit`, `log`, `headers`, `tree`, `stdout`, `frames`,
`scenario-check`), and the first line of the reason; the full diff of a failure goes to
`<runs>/<suite>/<scenario>/diff.txt`. Exit code 0 when every selected scenario ran (passed or failed), 1
when the runner itself could not run one (that is a runner error, reported separately, never counted as a
pass or a fail). Print at the end: `scenarios: P of N pass, F fail, E runner errors`.

A scenario passes only when every comparison of its suite and every scenario-specific check of
`headless.snapshot.ts`/`sdk.snapshot.ts`/the ACP harness that applies to it ran and matched. A check you have
not implemented yet makes the scenario fail with reason `check <name> not implemented` — list them in the
notes (README 6.2, 6.3: nothing that weakens the comparison).

The model is replaced by a replay plugin inside the process (README 1.4). That plugin, `hoh-llm-replay`,
is a separate later task built on `hoh-llm` (being written now); your runner sets every `DSH_SNAPSHOT_*`
variable exactly as dsh does, so the plugin will find what it needs.

## Tests

The comparison must be shown to work in both directions without a built `hoh`:
- identity: for every scenario of the corpus, a fake process that writes back the recorded parent and child
  logs (with the tokens materialized as a live run would) and the recorded stdout/stderr/exit code passes
  the log, header and stdout comparisons — any failure here is a bug of the runner or a recorded difference
  you list with its cause;
- sensitivity: mutations of that output (one changed tool result text, one missing event, one reordered
  event, one changed `usage` number, a changed system prompt satellite, an extra file in the tree) each fail
  in the right phase;
- normalization cases of README section 2, one test per rule, with values taken from the corpus.
The fake process is a small test binary of this crate. Tests may read the dsh clone.

## Boundaries

- You own `crates/hoh-conformance` only (its manifest may gain dependencies: path crates of this workspace
  and entries of `[workspace.dependencies]` in the root `Cargo.toml`, written `name.workspace = true`).
  Nothing else: not the root `Cargo.toml`, not `Cargo.lock` by hand, not other crates, not `EXPECTED.tsv`.
- Environment of every cargo command: `CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4
  CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-cf1`. Allowed: `cargo fmt/build/clippy/test -p
  hoh-conformance`, running your own binary, read-only shell commands. No network, no Node, no installs, no
  git command that changes state. Run data only under `/mnt/toshiba/alexandrie/hoh/target-cf1/runs` or the
  system temp directory.
- Other writers work in the repository at the same time. If cargo fails inside a crate that is not yours,
  wait a minute and retry up to three times; never touch it.
- If a library is needed that has no entry in `[workspace.dependencies]`, stop on that point and report it.

## Done when

`cargo test -p hoh-conformance`, `cargo clippy -p hoh-conformance --all-targets -- -D warnings` and
`cargo fmt -p hoh-conformance -- --check` are clean; the identity and sensitivity tests run over the whole
corpus; running the binary with `--hoh /bin/false` over all three suites finishes with 242 lines in the
results file, every one `fail` in phase `launch` or `exit`, zero runner errors. `PORT_NOTES.md` (Russian,
format of PORTING.md section 12) lists: what is implemented per README section, the checks not implemented
yet, the hosts tools needed, the answers you established to open questions 5 and 6. Work until it is done;
do not stop with a partial result and a plan.

Final report (your last message): test command and result line; the `/bin/false` run's summary line; checks
not implemented, by name; recorded differences found by the identity test; open questions. At most 400 words.
