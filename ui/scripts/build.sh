#!/bin/bash -p
set -euo pipefail
unset CDPATH
cd -- "${BASH_SOURCE[0]%/*}/.."
# Clear loader/Python/Node/pnpm settings before even starting the Python runner.
# offline.py constructs the documented allow-list and a disposable HOME.
# HOH-40: when the T3 web client is in the tree (src/t3/base, scripts/t3-sync.py), its entry is built second into the
# same dist (src/t3-entry/vite.ts): dist/index.html becomes the T3 client, the previous shell stays as dist/legacy.html.
exec /usr/bin/env -i /usr/bin/python3 -I scripts/offline.py /bin/sh -c 'pnpm run build:offline && if [ -f src/t3/base/web/bootstrap.ts ]; then ./node_modules/.bin/vite build --mode t3 && node scripts/check-dist.mjs; fi'
