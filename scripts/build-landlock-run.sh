#!/usr/bin/env bash
# Builds `landlock-run` (crates/hoh-node-addon-system) the way it must be shipped: as a static executable,
# checks that the file is one, and runs the launcher's tests against it.
#
# Why static. An ordinary Rust executable starts through the dynamic loader, and the loader reads LD_PRELOAD,
# LD_LIBRARY_PATH and LD_AUDIT and runs the libraries they name BEFORE the launcher restricts itself. Whoever
# can set a variable of the launcher's environment and write one file would run code outside the sandbox.
# The source's launcher is a static executable for the same reason (its contract: "no environment-variable
# inputs"). A static executable has no loader, so nothing in the environment runs ahead of the ruleset.
#
#   scripts/build-landlock-run.sh
# The last line printed is the path of the launcher. Exit 0 only if the build and every check passed.
# Needs the static C library of the system (`libc.a`; on Debian and Ubuntu it comes with `libc6-dev`).
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_NET_OFFLINE=true

host=$(rustc -vV | sed -n 's/^host: //p')
target_dir="${HOH_LANDLOCK_TARGET_DIR:-/mnt/toshiba/alexandrie/hoh/target-landlock-run}"
launcher="$target_dir/$host/release/landlock-run"

# An explicit --target keeps the flag away from build scripts and procedural macros, which stay dynamic.
CARGO_TARGET_DIR="$target_dir" RUSTFLAGS="-C target-feature=+crt-static" \
  cargo build -p hoh-node-addon-system --bin landlock-run --release --target "$host"

if readelf -lW "$launcher" | grep -q 'INTERP'; then
  echo "build-landlock-run: $launcher asks for a dynamic loader" >&2
  exit 1
fi
if readelf -dW "$launcher" 2>/dev/null | grep -q 'NEEDED'; then
  echo "build-landlock-run: $launcher needs shared libraries" >&2
  exit 1
fi

# All tests of the launcher against this file, with the one that only a static launcher passes.
HOH_LANDLOCK_RUN="$launcher" \
  cargo test -p hoh-node-addon-system --test launcher --test entry -- --include-ignored

echo "$launcher"
