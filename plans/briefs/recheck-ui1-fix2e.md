# Closure check: hoh-protocol, hoh-window-stub, hoh-credentials after fix2e (ui1)

You only read. Do not run builds, tests, servers, browsers or any command that writes; do not edit files.
Task given to the writer: `/mnt/toshiba/alexandrie/hoh/logs/lead1118/ui1-fix2e.txt` (items а–г). Previous closure check:
`/mnt/toshiba/alexandrie/hoh/logs/rc-ui1.full.md`. Writer's report: `/mnt/toshiba/alexandrie/hoh/logs/ui1.fix2e3.report.md`.
Code: `crates/hoh-protocol`, `crates/hoh-window-stub`, `crates/hoh-credentials/src/lib.rs`, generated `ui/src/protocol.ts`,
schemas/fixtures, `docs/window.md`, `scripts/gen-protocol-ts.mjs`. The lead runs the build and browser tests separately.

For each item а–г: closed / partly / not closed, file:line, the test that proves it and whether it would fail on the old code.
- а: `ResolvedCredential` Debug hides the value; nothing else in hoh-credentials changed behaviour.
- б: humanText of new approvals concrete per docs/window.md:142 (rules/set: folder and internet mode in words; key/set:
  provider address, never the key; probe with baseUrl: the address).
- в: a signed rejection is published as `rejected`, the effect is not applied.
- г: `ModelProbeResult.models: string[]` in Rust, schemas, fixtures, TS; empty when the provider gives none; the stub
  returns a test list.
Also: any new path where the API key reaches a frame, log, Debug, approval text or fixture; any fail-open. ≤500 words, Russian.
