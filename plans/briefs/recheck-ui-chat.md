# Closure check: window chat fixes (ui2, round 2)

You only read. Do not run builds, tests, servers, browsers or any command that writes; do not edit files.
Previous review: `/mnt/toshiba/alexandrie/hoh/logs/rv-ui-chat.full.md`. Lead's decisions for this round:
`/mnt/toshiba/alexandrie/hoh/logs/lead1118/ui2-chat4.txt` (item 4 narrowed to window-generated texts only; item 5 not required).
Files: `ui/src/screens/chat/*`, `ui/src/lib/*`, `ui/src/styles/tokens.css`, `ui/tests/chat/*`, README `ui/tests/chat/README.md`.
The lead ran the browser checks outside the sandbox: behavior 8/0 (first-run-to-chat, text-stream, engine-restart, work-steps,
cancel, safe-markdown, paths-and-window-language, 5000-messages), screens-a 16/0, screenshots `design/build-a/chat-1440*.png`.
For items 1, 2, 3, 4 (narrowed), 6: closed / not closed with file:line, and whether the test would fail on the old code.
Also: did the 19 `--chat-*` variables move into tokens.css unchanged; any literal colour/size left in components; any new
path/word leak introduced by the fixes. ≤500 words, Russian.
