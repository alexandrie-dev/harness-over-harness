#!/bin/bash -p
set -euo pipefail
unset CDPATH
cd -- "${BASH_SOURCE[0]%/*}/.."

mode=${1:-}
if (( $# > 1 )) || [[ -n "$mode" && "$mode" != --check ]]; then
  printf 'Usage: %s [--check]\n' "$0" >&2
  exit 2
fi

# The distribution checks run in both modes. The first build produces legacy
# and approval pages; T3 then becomes the default entry in the same dist.
exec /usr/bin/env -i /usr/bin/python3 -I scripts/offline.py /bin/bash -peuc '
  ../scripts/ui-supply-check.sh --npm-only --config-only
  pnpm install --frozen-lockfile --offline
  ../scripts/ui-supply-check.sh --npm-only
  ./node_modules/.bin/vite build
  node scripts/check-dist.mjs
  if [[ -f src/t3/base/web/bootstrap.ts ]]; then
    ./node_modules/.bin/vite build --mode t3
    node scripts/check-dist.mjs
  fi
  if [[ $1 == --check ]]; then
    ./node_modules/.bin/tsc --noEmit
    pnpm run test:unit
    if [[ -f src/t3/base/web/bootstrap.ts ]]; then
      node scripts/check-grid.mjs
      node scripts/check-align.mjs
      node scripts/check-first-turn-image.mjs
      node scripts/check-chat-words.mjs
    fi
  fi
' hoh-ui "$mode"
