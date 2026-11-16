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

if [[ -f conformance/EXPECTED.tsv ]]; then
  awk -F'\t' '
    NR == 1 { for (i = 1; i <= NF; i++) if ($i == "status") col = i; next }
    { n++; by[$col]++ }
    END {
      printf "сценарии: %d из %d проходят", by["pass"] + 0, n
      for (k in by) if (k != "pass") printf ", %s %d", k, by[k]
      printf "\n"
    }' conformance/EXPECTED.tsv
else
  echo "сценарии: conformance/EXPECTED.tsv ещё нет"
fi
