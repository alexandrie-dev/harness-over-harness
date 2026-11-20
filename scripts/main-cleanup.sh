#!/usr/bin/env bash
# Уборка общего дерева после перехода писателей в свои деревья (CEO 20.11 19:3x, указание основателя).
#   systemd-run --user --collect --unit hoh-main-cleanup ... bash scripts/main-cleanup.sh
# 1) ждёт, пока ни один ход claude -p не работает в общем дереве (cwd процесса);
# 2) снимок общего дерева (ref refs/hoh/snapshot-<время> + архив на toshiba);
# 3) недостающие деревья писателей и дерево parked (недописанные пакеты прежних писателей) — из этого снимка;
# 4) общее дерево → HEAD: изменённые отслеживаемые файлы возвращаются, неотслеживаемые удаляются.
#    Не трогаются: design/ (не наш каталог), игнорируемые файлы (target и т. п.).
set -uo pipefail
G=$(cd "$(dirname "$0")/.." && pwd); cd "$G"
H=/mnt/toshiba/alexandrie/hoh; L=$H/logs; S=$H/snapshots
WRITERS=(s1 s2 s3 s4 s5 u1 o1 f1 parked)
in_main() { local p; for p in /proc/[0-9]*; do [[ $(cat "$p/comm" 2>/dev/null) == claude ]] || continue
  [[ $(tr '\0' '\n' <"$p/cmdline" 2>/dev/null | sed -n 2p) == -p && $(readlink "$p/cwd") == "$G" ]] && return 0; done; return 1; }
until ! in_main; do sleep 60; done
exec 7>"$H/wt/.git.lock"; flock 7
exec 9>"$H/pipeline/commit-queue.lock"; flock 9
# файлы интеграции (скрипты, планы, документы) — коммитом в main, чтобы уборка их не удалила
INTEG=(scripts plans docs conformance research PLAN.md)
idx=$(mktemp -p /dev/shm/hoh mc-idx.XXXXXX); old=$(git rev-parse HEAD)
GIT_INDEX_FILE=$idx git read-tree HEAD && GIT_INDEX_FILE=$idx git add -A -- "${INTEG[@]}" || exit 1
if ! GIT_INDEX_FILE=$idx git diff --cached --quiet HEAD; then
  extra=$(GIT_INDEX_FILE=$idx git diff --cached --name-only HEAD | grep -vE '^(scripts|plans|docs|conformance|research)/|^PLAN\.md$')
  [[ -z $extra ]] || { echo "$(date +%H:%M) NEEDS-LEAD уборка: вне списка интеграции $extra" >>"$H/pipeline/attention.log"; exit 1; }
  new=$(git commit-tree "$(GIT_INDEX_FILE=$idx git write-tree)" -p "$old" -m "интеграция: скрипты и планы перед уборкой общего дерева

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>") && git update-ref HEAD "$new" "$old" && git reset -q || exit 1
fi
rm -f "$idx"; flock -u 9
T=$(date +%Y%m%d-%H%M); idx=$(mktemp -p /dev/shm/hoh mc-idx.XXXXXX)
GIT_INDEX_FILE=$idx git read-tree HEAD && GIT_INDEX_FILE=$idx git add -A -- . || exit 1
snap=$(git commit-tree "$(GIT_INDEX_FILE=$idx git write-tree)" -p HEAD -m "снимок общего дерева $T перед уборкой") || exit 1
rm -f "$idx"; git update-ref "refs/hoh/snapshot-$T" "$snap"; echo "$snap" >"$S/main-cleanup-$T.sha"
git archive --format=tar "$snap" | gzip -1 >"$S/main-cleanup-$T.tar.gz" || exit 1
made=()
for w in "${WRITERS[@]}"; do
  [[ -e $H/wt/$w/.git ]] && continue
  git worktree add -q -B "w/$w" "$H/wt/$w" "$snap" && made+=("$w") \
    && echo "$(date +%H:%M) $w: дерево $H/wt/$w, ветка w/$w от $snap (уборка общего дерева)" >>"$L/worktrees.log"
done
n=$(git status --porcelain -uall -- . ':!design' | wc -l)
git checkout -q -f HEAD -- . ':!design' && git clean -fdq -- . ':!design' || exit 1
left=$(git status --porcelain -uall -- . ':!design' | wc -l)
echo "$(date +%H:%M) уборка общего дерева: снимок $snap, файлов возвращено/удалено $n, осталось $left, новые деревья: ${made[*]:-нет}" | tee -a "$L/worktrees.log" >>"$H/pipeline/attention.log"
