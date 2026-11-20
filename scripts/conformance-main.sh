#!/usr/bin/env bash
# Полный прогон сценариев на main после слияний (CEO 20.11 19:3x, п. 7): выгрузка HEAD в отдельное дерево,
# свой каталог сборки, по умолчанию 2 задания (HOH_CONFORMANCE_JOBS). Итог — новая папка conformance-runs/<время>,
# её читает scripts/port-status.sh. Запуск: systemd-run --user --collect --unit hoh-conformance-main ... bash scripts/conformance-main.sh
set -uo pipefail
G=$(cd "$(dirname "$0")/.." && pwd); cd "$G"
H=/mnt/toshiba/alexandrie/hoh; D=$H/it/conf-main
# CEO 20.11 19:47: только когда нет сборок приёмки и прогонов ведущего — иначе таймауты прогона ничего не значат
busy() { local f; for f in /proc/[0-9]*/environ; do [[ $(cat "${f%/environ}/comm" 2>/dev/null) == cargo ]] || continue
  tr '\0' '\n' <"$f" 2>/dev/null | grep -qE '^(CARGO_TARGET_DIR=.*/(target-accept-|lead-pl-build|cc-build)|HOH_PRIORITY=1$)' && return 0; done; return 1; }
while busy; do sleep 60; done
bash scripts/pkg.sh checkout "$D" "$(git rev-parse HEAD)" || exit 1
echo "$(date +%H:%M) прогон сценариев на main $(git rev-parse --short HEAD)"
CARGO_HOME=$H/cargo-home CARGO_TARGET_DIR=$H/target-conf-main HOH_FULL_RUN=1 bash "$D/scripts/conformance-v1.sh" all
