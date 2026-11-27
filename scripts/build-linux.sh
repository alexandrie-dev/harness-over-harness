#!/bin/bash -p
set -euo pipefail
unset CDPATH
cd -- "${BASH_SOURCE[0]%/*}/.."

case "${1:-build}" in
  prepare)
    [[ $# == 1 ]] || exit 2
    tools="$PWD/ui/.supply-cache/bin"
    mkdir -p -- "$tools"
    for tool in node pnpm; do
      override="HOH_${tool^^}"
      executable=${!override:-$(command -v "$tool" || true)}
      if [[ -z "$executable" || ! -f "$executable" || ! -x "$executable" ]]; then
        printf 'Install %s or set %s to its executable.\n' "$tool" "$override" >&2
        exit 1
      fi
      if [[ ! "$executable" -ef "$tools/$tool" ]]; then
        ln -sfnT -- "$(realpath -e -- "$executable")" "$tools/$tool"
      fi
    done
    bash -p ui/scripts/acquire.sh
    cargo fetch --locked --target host-tuple
    cargo fetch --locked --target host-tuple --manifest-path apps/hoh-desktop/Cargo.toml
    ;;
  build|check)
    [[ $# -le 1 ]] || exit 2
    build_dir=$(realpath -m -- "${HOH_BUILD_DIR:-target}")
    ui_args=()
    [[ ${1:-build} != check ]] || ui_args+=(--check)
    bash -p ui/scripts/build.sh "${ui_args[@]}"
    export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2} CARGO_INCREMENTAL=${CARGO_INCREMENTAL:-0}
    cargo build --frozen --release -p hoh --target-dir "$build_dir/engine"
    HOH_ENGINE_PATH="$build_dir/engine/release/hoh" cargo build --frozen --release \
      --manifest-path apps/hoh-desktop/Cargo.toml --target-dir "$build_dir/desktop"
    ;;
  *)
    printf 'Usage: %s [prepare|build|check]\n' "$0" >&2
    exit 2
    ;;
esac
