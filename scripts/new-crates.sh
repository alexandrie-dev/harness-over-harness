#!/usr/bin/env bash
# Creates the skeleton of every package that is ready to be handed to a writer and has no crate yet:
# a manifest that lists the HoH crates the source package depends on, and an empty src/lib.rs.
#   scripts/new-crates.sh [STAGE] [CRATE...]   only the named crates when any are given
# "Ready" is what `scripts/port-status.sh ready` prints; HOH_ASSUME_DONE is passed through to it.
# Third-party crates are not added here: a writer takes them from [workspace.dependencies] by name.
set -euo pipefail
cd "$(dirname "$0")/.."

stage="${1:-}"
[[ $# -gt 0 ]] && shift
only=" $* "

bash scripts/port-status.sh ready $stage | while IFS=$'\t' read -r crate _src _tests deps; do
  [[ "$crate" == hoh-* ]] || continue
  [[ "$only" == "  " || "$only" == *" $crate "* ]] || continue
  [[ -e "crates/$crate" ]] && continue
  row=$(awk -F'\t' -v c="$crate" '$5 == c { print $3 "\t" $4; exit }' docs/porting/MAP.tsv)
  path=${row%%$'\t'*}
  name=${row##*$'\t'}
  mkdir -p "crates/$crate/src"
  {
    printf '[package]\nname = "%s"\ndescription = "Port of %s (%s)"\n' "$crate" "$name" "$path"
    printf 'version.workspace = true\nedition.workspace = true\nrust-version.workspace = true\npublish.workspace = true\n'
    printf '\n[lints]\nworkspace = true\n\n[dependencies]\n'
    for dep in $deps; do
      [[ "$dep" == - ]] || printf '%s = { path = "../%s" }\n' "$dep" "$dep"
    done
  } >"crates/$crate/Cargo.toml"
  printf '//! Port of %s (%s).\n' "$name" "$path" >"crates/$crate/src/lib.rs"
  echo "$crate: $path"
done
