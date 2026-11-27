#!/usr/bin/env bash
# The single final check of the port: formatting, lints, tests, recorded scenarios.
#   scripts/verify.sh            full check of the workspace; exit 0 only if every phase ran and passed
#   scripts/verify.sh CRATE...   formatting, lints and tests of the named crates only; never a final check
# Reports go to $HOH_REPORT_DIR (default: /mnt/toshiba/alexandrie/hoh/reports/<time>).
set -uo pipefail
cd "${HOH_TREE:-$(dirname "$0")/..}"   # CEO 20.11 19:3x: приёмка в выгрузке коммита пакета
export CARGO_NET_OFFLINE=true
export CARGO_HOME=${CARGO_HOME:-/mnt/toshiba/alexandrie/hoh/cargo-home}   # CEO 20.11: кэш crates не на корневом диске

# Read-only window artifact check. This mode exits before report setup or cargo.
window_artifacts() {
  /usr/bin/python3 -I - "$@" <<'PY_ARTIFACTS'
from pathlib import Path
import sys

try:
    dist = Path(sys.argv[1])
    binary = Path(sys.argv[2]) if len(sys.argv) == 3 else None
    if not dist.is_dir() or dist.is_symlink():
        raise ValueError('window artifacts: dist missing or symlink')
    files = []
    for entry in sorted(dist.rglob('*')):
        if entry.is_symlink():
            raise ValueError('window artifacts: symlink ' + str(entry))
        if entry.is_file():
            files.append(entry)
    if not files:
        raise ValueError('window artifacts: empty dist')
    if binary is not None and (not binary.is_file() or binary.is_symlink() or not binary.stat().st_size):
        raise ValueError('window artifacts: final binary missing/empty or symlink')
    needles = [b'picomatch', 'picomatch'.encode('utf-16le'), 'picomatch'.encode('utf-16be')]
    overlap = max(map(len, needles)) - 1
    for path in [*files, *([binary] if binary is not None else [])]:
        with path.open('rb') as stream:
            tail = b''
            while chunk := stream.read(65536):
                data = tail + chunk
                if any(needle in data.lower() for needle in needles):
                    raise ValueError('window artifacts: picomatch found in ' + str(path))
                tail = data[-overlap:]
    print('passed: no picomatch in dist (including maps)' + (' or final Tauri binary' if binary is not None else ''))
    if binary is None:
        print('skipped: Tauri binary not supplied (not built in this run)')
except (OSError, ValueError) as error:
    print(str(error), file=sys.stderr)
    sys.exit(1)
PY_ARTIFACTS
}
if [[ ${1:-} == --window-artifacts-only ]]; then
  if (($# != 2 && $# != 3)); then
    echo 'usage: scripts/verify.sh --window-artifacts-only DIST [TAURI_BINARY]' >&2
    exit 2
  fi
  window_artifacts "$2" "${@:3}"
  exit $?
fi

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
phase clippy cargo clippy "${select[@]}" --all-targets --no-deps -- -D warnings   # 19.11: lint the selected crates only; a dependency in progress must not fail them
phase test cargo test "${select[@]}"
# The launcher confines only as a static executable: build it that way and test that file too.
if (($# == 0)) || [[ " $* " == *" hoh-node-addon-system "* ]]; then
  phase landlock-run bash scripts/build-landlock-run.sh
fi

read -r passed broken < <(awk '/^test result:/ { p += $4; f += $6 } END { print p + 0, f + 0 }' "$report/test.log")
say "тесты: прошло $passed, упало $broken"
if ((passed == 0)); then
  say "test: ни один тест не выполнен"
  failed+=(test-empty)
fi

if (($# == 0)); then
  phase ui bash -p ui/scripts/build.sh --check
  if command -v cargo-deny >/dev/null; then
    phase ui-rust-licenses cargo deny --manifest-path apps/hoh-desktop/Cargo.toml --locked --offline check --disable-fetch
  fi
  phase window-artifacts window_artifacts ui/dist "${HOH_DESKTOP_BINARY:-${HOH_BUILD_DIR:-target}/desktop/release/hoh-desktop}"
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
