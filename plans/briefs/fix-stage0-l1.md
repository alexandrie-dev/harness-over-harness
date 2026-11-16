# Fix round for writer l1: two independent reviews of `hoh-loader`, part A

Same commands and boundaries as in your task (`CARGO_NET_OFFLINE=true
CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-l1`; you own only `crates/hoh-loader`). Part A is accepted as
a base (62 tests pass at the lead). Two reviewers read it independently; the lead merged and triaged their
findings. Do exactly what is listed.

Two references are new, both read-only:
- `/mnt/toshiba/alexandrie/hoh/refs/js-yaml` — js-yaml 4.3.2, the exact version the harness uses
  (`lib/schema/json.js`, `lib/type/{null,bool,int,float}.js`, `lib/loader.js`, `lib/dumper.js`). You wrote the
  scalar rules from memory because it was missing; now port them from this source.
- `docs/kernel.md` section 5.5: three rows were added to the table (`a + b`, `??` next to `||` or `&&`,
  `.includes` order). `docs/porting/PORTING.md` section 4: three bullets were added before "Время в виде
  строки" (JSON text, recursion, exact integers). Read both.

## YAML reading

1. **Scalar resolution.** Make plain-scalar resolution equal to js-yaml 4.3.2 `JSON_SCHEMA` exactly, from
   `lib/type/int.js`, `float.js`, `bool.js`, `null.js` (resolve and construct). That version has no underscore
   support at all, so `1_`, `0_1`, `0b_`, `0xF_`, `.1_` are strings; `.`, `+.`, `-.`, `.e2`, `-.5`, `+.5` are strings
   (the port now fails on some and makes numbers of others); `.2e2` is the number 20; an integer whose value is
   not finite is not an integer; see what `constructYamlInteger` and `constructYamlFloat` give for `-0`, `-0.0`,
   `0x`, `0o`, `0b` forms and large values such as `0x20000000000000801` and do the same. An explicit `!!float 1`
   is valid and gives a number; check `!!int`, `!!str`, `!!bool`, `!!null` on plain and quoted scalars the same way.
   Replace the current expectations that contradict the source (`yaml.rs:647`, `tests/entry_yaml.rs:181`, `:204`).
   Test: one table of at least 60 plain scalars with the expected kind and value, each traceable to a line of the
   js-yaml source. `.inf` and `.nan` stay rejected (JSON cannot hold them); say so in the notes.
2. **Aliases.** js-yaml shares the referenced node; the port clones the subtree at every alias (`yaml.rs:78-84`).
   Thirty lines of `&a2 [*a1, *a1]` give exponential memory, and a chain `&a2 [*a1]`, `&a3 [*a2]`, … gives a depth
   that the 128 limit does not see (it counts only where the alias stands), after which `Node::value`,
   `validate_expressions`, `Clone` and `Drop` overflow the stack. Bound both: the depth limit applies to the
   expanded value, and the total number of nodes produced by expansion has a limit; exceeding either is a
   `LoadError` with file and position. Tests for both inputs, expecting an error.
3. **Cost.** `validate_expressions` (`yaml.rs:166`) and `entry` (`:347`) call `node.value()` at every level, which is
   O(n·depth). Make reading linear.
4. **Empty input.** The source test asserts `parseEntryYaml('')` is `undefined`; the port asserts an error
   (`tests/entry_yaml.rs:36`) and the notes do not mention it. Follow what each caller in the source does with
   that `undefined` (`app-boot/src/index.ts:590`, `:659`, `vendor/include/src/index.ts:294`) and make the port's
   result for an empty file, a file of only comments and a `null` document the same at each of those places.
5. **`parse_patches`** (`yaml.rs:446-472`): `id: ~`, `name: ~` and `insert: ~` are errors in the port. In `patch.ts`
   they are falsy and handled by the patch algebra (`if (id)`, `if (insert)`), ending in the warning
   `patch: id is required for non-insert patches`. Do the same. Test: `- {id: ~, config: {}}`.

## YAML writing

6. Strings containing U+007F, U+0085, U+2028, U+2029 or U+FEFF are written raw inside double quotes
   (`yaml.rs:625`). Look at how `lib/dumper.js` escapes non-printable characters and make the writer's output read
   back to the same value by your reader. Test: render and re-read each as a value and as a key.
7. The source tests compare whole dumped texts (`scripts/cordis-entry-yaml.spec.ts:6-20`); the port compares
   substrings and counts (`tests/entry_yaml.rs:7-13`, `:38-47`). Assert the complete expected text of the port's
   writer for expression nodes and for repeated aliases.

## Expressions

8. **`JSON.parse(e)`** uses `serde_json::from_str` (`expr.rs:717-721`). Use `hoh_util_values::js_json_parse`: key
   order, nesting beyond 128 and the error text all differ otherwise. Test: `JSON.parse('{"b":0,"2":0,"1":0}')`
   gives keys `1`, `2`, `b`; 129 nested arrays parse.
