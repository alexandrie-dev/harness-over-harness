# Review: window chat screen (package 5) — ui2

You only read. Do not run builds, tests, servers, browsers or any command that writes; do not edit files.
Requirements: `docs/window.md` §1, §2 (what a person never sees), §6 package 5; brief `plans/briefs/window-ui3-chat.md`;
design decision: variant A, all colours/sizes/radii/fonts only through `ui/src/styles/tokens.css` (temporary
`ui/src/screens/chat/chat-tokens.css` allowed, listed for the lead). Files: `ui/src/lib/store.ts`, `ui/src/lib/markdown.tsx`,
`ui/src/screens/chat/*`, `ui/src/main.tsx`, tests `ui/tests/chat/*`, report `ui/tests/chat/README.md`,
screenshot `design/build-a/chat-1440.png` vs reference `design/round1/a/main-1440.png`.
Check: (1) safety — Markdown never executes HTML, links/images inert, no `dangerouslySetInnerHTML`, no eval; text from the
engine never reaches DOM as HTML; (2) words a person must never see (JSON, paths outside the user folder, «токен»,
«сессия», «агент», «инструмент», English tool names) — including error and fallback texts; (3) streaming and cancel:
`session/cancel` sent once, no lost or duplicated text on cancel/reconnect, ordering of steps vs text; (4) literal colours or
sizes in components (any hex/rgb/px outside token files); (5) step lines: read/write/command in plain words, exit code in
words, expandable short content; (6) the 5000-message test really measures what it claims. Triage: bug / requirement
violation / taste (mark taste separately, it does not block).
Output: findings most severe first: kind, file:line, scenario, observed vs required. ≤700 words. Answer in Russian.
