#!/usr/bin/env bash
# Коммит принятого пакета только если ЧИСТАЯ выгрузка коммита-кандидата собирается (CEO 19.11 11:2x).
#   scripts/commit-check.sh "сообщение коммита" PATH...      (пути пакета: crates/<крейт>, docs/porting/MAP.tsv, ...)
#   scripts/commit-check.sh --head-test                       (один раз: чистая выгрузка HEAD, check + test всего workspace)
# Шаги: git add PATH... → кандидат из индекса (HEAD не двигается) → git worktree в tmpfs → Cargo.lock кандидата
# подгоняется офлайн под закоммиченный workspace (cargo update --workspace) и кладётся в индекс, рабочий Cargo.lock
# с чужими правками не трогается → cargo check --workspace --all-targets --locked в выгрузке → коммит или откат индекса.
# Итог одной строкой в logs/commit-check.done.
set -uo pipefail
cd "$(dirname "$0")/.."
H=/mnt/toshiba/alexandrie/hoh; W=/dev/shm/hoh/cc-tree; T=/home/save/.cache/hoh-build/cc-build   # CEO 20.11 17:2x: на USB, tmpfs — писателям. Было: 14:46: в tmpfs (на USB-диске проверка стояла 16+ мин); вытесняется, когда cargo в нём не работает
[[ ${1:-} == --head-test ]] && W=$H/ht-tree   # своя выгрузка: head-test не мешает очереди коммитов
export HOH_PRIORITY=1 CARGO_TARGET_DIR=$T CARGO_BUILD_JOBS=4
export CARGO_HOME=/mnt/toshiba/alexandrie/hoh/cargo-home   # CEO 20.11: кэш crates не на корневом диске
cleanup() { git worktree remove --force "$W" >/dev/null 2>&1; rm -rf "$W"; }   # cc-target остаётся: инкрементальная сборка следующей проверки
trap cleanup EXIT
cleanup
log() { echo "$(date +%H:%M) $*" | tee -a "$H/logs/commit-check.done"; }

if [[ ${1:-} == --head-test ]]; then
  export CARGO_TARGET_DIR=$H/headtest-target   # полный test всего workspace не помещается в tmpfs (Bus error линковщика 13:49)
  rev=$(git rev-parse --short HEAD); git worktree add -q --detach "$W" "$rev" || exit 1   # метка = проверенный коммит, а не HEAD на момент конца
  ( cd "$W" && cargo update --workspace --offline -q && timeout 3600 cargo check --workspace --all-targets --offline --locked -q \
      && timeout 5400 cargo test --workspace --offline --locked --no-fail-fast ) >"$H/logs/head-test.txt" 2>&1; rc=$?
  pass=$(grep -oE '^test result: [a-zA-Z]+\. [0-9]+ passed; [0-9]+ failed' "$H/logs/head-test.txt" | awk '{p+=$4; f+=$6} END{print p"/"f}')
  log "head-test $rev rc=$rc тесты $pass (logs/head-test.txt)"; exit $rc
fi

msg=$1; shift
git add -- "$@" || exit 1
cand=$(git commit-tree "$(git write-tree)" -p HEAD -m tmp) || exit 1
git worktree add -q --detach "$W" "$cand" || exit 1
( cd "$W" && cargo update --workspace --offline -q ) >"$H/logs/commit-check.last.txt" 2>&1 || { log "ОТКАЗ lock: $*"; git reset -q -- "$@"; exit 1; }
blob=$(git hash-object -w "$W/Cargo.lock"); git update-index --cacheinfo "100644,$blob,Cargo.lock"   # выгрузка уже = индекс
if ( cd "$W" && timeout 3600 cargo check --workspace --all-targets --offline --locked ) >>"$H/logs/commit-check.last.txt" 2>&1; then
  git commit -q -m "$msg" && log "КОММИТ $(git rev-parse --short HEAD): $*"
else
  git reset -q -- "$@" Cargo.lock
  log "ОТКАЗ: чистая выгрузка не собирается — $* (logs/commit-check.last.txt)"; exit 1
fi
