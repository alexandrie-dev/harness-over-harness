# Closure check: first-run and approval screens after fix rounds (ui2)

You only read. Do not run builds, tests, servers, browsers or any command that writes; do not edit files.
Review being closed: `/mnt/toshiba/alexandrie/hoh/logs/rv-screens-a.full.md` (P1 + 6 P2 + P3). Lead's decisions:
`/mnt/toshiba/alexandrie/hoh/logs/lead1118/ui2-screens1.txt`. The lead ran outside the sandbox: screens-a 22/0 (after changing
one assertion at `ui/tests/screens-a/run.mjs:371` from exact text match to `getByRole('status').getByText('Отклонено')`, because
the status element also contains a <small> note), chat behavior 8/0.
For items 1–8: closed / not closed with file:line, and whether the test would fail on the old code. Check especially: the API
key is never kept in React state or any storage; expired requests never show actions and decide() re-checks time; Esc/pagehide
after a send error still send `rejected`; the wizard cannot advance on stale data; model name only in «Дополнительно»; the
lead's one-line test change does not weaken what the test proves. ≤500 words, Russian.
