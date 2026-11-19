#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../../.." && pwd)
# Never builds or downloads. run-isolated-window.py owns and cleans its temporary HOME.
exec python3 "$root/ui/scripts/run-isolated-window.py" /mnt/toshiba/alexandrie/hoh/target-ui2/debug/hoh-desktop
