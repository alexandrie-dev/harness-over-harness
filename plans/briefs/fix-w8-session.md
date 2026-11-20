# w8: fix round for hoh-session after review

Sonnet's read-only review of `crates/hoh-session` (source is `packages/core/session`). Confirm each item against
the source before fixing; each fix with a test that fails before it. Same environment as before (`target-w8`).

1. **Projection context on a single append.** `src/session.rs:452-455` passes `log + new events` to `fold.apply`
   for both single and batch appends. Source `surface.ts:530` with `validateNext`: on a single append `ctx.events`
   is the log without the candidate; candidates are in the context only in `prepareBatch`. Test: a projection
   records `ctx.events.len()` against `event.seq()` on `append` and on `append_batch`.
2. **Veto in raw `session/created` listeners.** `store.rs:222`: after a listener returns `Err`, later raw
   `on_contained::<SessionCreated>` listeners still run; source `index.ts:1378-1390` stops at the throw. Make the
   raw path stop as the source does (if that needs a kernel facility, stop and report). Test with two raw listeners.
3. **Error order in a batch.** `session.rs:409-449` snapshots and validates member by member; source
   `index.ts:882-886` snapshots all members first, then validates. Input: `append_batch([invalid by validate,
   member carrying -0.0])` → source reports member 1's non-JSON-serializable error.
4. **Panic in repair.** `types.rs:318-323` `seq()`/`time()` `expect`, reached from `ToolCallRecovery::observe` and
   `open_turn_closers` on a damaged tail: `interrupted_turn_closers(&[{"type":"turn/start","seq":-1,"time":0,
   "data":{"turn":1}}])` — source returns `turn/end` with seq 0, the port panics. No panics on log input anywhere in
   repair/fork; follow the source.
5. **Cost of append.** `session.rs:404` clones the whole log and `FoldState` and copies `history` on every append:
   O(n) per event, quadratic per session (source O(1)). Make append amortised O(1) in the log length while keeping
   the source's rollback-on-failure semantics; a test that appends 100 000 small events within a time bound that a
   quadratic version cannot meet (state the bound and the machine assumption).
6. **Missing tests:**
   - depth: `append`, seed, `fork`, `derive_messages` on data nested 4096 deep, on the default test thread stack
     (2 MiB) — no stack overflow in `Clone`, `Drop`, `deep_equal_json` or copying; if it overflows, fix it.
   - pin `KNOWN_SESSION_EVENT_TYPES` (64 sorted names) and `MESSAGE_PROJECTION_EVENT_TYPES` (`image/offload` only
     there) against the source list.
7. **Weakened assertions** (restore the source's): `fork.rs` "last seq: N" for a non-empty log; the texts
   `not found`, `not the live store instance`, `already exists`; the forked `turn/end` envelope and the `end-seed`
   marker compared in full and byte for byte (key order — `Value ==` ignores it); parents built by live appends as
   in the source, not `seed:` (no extra `end-seed {}`); `append.rs` the reentrant-append warning
   `session append cannot reenter`; `repair.rs` "preserves unanswered call after closed step" compares the full
   object including `data.reason`.

Note 5 of the review (`version: 4.0` as F64) — readers parse through `js_json_parse`; record it in PORT_NOTES.

Report: per item confirmed/rejected with lines, what changed, result lines. At most 300 words.
