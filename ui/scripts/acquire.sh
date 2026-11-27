#!/bin/bash -p
# The explicit network step for dependency acquisition.
set -euo pipefail
unset CDPATH
cd -- "${BASH_SOURCE[0]%/*}/.."

record=${1:-}
if (( $# > 1 )) || [[ -n "$record" && "$record" != --record ]]; then
  printf 'Usage: %s [--record]\n' "$0" >&2
  exit 2
fi
exec /usr/bin/env -i /usr/bin/python3 -I scripts/offline.py --acquire /bin/bash -peuc '
  ../scripts/ui-supply-check.sh --npm-only --config-only
  if [[ ! -f pnpm-lock.yaml ]]; then
    pnpm install --lockfile-only --no-frozen-lockfile --no-offline --ignore-scripts --no-optional --network-concurrency=4
  fi

  report=(--local)
  [[ $1 != --record ]] || report=()
  ../scripts/ui-supply-check.sh --npm-only --refresh --before-install "${report[@]}"
  pnpm install --frozen-lockfile --no-offline --ignore-scripts --no-optional --network-concurrency=4
  pnpm install --frozen-lockfile --offline --ignore-scripts --no-optional
  ../scripts/ui-supply-check.sh --npm-only
' hoh-acquire "$record"
