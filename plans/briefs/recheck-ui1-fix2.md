# Closure check: hoh-protocol and hoh-window-stub after fix2 (ui1)

You only read. Do not run builds, tests, servers or any command that writes; do not edit files.
Fix brief with the review items and the lead's addition (new methods): `plans/briefs/fix2-ui1-protocol.md`.
Writer's report: `/mnt/toshiba/alexandrie/hoh/logs/ui1.fix2d.report.md`. Code: `crates/hoh-protocol`, `crates/hoh-window-stub`,
generated `ui/src/protocol.ts`, `docs/window.md` §4, `scripts/gen-protocol-ts.mjs`.
For each of the 8 items: closed / not closed with file:line. For the new methods `hoh/model/key/set`,
`hoh/settings/rules/set`: privileged, MAC-approved, the key never appears in logs/Debug/frames/approval text (only a hash),
stored via hoh-credentials only after approval; what happens on rejection/expiry. Also answer one question for the window:
does `model/probe` (or equivalent) return the list of model names the provider offers — yes/no, with the field name.
Any new divergence or fail-open introduced. ≤600 words, Russian.
