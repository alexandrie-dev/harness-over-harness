#!/usr/bin/env bash
# Снимок рабочего дерева для QA без изменения индекса, веток и файлов: временный индекс → дерево → объект коммита.
#   bash scripts/qa-snapshot.sh   → печатает SHA коммита-снимка (родитель HEAD), его можно отдать `git worktree add`.
# Включает и неотслеживаемые файлы (кроме .gitignore). Ссылки не создаются; объект живёт до git gc.
set -euo pipefail
cd "$(dirname "$0")/.."
idx=$(mktemp -p /dev/shm/hoh qa-index.XXXXXX)
trap 'rm -f "$idx"' EXIT
cp .git/index "$idx"
GIT_INDEX_FILE=$idx git add -A .
tree=$(GIT_INDEX_FILE=$idx git write-tree)
git commit-tree "$tree" -p HEAD -m "QA snapshot $(date '+%F %H:%M')"
