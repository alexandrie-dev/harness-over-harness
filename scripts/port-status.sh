#!/usr/bin/env bash
# Prints the two progress numbers of the port: crates done and scenarios passing.
set -euo pipefail
cd "$(dirname "$0")/.."

awk -F'\t' '
  NR == 1 { next }
  $1 != 9 { total[$1]++; if ($8 == "done") done[$1]++; all++; if ($8 == "done") alldone++ }
  END {
    for (s = 0; s <= 5; s++) printf "этап %d: %d из %d пакетов\n", s, done[s] + 0, total[s] + 0
    printf "пакеты: %d из %d\n", alldone + 0, all + 0
  }' docs/porting/MAP.tsv

if [[ -f conformance/EXPECTED.tsv ]]; then
  awk -F'\t' '
    NR == 1 { for (i = 1; i <= NF; i++) if ($i == "status") col = i; next }
    { n++; by[$col]++ }
    END {
      printf "сценарии: %d из %d проходят", by["pass"] + 0, n
      for (k in by) if (k != "pass") printf ", %s %d", k, by[k]
      printf "\n"
    }' conformance/EXPECTED.tsv
else
  echo "сценарии: conformance/EXPECTED.tsv ещё нет"
fi
