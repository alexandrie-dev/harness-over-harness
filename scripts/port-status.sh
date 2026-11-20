#!/usr/bin/env bash
# Prints the two progress numbers of the port: crates done and scenarios passing.
# `port-status.sh ready [STAGE]` instead lists the packages of a stage (default 1) that can be assigned now:
# status `todo` and every ported package their source imports is `done`; then the packages that block the most.
# HOH_ASSUME_DONE="crate crate" counts those crates as done, to see what the next wave would be.
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "${1:-}" == ready ]]; then
  exec python3 -I - "${2:-1}" "${HOH_DSH:-/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness}" <<'PY'
import collections, csv, json, os, re, sys
stage, dsh = sys.argv[1], sys.argv[2]
rows = [r for r in csv.DictReader(open('docs/porting/MAP.tsv', encoding='utf-8'), delimiter='\t', quoting=csv.QUOTE_NONE)
        if r['stage'] != '9']
status = collections.defaultdict(set)          # crate -> statuses of its rows
for r in rows:
    status[r['hoh_crate']].add(r['status'])
crate_of = {r['dsh_name']: r['hoh_crate'] for r in rows}
assumed = set(os.environ.get('HOH_ASSUME_DONE', '').split())   # crates to count as done, to plan the next wave
finished = lambda crate: crate in assumed or status[crate] == {'done'}
ready, blockers = [], collections.Counter()
for r in rows:
    if r['stage'] != stage or r['status'] != 'todo':
        continue
    # What the code imports, not what package.json lists: a peer dependency the code never touches is not
    # needed by the crate, and a package imported only for its types is.
    names, sources = set(), 0
    for folder, _, files in os.walk(os.path.join(dsh, r['dsh_path'], 'src')):
        for name in files:
            if name.endswith(('.ts', '.tsx')):
                sources += 1
                text = open(os.path.join(folder, name), encoding='utf-8').read()
                names.update(re.findall(r"""(?:from|import)\s*\(?\s*['"](@[\w.-]+/[\w.-]+)""", text))
    if not sources:
        manifest = json.load(open(os.path.join(dsh, r['dsh_path'], 'package.json'), encoding='utf-8'))
        names = {**manifest.get('dependencies', {}), **manifest.get('peerDependencies', {})}
    deps = sorted({crate_of[n] for n in names if n in crate_of} - {r['hoh_crate']})
    waiting = [d for d in deps if not finished(d)]
    blockers.update(waiting)
    if not waiting:
        ready.append((int(r['src_lines']), int(r['test_lines']), r['hoh_crate'], deps))
for src, tests, crate, deps in sorted(ready):
    print(f"{crate}\t{src}\t{tests}\t{' '.join(deps) or '-'}")
print(f"готовы к выдаче: {len(ready)}; ждут зависимостей: {sum(1 for r in rows if r['stage'] == stage and r['status'] == 'todo') - len(ready)}")
print('больше всего ждут: ' + ', '.join(f'{c} ({n})' for c, n in blockers.most_common(8)))
PY
fi

awk -F'\t' '
  NR == 1 { next }
  $1 != 9 { total[$1]++; if ($8 == "done") done[$1]++; all++; if ($8 == "done") alldone++ }
  END {
    for (s = 0; s <= 5; s++) printf "этап %d: %d из %d пакетов\n", s, done[s] + 0, total[s] + 0
    printf "пакеты: %d из %d\n", alldone + 0, all + 0
  }' docs/porting/MAP.tsv

# Сценарии — из последнего прогона scripts/conformance-v1.sh (CEO 20.11: это главная мера ядра).
# Точечные прогоны писателей (--scenario) не в счёт: берём последний полный (119 сценариев + шапка).
last=; for f in $(ls -d /mnt/toshiba/alexandrie/hoh/conformance-runs/2*/summary.tsv 2>/dev/null | sort -r); do
  [[ $(wc -l <"$f") -eq 120 ]] && { last=$f; break; }; done
if [[ -n $last ]]; then
  awk -F'\t' -v run="$(basename "$(dirname "$last")")" '
    NR == 1 { next }
    { n[$3]++; if ($4 == "прошло") { ok[$3]++; if ($6 != "-") unchecked[$3]++ } else if ($4 == "расхождение") ph[$5]++; else other[$4]++ }
    END {
      # «прошло» с непустым body_check = сравнения совпали, но проверка тела сценария ещё не перенесена.
      printf "сценарии (прогон %s): in прошло %d из %d (тело не проверено %d), extra %d из %d; расхождения по фазам:", run, ok["in"] + 0, n["in"] + 0, unchecked["in"] + 0, ok["extra"] + 0, n["extra"] + 0
      for (k in ph) printf " %s %d", k, ph[k]
      for (k in other) printf "; %s %d", k, other[k]
      printf "\n"
    }' "$last"
else
  echo "сценарии: прогона conformance-v1 ещё не было"
fi
