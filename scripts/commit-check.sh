#!/usr/bin/env bash
# Коммит принятого пакета только если ЧИСТАЯ выгрузка коммита-кандидата собирается (CEO 19.11 11:2x).
#   scripts/commit-check.sh [--pkg C] "сообщение коммита" PATH...   (пути пакета: crates/<крейт>, apps/hoh, docs/porting/MAP.tsv)
#   scripts/commit-check.sh --head-test                              (чистая выгрузка HEAD, check + test всего workspace)
# CEO 20.11 19:3x: коммитятся ТОЛЬКО перечисленные пути. Кандидат собирается во временном индексе от HEAD:
#   - пути пакета берутся из коммита пакета C (--pkg; то, что прошло прогон, ревью и приёмку), без --pkg — из общего дерева;
#   - docs/porting/MAP.tsv — из общего дерева (файл интеграции);
#   - Cargo.lock подгоняется офлайн (cargo update --workspace) в чистой выгрузке кандидата.
# Индекс общего дерева не используется: чужие застейдженные файлы в коммит попасть не могут. Перед коммитом проверка:
# изменённые относительно HEAD пути ⊆ PATH... ∪ {Cargo.lock}; иначе отказ. Итог одной строкой в logs/commit-check.done.
set -uo pipefail
cd "$(dirname "$0")/.."
G=$PWD
H=/mnt/toshiba/alexandrie/hoh; W=/dev/shm/hoh/cc-tree; T=$H/cc-build   # CEO 20.11 19:17: NVMe ≤25 ГБ — проверка на toshiba
[[ ${1:-} == --head-test ]] && W=$H/ht-tree   # своя выгрузка: head-test не мешает очереди коммитов
export HOH_PRIORITY=1 CARGO_TARGET_DIR=$T CARGO_BUILD_JOBS=4
export CARGO_HOME=/mnt/toshiba/alexandrie/hoh/cargo-home   # CEO 20.11: кэш crates не на корневом диске
idx=$(mktemp -p /dev/shm/hoh cc-idx.XXXXXX)
cleanup() { git worktree remove --force "$W" >/dev/null 2>&1; rm -rf "$W" "$idx"; }
trap cleanup EXIT
cleanup; idx=$(mktemp -p /dev/shm/hoh cc-idx.XXXXXX)
log() { echo "$(date +%H:%M) $*" | tee -a "$H/logs/commit-check.done"; }

if [[ ${1:-} == --head-test ]]; then
  export CARGO_TARGET_DIR=$H/headtest-target
  rev=$(git rev-parse --short HEAD); git worktree add -q --detach "$W" "$rev" || exit 1
  ( cd "$W" && cargo update --workspace --offline -q && timeout 3600 cargo check --workspace --all-targets --offline --locked -q \
      && timeout 5400 cargo test --workspace --offline --locked --no-fail-fast ) >"$H/logs/head-test.txt" 2>&1; rc=$?
  pass=$(grep -oE '^test result: [a-zA-Z]+\. [0-9]+ passed; [0-9]+ failed' "$H/logs/head-test.txt" | awk '{p+=$4; f+=$6} END{print p"/"f}')
  log "head-test $rev rc=$rc тесты $pass (logs/head-test.txt)"; exit $rc
fi

pkg=; [[ ${1:-} == --pkg ]] && { pkg=$2; shift 2; }
msg=$1; shift
old=$(git rev-parse HEAD)
export GIT_INDEX_FILE=$idx
git read-tree HEAD || exit 1
for p in "$@"; do
  if [[ $p == docs/porting/MAP.tsv || -z $pkg ]]; then git --work-tree="$G" add -A -- "$p" 2>/dev/null || [[ ! -e $G/$p ]] || exit 1
  else
    git rm -r -q --cached --ignore-unmatch -- "$p" >/dev/null
    git ls-tree -r --full-tree "$pkg" -- "$p" | git update-index --index-info || exit 1
  fi
done
extra=$(git diff --cached --name-only HEAD | while read -r f; do
  ok=0; for p in "$@" Cargo.lock; do [[ $f == "$p" || $f == "$p"/* ]] && ok=1; done; (( ok )) || echo "$f"; done)
[[ -z $extra ]] || { log "ОТКАЗ пути: вне списка пакета $(tr '\n' ' ' <<<"$extra")— $*"; exit 1; }
cand=$(git commit-tree "$(git write-tree)" -p "$old" -m tmp) || exit 1
unset GIT_INDEX_FILE
git worktree add -q --detach "$W" "$cand" || exit 1
( cd "$W" && cargo update --workspace --offline -q ) >"$H/logs/commit-check.last.txt" 2>&1 || { log "ОТКАЗ lock: $*"; exit 1; }
blob=$(git hash-object -w "$W/Cargo.lock"); GIT_INDEX_FILE=$idx git update-index --cacheinfo "100644,$blob,Cargo.lock"
if ( cd "$W" && timeout 3600 cargo check --workspace --all-targets --offline --locked ) >>"$H/logs/commit-check.last.txt" 2>&1; then
  new=$(git commit-tree "$(GIT_INDEX_FILE=$idx git write-tree)" -p "$old" -F - <<<"$msg") || exit 1
  if git update-ref -m "commit-check: $*" HEAD "$new" "$old"; then
    git reset -q   # индекс общего дерева = новый HEAD (рабочие файлы общего дерева не трогаются)
    log "КОММИТ $(git rev-parse --short HEAD): $*"
  else log "ОТКАЗ: HEAD сдвинулся во время проверки — $*"; exit 1; fi
else
  log "ОТКАЗ: чистая выгрузка не собирается — $* (logs/commit-check.last.txt)"; exit 1
fi
