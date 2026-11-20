You only read. Do not run builds, tests, servers, browsers or any command that writes; do not edit or create files. Write in Russian.

Project HoH, working tree /home/save/Alexandrie/HarnessOverHarness (uncommitted changes vs HEAD). Subject: fix of QA finding A-03
by writer sg2 — sections «fix A-03» and «details» of /mnt/toshiba/alexandrie/hoh/logs/sg2-guard.glue.sonnet.md.
Changed areas: crates/hoh-window-destructive-guard/**, crates/hoh-user-approval/**, crates/hoh-protocol/** (+ protocol/schema,
protocol/core.ts), apps/hoh/src/window_guard.rs and apps/hoh/tests/{window_guard,window_e2e}.rs. Use `git diff` mentally via the files.
QA finding: /mnt/toshiba/alexandrie/hoh/logs/qa-window-chat.A.report.md, A-03.

Requirements to check:
1. The confirmation's main text (reason) is human Russian with the file list and never contains the raw command; the raw command
   goes only into the new optional `details` field (delete, overwrite `>`/`sed -i`, escalation).
2. `details` is optional, absent from JSON when None, old JSON still parses; it is NOT part of action/actionHash/mac, yet cannot be
   used to show the person something different from what is signed in a way that misleads (assess: is details derived from the same
   command that action describes?).
3. IMPORTANT — the guard no longer returns PreToolDecision::Ask but calls approval.request directly. Verify nothing was lost versus
   the previous waterfall path (service_ask): same outcomes and refusal codes, single question per action, allowed-once/deny/repeat
   semantics, the one-time 5-minute escalation grant (pre_approve_escalation) and the policy_switch lock, behaviour without an
   agent/host, cancellation/timeouts, journal entries, ordering with other pre-tool hooks (could another hook now be skipped or run
   after a denied action?), and that ordinary safe commands still produce no question.
4. Tests prove what is claimed and would fail on the old code.
Each finding: scenario, deviation, evidence file:line, level (blocker / error / note). Taste is not a finding.
The LAST line must be exactly 'ВЕРДИКТ: ЧИСТО' or 'ВЕРДИКТ: ЗАМЕЧАНИЯ'.
