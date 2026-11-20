# Review: first-run and approval screens (s14, variant A)

You only read. Do not run builds, tests, servers, browsers or any command that writes; do not edit files.
Requirements: `docs/window.md` §1, §2 (what a person never sees), §3–§4 (approval window, first run, protocol methods), §6;
design: variant A `design/round1/a/index.html` + PNGs; wording of the approval window from variant C («почему это опасно»,
the Assistant's explanation); all colours/sizes/fonts only through `ui/src/styles/tokens.css`.
Files: `ui/src/screens/firstrun/*` (adapter `engine.ts`), `ui/src/screens/approval/*` (adapter `bridge.ts`),
`ui/src/screens/shared/*`, `ui/approval.html`, `ui/firstrun.html`, `ui/vite.config.ts`, tests `ui/tests/screens-a/*`,
screenshots `design/build-a/{firstrun*,approval*}.png`. The lead ran screens-a outside the sandbox: 16/0.
Check: (1) API key — never stored in JS state longer than the save call, never in localStorage/sessionStorage/logs/DOM
attributes, password field, cleared after save; (2) approval window — only allow-once / reject, Esc and close = reject,
expired request shows no buttons and never answers for the user, request text and model explanation rendered as text not
markup; no way for the main window to trigger a decision; (3) adapters match the protocol methods in `crates/hoh-protocol`
(names, params, results) — list any mismatch; (4) words a person never sees; (5) literal colours/sizes outside tokens;
(6) first-run steps: model address + key + «Проверить», rules, introduction, back navigation. Triage: bug / requirement
violation / taste (taste does not block).
Output: findings most severe first: kind, file:line, scenario, observed vs required. ≤700 words. Answer in Russian.
