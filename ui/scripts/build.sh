#!/bin/bash -p
set -euo pipefail
unset CDPATH
cd -- "${BASH_SOURCE[0]%/*}/.."
# Clear loader/Python/Node/pnpm settings before even starting the Python runner.
# offline.py constructs the documented allow-list and a disposable HOME.
exec /usr/bin/env -i /usr/bin/python3 -I scripts/offline.py pnpm run build:offline