9. **`[..].includes(x)`** evaluates the argument before the elements (`expr.rs:663-672`). JavaScript evaluates the
   elements first. Test with a lookup that records the order, or with two absent services and the name in the error.
10. **`+`.** Implement the row of 5.5 as it now reads: a string operand makes it concatenation, two numbers add,
    anything else is an error. The test that pins `1 + true + null == "1truenull"` (`tests/support/expr_eval.rs`)
    pins a wrong result (JavaScript gives 2): replace it.
11. **`??` with `||` or `&&`** without parentheses is a syntax error in JavaScript and must be a parse error with a
    position; the test `escaped_strings_and_operator_precedence_parse` pins the opposite. Check first that none
    of the 76 listed expressions and none of the shipped YAML files needs the loose form; if one does, stop and
    report it instead.
12. **Escapes.** `from_str_radix` accepts a leading `+`, so `'\x+1'` and `'\u+123'` lex as valid escapes
    (`expr.rs:145`, `:150`) and `decode_uri` turns `%+1` into a byte (`expr.rs:912`). JavaScript rejects all three
    (`decodeURIComponent` throws `URIError`). Accept exactly hex digits.
13. **Long chains.** `binary()` builds a left-deep tree without a depth check, so `'a'+'a'+…` with 200,000 terms
    overflows the stack in evaluation, `Clone` or `Drop`. Give an expression a size limit (a parse error with a
    position beyond it) and document the number.
14. **Win32 `resolve`** keeps only drive-letter roots (`expr.rs:877-895`). With platform `win32`, cwd
    `\\server\share\work` and `resolve('/x')`, Node gives `\\server\share\x`; the port gives `\x`. Fix it, add the
    case beside the drive-letter tests, and say in the notes that the win32 path functions were not run on Windows.

## Profile and composition

15. **Manifest.** `profile.rs:66` reads `package.json` with `serde_json::from_str`; the source uses `JSON.parse`. Use
    `js_json_parse`.
16. **`bundles: null`.** The source reads `manifest.dsh?.profile?.bundles ?? []` (`profile.ts:684-686`), so `null`
    means no bundles. The port rejects it (`profile.rs:82-97`) and `tests/profile.rs:154-160` asserts the rejection.
    Follow the source for `null` and for an absent key at each level of that chain; keep rejecting what the
    source rejects.
17. **Telemetry row.** The source looks only at root rows (`profile-context.ts:81-83`,
    `.some(row => row.id === …)`); the port's `contains` descends into groups (`profile.rs:165-179`, `:188-195`).
    With the variable set and the row nested in a group the source emits no launcher patch and the port emits one.
    Follow the source. Test: that composition has no `LayerSource::Launcher`.
18. **Warning texts.** The source formats warnings with a `%C` placeholder
    (`warn('patch: entry %C not found', id)`, `patch.ts:70-98`). Establish from the source how `%C` is rendered where
    these warnings are printed (the `Warn` implementations in `app-boot/src/index.ts` and the logger's formatter)
    and make the port's texts equal to what the source prints, byte for byte. Report the `path:line` you relied on.
19. **Generated ids.** L4 requires an id to be unique in the tree. `assign_ids` (`yaml.rs:403`) checks only within one
    parse call, so ids generated for the base file, for a patch file and for another patch file can collide.
    Guarantee uniqueness in the composed tree and say in the notes where it is enforced.
20. **Origins.** `compose` makes one layer per bundle file, so `patched_by` can name the same bundle twice. Compare
    with `renderConfigDump` in the source and do what it does.
21. **Absolute names.** An absolute `bundle.files` name or bundle name replaces the directory in `join`
    (`profile.rs:114`, `:116`). Do what the source does with such a name; if it accepts it, keep that and say so.

## Tests and notes

22. `tests/user_patches.rs:70-85` checks only `is_err()`: assert the message and the position. The source's
    `profile.spec.ts:201-226` checks telemetry with the variable unset and re-reads the profile after files change;
    `tests/profile.rs:203-250` covers only one static case: add the unset case and repeated `Profile::load` calls
    after edits. `bundle_platform` has no cases for metadata that is not a string: add them at the boundary where
    such data can enter.
23. `PORT_NOTES.md`: correct the statements about `.` and `+.` (`:56`, `:67-70`, `:266`); record every change above
    that differs from the source on purpose (non-finite numbers, alias limits, the expression size limit); record
    the new references.

## Done when

`cargo test -p hoh-loader`, `cargo clippy -p hoh-loader --all-targets -- -D warnings` and
`cargo fmt -p hoh-loader -- --check` are clean. Final report: the result line and one line per item saying what
changed, with the `path:line` of the source you relied on for items 1, 4, 18, 20 and 21; any item you consider
wrong, with the reason; anything unfinished.
