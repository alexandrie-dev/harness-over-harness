# c4: fix round for hoh-session-format-v1-to-v2 after review

Sonnet's read-only review of `crates/hoh-session-format-v1-to-v2` (text below in short; full text kept by the lead).
Confirm each against the source before fixing; each fix with a test that fails before it.

1. **bug — cause dropped.** `src/testing/validation.rs:74` uses `error(...)`, but `SessionFormatError::new_with_cause`
   / `FormatWithCause` already exist (`crates/hoh-session-format/src/error.rs:41`). Source
   `src/testing/validation.ts:77-79`. Use it; `tests/validation.rs:281` asserts `source()` is `Some`. Correct your
   PORT_NOTES question 1 (the API was there).
2. **bug — malformed chunks fail earlier than in the source.** `migration.rs:404-409` → `hoh_llm::AssistantStreamAccumulator::push_value`.
   Input: v1 `assistant/chunk` with `chunk:{type:"finish"}` (no `reason`) or `{type:"usage"}` (no `usage`). Source:
   migration succeeds, restore later fails with `SessionFormatError "assistant/attempt N has an invalid embedded
   stream"`; port fails during migration with `Other("Assistant stream chunk must be losslessly JSON-serializable")`.
   w7 is changing `push_value` to check what the source checks (task `plans/briefs/fix-w7-shared-precision.md`);
   once it lands, add the tests through the catalog (`[turn/start, step/start, chunk{finish}]` etc.) and make them
   pass, in this crate only. If w7's change is not in yet when you reach this item, do the rest and say so.
3. **weakened assertions.** Restore the source's regexes as full matches (`unseeded.*inherited events` at
   `tests/codec.rs:43`, `tests/validation.rs:130,221`; `unseeded.*inherited end-seed` at `tests/validation.rs:338,358`
   — the substring also matches the seeded message; `unknown event type.*external/unknown` at `:143`), and make
   `tests/common/mod.rs:49` (`err`) check the error class as well as the text.
4. **missing test:** tie `RELEASED_V2_EVENT_TYPES` (`dispositions.rs:58-110`) to the sorted keys of the dispositions
   table, as `dispositions.ts:32` derives it.
5. **performance:** `RELEASED_V2_EVENT_DISPOSITIONS.get` (`dispositions.rs:12`) rebuilds a 51-entry map per call; build
   it once.

Notes 6–8 of the review (TypeError vs SessionFormatError on unreachable malformed input, deep-equal vs `!==` for a
non-scalar `turn`, inherited names) need no change: record each in PORT_NOTES with why it cannot reach an observer,
or fix it if you find it can.

Same environment as before (`target-c4`). Report: per item confirmed/rejected with lines, result lines. At most 300 words.
