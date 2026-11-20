#!/usr/bin/env bash
# CEO 20.11 ~19:00: после u1d (правки P2) — снимок для q1, короткий QA семи пунктов, и параллельно u1 ставит npm T3.
# Снимок берётся до установки npm, чтобы QA проверял ровно правки u1d.
set -u; cd "$(dirname "$0")/.."
L=/mnt/toshiba/alexandrie/hoh/logs/lead1118
timeout 14400 bash -c 'while systemctl --user is-active -q hoh-chain-u1d; do sleep 60; done'
head -1 /mnt/toshiba/alexandrie/hoh/logs/u1.window-qa-fix2.report.md | grep -q '^ГОТОВ' || { echo "$(date +%H:%M) NEEDS-LEAD u1d без ГОТОВ — q1/npm не запущены" >>/mnt/toshiba/alexandrie/hoh/pipeline/attention.log; exit 1; }
bash scripts/qa-snapshot.sh >"$L/q1-snapshot.sha" || exit 1
systemd-run --user --collect --unit hoh-chain-q1c --setenv=PATH="$PATH" --setenv=HOME="$HOME" --working-directory="$PWD" \
  bash scripts/claude-chain.sh q1 claude-sonnet-5-5 "$L/q1-recheck2.txt" window-qa-recheck2
exec bash scripts/claude-chain.sh u1 claude-sonnet-5-5 "$L/u1-npm-t3.txt" npm-t3
