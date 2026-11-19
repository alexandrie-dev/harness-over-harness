#!/usr/bin/env bash
# This is the only intentional network step. Never run the build here.
set -euo pipefail
cd "$(dirname "$0")/.."
export XDG_RUNTIME_DIR="${TMPDIR:-/tmp}/hoh-ui2-runtime-$UID"
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
[[ $(pnpm --version) == 12.21.0 ]] || { echo 'pnpm 12.21.0 required' >&2; exit 1; }
../scripts/ui-supply-check.sh --config-only
if [[ ! -f pnpm-lock.yaml ]]; then
  pnpm install --lockfile-only --no-frozen-lockfile --no-offline --ignore-scripts --no-optional --network-concurrency=4
fi
# Check every locked package BEFORE acquiring its archive or executing it.
../scripts/ui-supply-check.sh --refresh
# pnpm 12.21 fetch has no --no-optional flag (verified with fetch --help).
# Materialize with install, whose flags explicitly preserve all acquisition rules.
pnpm install --frozen-lockfile --no-offline --ignore-scripts --no-optional --network-concurrency=4
pnpm install --frozen-lockfile --offline --ignore-scripts --no-optional
../scripts/ui-supply-check.sh
