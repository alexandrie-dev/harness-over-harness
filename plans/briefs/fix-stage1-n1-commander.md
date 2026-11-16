# Fix round for `hoh-commander` after the lead's comparison with the library and an independent review

Your port is accepted as a base. The lead added a comparison with commander 15.0.0 itself:
`tests/fixtures/gen-commander-oracle.mjs` runs the real library on programs described as lists of builder
calls, `tests/fixtures/commander_oracle.jsonl` holds what it printed, threw, parsed and passed to actions, and
`tests/oracle.rs` builds the same programs with the port. Your first delivery matched all 795 runs of ten
programs. The lead then extended the fixture with what the reviewer suspected: it now has 13 programs,
996 runs and 9 definitions the library refuses, and the port differs on 15 of them; `tests/oracle.rs` also
calls two methods that do not exist yet (item 6), so it does not compile until you add them.

Do not edit `tests/oracle.rs` or anything in `tests/fixtures/`. They are the acceptance test of this round.
Same boundaries and commands as `plans/briefs/stage1-n1-commander.md`: you own only `crates/hoh-commander`;
environment `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-n1`; no Node.
The library's source is in `/mnt/toshiba/alexandrie/hoh/refs/commander/lib`.

## A. Differences from the library (the comparison shows each one)

1. **`help` with an empty command name** (`src/command.rs:809-818`). The library shows the program's own help
   on stdout with exit code 0 (`command.js:1392-1395`, `if (!subcommandName) this.help()`); the port treats
   `''` as an unknown subcommand: help on stderr, exit code 1.
2. **White space in help wrapping** (`src/help.rs:214-220`, `:254-285`). The library's regular expressions
   use JavaScript's `\s`; the port uses `char::is_whitespace`. They disagree on two characters: U+0085 is
   white space only for Rust, U+FEFF only for JavaScript. One private function with JavaScript's set, used
   everywhere `help.rs` asks whether a character is white space.
3. **Definitions the library refuses.** The library throws from the builder call that makes a definition
   wrong. The port's builders return `&mut Self`, so it keeps the error and reports it from `parse`; that
   design stays, with these rules:
   - a command keeps its **first** definition error (today the last one overwrites it);
   - `parse` reports the first definition error of the **whole tree** before it parses anything: the
     command's own, then those of its subcommands in the order they were added, recursively. Today an error
     in a subcommand is reported only when that subcommand is the one invoked;
   - **pass-through options** (`command.js:866-885`): the library checks, at the moment
     `passThroughOptions()` is called, whether the parent command already has positional options enabled.
     Do the same: a subcommand knows whether its parent had them enabled when the call is made (a
     subcommand created later takes the parent's value; `enable_positional_options` on the parent tells the
     subcommands that exist), and the call records the definition error with the library's text. The check
     at parse time (`command.rs:782`) goes away;
   - **which option a duplicate flag names** (`command.js:620-632`): the option that owns the short flag if
     the short flag is taken, otherwise the one that owns the long flag; the conflicting flag printed is the
     long one if it is taken, otherwise the short one. The port names the owner of the long flag.
   In `PORT_NOTES.md`: definition errors come from `parse`, and when both a command and one of its
   subcommands are wrong the library reports whichever call was made first, the port the command's own.

## B. What the consumers need

4. **`Command` must be `Send`.** The closure types are `Rc<dyn Fn…>`, so a `Command` cannot be held by a
   plugin's asynchronous code. Every closure the crate stores is `Send + Sync` and is held in an `Arc`;
   `ParseError::Other` holds `Box<dyn Error + Send + Sync>`. A test that fails to compile otherwise:
   `fn assert_send<T: Send>() {}` for `Command`, `Opt`, `Argument`, `Output`, `ParseError`,
   `CommanderError`. (`tests/oracle.rs` already passes closures that are `Send + Sync`.)
