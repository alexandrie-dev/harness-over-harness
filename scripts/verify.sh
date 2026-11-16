#!/usr/bin/env bash
# The single final check of the port: formatting, lints, tests, recorded scenarios.
#   scripts/verify.sh            full check of the workspace; exit 0 only if every phase ran and passed
#   scripts/verify.sh CRATE...   formatting, lints and tests of the named crates only; never a final check
# Reports go to $HOH_REPORT_DIR (default: /mnt/toshiba/alexandrie/hoh/reports/<time>).
set -uo pipefail
cd "$(dirname "$0")/.."
export CARGO_NET_OFFLINE=true

report="${HOH_REPORT_DIR:-/mnt/toshiba/alexandrie/hoh/reports/$(date +%Y%m%d-%H%M%S)}"
mkdir -p "$report"
say() { echo "$*" | tee -a "$report/summary.txt"; }

select=(--workspace)
fmt_select=(--all)
if (($# > 0)); then
  select=()
  for crate in "$@"; do select+=(-p "$crate"); done
  fmt_select=("${select[@]}")
fi

child=
trap 'kill "$child" 2>/dev/null; wait "$child" 2>/dev/null; say "ИТОГ: прервано"; exit 130' INT TERM

failed=()
phase() { # name command...
  local name=$1
  shift
  "$@" >"$report/$name.log" 2>&1 &
  child=$!
  if wait "$child"; then
    say "$name: ok"
  else
    say "$name: не пройдено, см. $report/$name.log"
    failed+=("$name")
  fi
}

phase fmt cargo fmt "${fmt_select[@]}" -- --check
phase clippy cargo clippy "${select[@]}" --all-targets -- -D warnings
phase test cargo test "${select[@]}"

read -r passed broken < <(awk '/^test result:/ { p += $4; f += $6 } END { print p + 0, f + 0 }' "$report/test.log")
say "тесты: прошло $passed, упало $broken"
if ((passed == 0)); then
  say "test: ни один тест не выполнен"
  failed+=(test-empty)
fi

if (($# == 0)); then
  # The scenario runner (crates/hoh-conformance) is not written yet. Until it is, a full check cannot pass.
  say "scenarios: прогонщика сценариев ещё нет"
  failed+=(scenarios)
  bash scripts/port-status.sh | tail -2 | tee -a "$report/summary.txt"
fi

if ((${#failed[@]} > 0)); then
  say "ИТОГ: не пройдено (${failed[*]}); отчёты: $report"
  exit 1
fi
if (($# > 0)); then
  say "ИТОГ: частичная проверка пройдена ($*); отчёты: $report"
else
  say "ИТОГ: пройдено; отчёты: $report"
fi
