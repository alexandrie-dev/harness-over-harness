# Independent review: JavaScript-compatible JSON in `hoh-util-values`

You are a reviewer. Read-only: do not edit, create or delete files; do not run tests, builds, cargo, Node, servers
or browsers; no network; no git command that changes state. Working directory:
`/home/save/Alexandrie/HarnessOverHarness`.

## What is reviewed

`crates/hoh-util-values/src/js_json.rs` (about 600 lines, written by another model family) and its tests
`crates/hoh-util-values/tests/js_json.rs`, with `crates/hoh-util-values/PORT_NOTES.md`. Three functions that all
ported code will use wherever the TypeScript source calls `JSON.stringify` or `JSON.parse` and the text reaches a
model, a session log, stdout or a file:

- `js_json_stringify(value)` is `JSON.stringify(value)`, `js_json_stringify_pretty(value, n)` is
  `JSON.stringify(value, null, n)`;
- `js_json_parse(text)` is `JSON.parse(text)`, with V8's error messages, and a deliberate cap of 4,096 nesting levels
  (`JS_JSON_MAX_DEPTH`, message `Maximum call stack size exceeded`).

The rule that makes them mandatory is in `docs/porting/PORTING.md` section 4 (the bullets about JSON text and
recursion). The contract the writer was given: numbers printed as JavaScript prints doubles; integers beyond 2^53
rounded as JavaScript would; `-0` printed as `0`; strings escaped exactly as `JSON.stringify` does; object members
in JavaScript property order (integer-like keys `0`..`4294967294` first, ascending, then the rest in stored
order), both when writing and in the parsed result; a duplicate key keeps its first position and takes the last
value; no recursion per nesting level in either function; an escaped lone surrogate becomes U+FFFD and a number
overflowing to Infinity becomes `null` (both documented differences).

Already established by the lead, do not repeat: the crate's tests, clippy and formatting pass; two fixtures
generated with Node 20 pass on every line (`tests/fixtures/js_json_roundtrip.jsonl`, 2,088 lines;
`tests/fixtures/js_json_parse_errors.jsonl`, 1,799 lines; generator `tests/fixtures/gen-js-json.mjs`).

## What to look for

1. Inputs outside the fixtures where the result would differ from JavaScript. Read the generator to see what the
   fixtures cover, then reason about what they do not: long strings, astral characters and positions counted in
   UTF-16 units in error messages, the length rules of the quoted context in V8's messages, numbers with many
   digits or huge exponents, keys that look almost like integers, deep or wide values, whitespace forms, input
   ending inside an escape or a number, NUL and other control characters.
2. Panics and overflows: slicing on a non-character boundary, indexing, arithmetic on positions, a failure path
   that drops a deep value recursively, the depth counter.
3. Cost: anything quadratic in the input size (key reordering, duplicate keys, building the error context).
4. Places where the code still recurses once per nesting level.
5. False or incomplete statements in `PORT_NOTES.md` about these functions.
6. Tests that would pass with a wrong implementation.

Do not report style or naming.

## Report

In English, at most 600 words, as your final message. Findings numbered, most serious first; for each: class
(`bug`, `hazard`, `missing test`, `false note`, `note`), a concrete input, what JavaScript gives, what this code
gives (`file:line`), and a test that would confirm it. If you are not sure, say what would settle it. Then
"Checked with no findings" and "Not checked".