5. **`#[must_use]`** on `Command::error`, `error_with_options`, `help` and `help_with_error`, with a message
   that the returned error has to be returned from the action. In the library these calls never return; in
   the port an action that forgets `return Err(…)` would carry on after the output was written.
6. **The width of help comes from the output, as in the library** (`command.js:66-69`, `:2494-2508`;
   `help.js:31`). `Output` gets two builder methods, `out_help_width(self, columns: usize) -> Self` and
   `err_help_width(self, columns: usize) -> Self`; help written to stdout wraps at the first, help written
   to stderr at the second, 80 when unset. `Output::default()`, which writes to the process's stdout and
   stderr, sets each width to the number of columns of that stream when it is a terminal
   (`rustix::termios::tcgetwinsize`; add to your own `Cargo.toml`
   `[target.'cfg(unix)'.dependencies] rustix = { workspace = true, features = ["termios"] }`; on other
   targets the widths stay unset — say so in the notes). The fixture's programs `narrow` (50 columns: below
   40 columns for a description the library stops wrapping it, `help.js:663-670`, `:697`) and `wide`
   (120 columns) cover the wrapping; add a unit test that `Output::default()` leaves both widths unset when
   the streams are not terminals, as under `cargo test`.
7. **Exit callback and exit override: the last call wins.** After `exit_override()` a later
   `exit_callback(f)` is never called (`src/command.rs:445-452`); in the library a later `exitOverride(fn)`
   replaces the earlier setting. Test both orders.

## C. Project rules (`docs/porting/PORTING.md`)

8. Section on values in text: `src/help.rs:296-435` writes JSON and numbers by hand (`json_text`,
   `number_text`). Use `hoh_util_values::js_json_stringify` (add the dependency as
   `hoh-util-values = { path = "../hoh-util-values" }`) and delete the hand-written code.
9. Line 146: a crate's own errors are `enum`s or structs deriving `thiserror::Error`
   (`thiserror.workspace = true`); `src/error.rs` implements `Display` and `Error` by hand.

## D. Tests as strong as the source's

10. `command.helpCommand.test.js` (Rust `tests/command.rs:410-431`): the source tells apart which command
    handled `help`, `help sub1`, `help sub2`; the port asserts only exit code 0, which the program's own help
    also gives. Assert the text (the usage line of the command that answered). Restore the two cases that
    set `helpOption('-H')` and `helpOption('-h, --help')` on the program, and in the case with a custom help
    command check its description too.
11. `options.preset.test.js`, "preset is concatenated": the source's arguments are `-n one -n -n three`
    giving `['one', 'two', 'three']`; the port's test (`tests/repeated_parse.rs:72`) uses other arguments and
    never has a real value followed by a preset. Use the source's.
12. `command.unknownCommand.test.js`, "unknown command but action handler taking arg": the source has a
    subcommand plus `argument('[args...]')` with an action; the port's test (`tests/command.rs:246-253`) has
    no subcommand. Use the source's program.

## E. Notes

13. `PORT_NOTES.md` and `SOURCE_TESTS.md` say `showHelpAfterError`, copying of that setting, and
    `new Command(name)` are used by nobody. Two more packages import commander:
    `apps/plugin-market-generator/src/cli.ts` (uses `showHelpAfterError`) and
    `packages/experimental/cua-mcp/src/index.ts`. Neither is ported now (`docs/porting/MAP.tsv`: `skip` and
    `later`). Say exactly that: not used by the consumers that are ported, and what each of the two would need.

## Done when

`cargo test -p hoh-commander` passes, including `tests/oracle.rs` unchanged;
`cargo clippy -p hoh-commander --all-targets -- -D warnings` and `cargo fmt -p hoh-commander -- --check` are
clean. For items 7 and 10–12 write the test first and see it fail (or, where the code is already right, see
it fail against a temporary break). If you disagree with an item, leave the code alone and say why with the
library's `file:line`.

Work until it is done. Your last message is the report: the result line of each command, one line per item,
anything that differs from this task. At most 300 words.
