#!/usr/bin/env bash
# Пакет = явный список путей из дерева писателя поверх main (CEO 20.11 19:3x, указание основателя «чтобы писатели
# не мешали друг другу»). Прогон, ревью, приёмка и коммит работают с одним и тем же коммитом пакета C.
#   scripts/pkg.sh paths CRATE...                    → пути пакета (по строке)
#   scripts/pkg.sh commit ID WRITER CRATE...         → C = HEAD + пути пакета из дерева WRITER (+ зависимости), stdout: C
#   scripts/pkg.sh checkout DIR C                    → DIR — отдельное git-дерево ровно на C (чистое)
# Дерево писателя: /mnt/toshiba/alexandrie/hoh/wt/<WRITER>; нет такого (ещё не перешёл) — общее дерево.
# Общие файлы (SHARED) в пакет не входят никогда: их вносит интеграция.
# Зависимости: pipeline/deps.tsv «крейт<TAB>крейт-сосед»; владелец соседа — pipeline/owners.tsv «крейт<TAB>писатель».
# Сосед подкладывается в C для прогона, но коммитится только своим пакетом (commit-check берёт пути пакета).
set -uo pipefail
G=$(cd "$(dirname "$0")/.." && pwd); cd "$G"
H=/mnt/toshiba/alexandrie/hoh; P=$H/pipeline
SHARED=(Cargo.toml Cargo.lock docs/porting/MAP.tsv apps/hoh/Cargo.toml apps/hoh/src/lib.rs apps/hoh/src/v1_registry.rs apps/hoh/src/registry.rs apps/hoh/src/profile_boot.rs apps/hoh/tests/built_bin.rs)   # подключение плагинов v1 — интеграция

tree_of() { [[ -n ${1:-} && -e $H/wt/$1/.git ]] && echo "$H/wt/$1" || echo "$G"; }
path_of() { case $1 in hoh) echo apps/hoh;; hoh-desktop) echo apps/hoh-desktop;; *) echo "crates/$1";; esac; }
paths() { local c; for c in "$@"; do path_of "$c"; done; }

overlay() {   # overlay IDX SRC PATH — путь в индексе IDX заменяется содержимым дерева SRC
  local idx=$1 src=$2 p=$3
  GIT_INDEX_FILE=$idx git rm -r -q --cached --ignore-unmatch -- "$p" >/dev/null
  [[ -e $src/$p ]] && GIT_INDEX_FILE=$idx git --work-tree="$src" add -A -- "$p" 2>/dev/null \
    || [[ ! -e $src/$p ]]   # удалённый в дереве писателя путь — удалён и в пакете
}

case ${1:-} in
  paths) shift; paths "$@" ;;
  commit)
    id=$2 writer=$3; shift 3
    src=$(tree_of "$writer"); idx=$(mktemp -p /dev/shm/hoh pkg-idx.XXXXXX); trap 'rm -f "$idx"' EXIT
    M=   # дерево писателя: трёхстороннее слияние своих путей (база ветки w/<писатель> → правки писателя) с HEAD
    if [[ $src != "$G" ]]; then
      B=$(git rev-parse "w/$writer") || exit 1
      GIT_INDEX_FILE=$idx git read-tree "$B" || exit 1
      for c in "$@"; do overlay "$idx" "$src" "$(path_of "$c")" || exit 1; done
      T=$(git commit-tree "$(GIT_INDEX_FILE=$idx git write-tree)" -p "$B" -m "правки $writer") || exit 1
      out=$(git merge-tree --write-tree --name-only --merge-base "$B" HEAD "$T" 2>&1); M=$(head -1 <<<"$out")
      # путь пакета, которого ещё нет в main, — новый пакет: берётся версия писателя без слияния
      # (иначе «нет в main» читается как удаление и даёт ложный конфликт modify/delete)
      newp=(); for c in "$@"; do p=$(path_of "$c"); git cat-file -e "HEAD:$p" 2>/dev/null || newp+=("$p"); done
      conf=$(sed -n '2,/^$/p' <<<"$out" | grep -v '^$' | while read -r f; do
        for c in "$@"; do p=$(path_of "$c"); [[ " ${newp[*]} " == *" $p "* ]] && continue; [[ $f == "$p"/* ]] && echo "$f"; done; done | sort -u)
      if [[ -n $conf ]]; then
        echo "КОНФЛИКТ слияния по коду пакета ($writer, база $B, main $(git rev-parse --short HEAD)): $(tr '\n' ' ' <<<"$conf")" >&2
        git update-ref "refs/hoh/conflict/$id/writer" "$T"; exit 4
      fi
    fi
    GIT_INDEX_FILE=$idx git read-tree HEAD || exit 1
    for c in "$@"; do
      p=$(path_of "$c")
      if [[ -n $M ]]; then GIT_INDEX_FILE=$idx git rm -r -q --cached --ignore-unmatch -- "$p" >/dev/null
        from=$M; [[ " ${newp[*]} " == *" $p "* ]] && from=$T
        git ls-tree -r --full-tree "$from" -- "$p" | GIT_INDEX_FILE=$idx git update-index --index-info || exit 1
      else overlay "$idx" "$src" "$p" || exit 1; fi
      while IFS=$'\t' read -r k dep; do
        [[ $k == "$c" && -n $dep ]] || continue
        owner=$(awk -F'\t' -v c="$dep" '$1==c{print $2}' "$P/owners.tsv" 2>/dev/null)
        overlay "$idx" "$(tree_of "$owner")" "$(path_of "$dep")" || exit 1
        echo "зависимость: $dep из $(tree_of "$owner")" >&2
      done < <(cat "$P/deps.tsv" 2>/dev/null)
    done
    for f in "${SHARED[@]}"; do GIT_INDEX_FILE=$idx git reset -q HEAD -- "$f" 2>/dev/null; done
    c=$(git commit-tree "$(GIT_INDEX_FILE=$idx git write-tree)" -p HEAD -m "пакет $id ($writer): $*") || exit 1
    git update-ref "refs/hoh/pkg/$id" "$c"
    { echo "# пакет $id, писатель $writer, дерево $src, коммит $c (main $(git rev-parse --short HEAD) + пути пакета)"
      for f in "${SHARED[@]}"; do [[ -e $src/$f ]] && ! cmp -s "$src/$f" <(git show "HEAD:$f" 2>/dev/null) \
        && echo "# общий файл отличается в дереве писателя (в пакет не вошёл, решает интеграция): $f"; done
      git -c core.quotepath=off diff --stat HEAD "$c"; echo; git diff HEAD "$c"; } >"$P/reviews/$id.diff"
    echo "$c" ;;
  checkout)
    dir=$2 c=$3
    if [[ -e $dir/.git ]]; then git -C "$dir" checkout -q -f --detach "$c" && git -C "$dir" clean -fdxq
    else rm -rf "$dir"   # CEO 20.11 20:20: деревья it/* сразу на NVMe, на прежнем пути — ссылка
      if [[ $dir == "$H"/it/* ]]; then n=${dir#"$H"/}; rm -rf "/home/save/.cache/hoh-build/$n"; mkdir -p "/home/save/.cache/hoh-build/$n" && ln -s "/home/save/.cache/hoh-build/$n" "$dir"; fi
      git worktree add -q --detach "$dir" "$c"; fi ;;
  *) echo "usage: $0 paths CRATE... | commit ID WRITER CRATE... | checkout DIR C" >&2; exit 2 ;;
esac
