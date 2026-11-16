# Third round for the kernel: what the closure check found

Your second round (commit `c887b43`) was checked against commit `7e9ca39` by the reviewer of the first round.
Closed and proven by a test that fails on the old code: A1, A7, B1, B2, B4, B5, B6, C1, C2, C4–C9, C12–C14, D1,
D2, E1, E2, F, and most of B3. Your three disputes (C3, C10, the unknown-key part of B3) are accepted: the lead
read the source lines you cited. What is left is below. Same boundaries and commands as before: you own only
`crates/hoh-kernel` and `crates/hoh-kernel-macros`; environment
`CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-k1`.

For every item: the test first, seen failing on the present code for the reason named, then the fix. If you
disagree with an item, leave the code alone and say why, with `path:line` of the source.

## Two regressions of the second round

1. **A binding registered by a provider that is still loading now sends `InternalService { available: false }`.**
   `publish` in `fiber.rs` (the new second loop, `old.get(address) != Some(record)`) treats the appearance of a
   hidden record as a change. The source notifies at registration only when the owner is active
   (`vendor/cordis/src/reflect.ts:330-332`) and always at withdrawal (`:335`). Registration of a hidden record
   sends nothing; its withdrawal sends one event (that was C11 and stays). `c11_loading_withdrawal_notifies`
   hides the extra event by clearing the observations after `provide`: remove the clearing and assert the
   exact sequence of events for register → withdraw, and for register → activate → withdraw.
2. **A reply that has already arrived loses to `Closed`.** `wait_reply` and `restart_from_update`
   (`fiber.rs:283`, `select! { biased; closed.cancelled() … rx }`) check the closing token first. The actor's
   `Drop` cancels that token on an ordinary shutdown too, so a `settled()` or `update()` whose `Ok` was
   delivered but which is polled after shutdown returns `Err(Closed)`; at `7e9ca39` it returned `Ok`. Take a
   ready reply first; `Closed` only when there is none. Test: the reply is delivered, the kernel shuts down,
   then the future is polled.

## Not closed

3. **Optional integers (B3).** Coercion and the range check test `s["type"] == "integer"` literally
   (`config/schema.rs:449`, `:741`). `schemars` 1.2.3 gives `Option<u32>` the type `["integer", "null"]`
   (`schemars-1.2.3/src/_private/mod.rs:477-481`). So for an optional integer `2.0` is not converted and `300`
   for `Option<u8>` is not range-checked; both then fail in serde with a text that has no path. Treat a type
   list that contains `integer` the same way. Tests: `Option<u32>` given `2.0` loads as 2; `Option<u8>` given
   `300` is a validation issue with its path in R49's wording; `null` still loads.
4. **`readOnly` / `writeOnly` (B1)** are accepted words without a derived type that produces them. Add a case
   with `#[serde(skip_serializing)]` and one with `#[serde(skip_deserializing)]` fields: the schema passes the
   checker and a value loads.
5. **Unknown key under `additionalProperties: false`** (`schema.rs:420-425`). Decision of the lead: the text
   stays. It is what schemastery prints for a schema that accepts nothing
   (`vendor/schemastery/src/index.ts:653-655`, `expected nullable but got …`), reported at the path of the
   key. Pin it with a test on a config type with `#[serde(deny_unknown_fields)]` (path and text), and say in
   the notes that this wording is the `never` wording. The lead records it in `docs/kernel.md`.

## New findings

6. **`types.retain` over every binding and node runs at the end of every `drive`** (`fiber.rs`). That is a
   full scan per command; during start-up with about 190 plugins it is repeated thousands of times. Do the
   release only after a command that withdrew a binding or removed a node. The C8 tests must still pass.
7. **The schema is inspected twice** (`schema.rs:263-290`): an unsupported word inside a referenced `$defs`
   entry, or under a recursive root (`"$ref": "#"`), is reported once through the reference and once through
   the `$defs` loop, so `ValidationError` lists the same issue twice. Each schema location is inspected once.
   Tests: exactly one issue for each of the two cases.
8. **`SelfWait` for a fiber that is already settled** (`events.rs` `spawn_inherited`). Fiber A is active and
   emits a serial event from a task that carries its mark; a listener of fiber B awaits `A.settled()` inside
   that dispatch and gets `Err(SelfWait)`, although A is settled and the source's promise would resolve at
   once. Waiting for a fiber that is already settled returns `Ok` whatever marks the task carries; `SelfWait`
   is only for a wait that would really block on the caller's own work (the `spawn` case you covered stays an
   error). Test: the scenario above.
9. **Words the checker accepts but `schemars` does not emit.** `WORDS` contains `contains`, `minContains`,
   `maxContains`, `if`, `then`, `else`, `dependentRequired`, `dependentSchemas`, `unevaluatedProperties`,
   `propertyNames`, `$id`, `minProperties`, `maxProperties`; the notes map only part of the list to a
   `schemars` source line. R48 (`docs/kernel.md` 10.1) defines the set as what `schemars` 1.2.3 emits for
   derived types. For each word of `WORDS`: either the notes name the `schemars-1.2.3` `file:line` that emits
   it (attributes such as `#[schemars(length(...))]`, `#[serde(flatten)]`, map keys included), or the word and
   the code that evaluates it are removed, so that it is reported as unsupported. `x-*` extension words stay.
10. **Notes.** Say that a step of an update chain or an event chain runs as its own task and therefore keeps
    running when the caller drops the future (the C14 decision; a JavaScript promise cannot be cancelled
    either), and that `settled()` of a settled fiber never reports `SelfWait` (item 8).

## Done when

`cargo test -p hoh-kernel` and `cargo test -p hoh-kernel-macros` pass;
`cargo clippy -p hoh-kernel -p hoh-kernel-macros --all-targets -- -D warnings` and
`cargo fmt -p hoh-kernel -p hoh-kernel-macros -- --check` are clean. No test of the second round is removed
or loosened except the clearing named in item 1.

Work until every item is done or disputed with evidence. Your last message is the report: the result line of
each command; one line per item (done with the name of its test, or disputed with the source line); for
item 9 the list of words kept and of words removed. At most 350 words.
