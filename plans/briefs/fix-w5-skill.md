# w5: fix round for hoh-skill after review

Sonnet's read-only review of `crates/hoh-skill` found no divergence in text the model sees. Fix the following;
confirm each against the source first; each change with a test that fails before it.

1. **Weakened assertions (PORTING §10).** Restore the source's assertions exactly:
   - `validates_provider_candidates` (`tests/skill.rs:707`): `Bad_Name` must assert `invalid skill name`, the NaN
     rank case `invalid rank`.
   - `registers_providers…` (`:169`): the `[name, description, provider]` triple including `provider == "override"`
     for `shadowed`.
   - `bounds_repeated…` (`:1021`): the mid-test `listCalls == 2` and the full-shape equality.
   - `sorts_model_visible_summaries` (`:743`): the equal-name tie check.
   - `returns_an_invocation_neutral_catalog…` (`:283`): the default `invocation` on the listed summary.
2. **A test that cannot observe its claim** (`tests/skill.rs:476-497` vs `src/lib.rs:745`, source `spec.ts:304-338`,
   `index.ts:506`): deleting line 745 leaves it green, because the lazy future aborts at the first `check_abort`.
   Make the test reach the post-cache recheck (e.g. multi-thread runtime with a barrier), or, if no interleaving can
   reach it in Rust, record that in PORT_NOTES with the reason and keep line 745.
3. **Untyped records** (`SkillRegistration::from_value` etc.): since the port accepts arbitrary JSON there, follow
   the source for such input: `SKILL_NAME.test(123)` coerces to `"123"` (`index.ts:745`); `String(new Error(''))` is
   `"Error"` (`index.ts:864`, port `lib.rs:961`); a missing `source` prints `undefined` (`lib.rs:844`);
   `is_model_invocable`/`is_user_invocable` return the raw value without an error (`index.ts:130`, `lib.rs:258`).
   Where the Rust return type cannot carry the raw value, record why in PORT_NOTES.
4. `Config.collect_cache_max_entries: f64`: check that every place the config is written as JSON text renders
   `128`, not `128.0` (PORTING §4); test it.

Only `crates/hoh-skill`. Environment as before (`target-w5`). Report: per item what changed, result lines. At most
250 words. After this round, continue with `hoh-skill-office` from your previous assignment (settings still waits
for loader APIs that l1 is adding).
