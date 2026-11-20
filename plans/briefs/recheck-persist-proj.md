# Closure check: hoh-session-persistence and hoh-session-projection after fix round (s12)

You only read. Do not run builds, tests, servers or any command that writes; do not edit files.
The previous review found 4 bugs (brief: `plans/briefs/review-hoh-session-persist-proj.md`, fix brief:
`plans/briefs/fix-w3-persist-proj.md`). The writer reports them fixed: (1) registration handed off while the owner was active
stays fulfilled after unload (`registration.rs`, `registry.rs`, OnceLock instead of `delivered`); (2) headers with integral
floats `createdAt: 1.0` / `delegationDepth: 2.0` accepted (`typed_header` in `registry.rs`); (3) `createdAt` validated after
`extra` merge (`storage_contract.rs`); (4) `"__proto__"` key omitted in `checkpoint()`, not reproduced in `restore` (PORT_NOTES).
For each item: closed / not closed, with Rust file:line and the TS source line
(`/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/session/...`), and whether the new test would fail on the old code.
Also: does the fix introduce new divergence (e.g. `typed_header` accepting 1.5 or > MAX_SAFE_INTEGER; a spawned task in
`register()` racing with drop; `restore` with `__proto__`). ≤500 words, Russian.
