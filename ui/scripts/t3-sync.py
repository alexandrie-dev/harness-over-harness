#!/usr/bin/env python3
"""Переносит файлы T3 Code (MIT, только чтение) в ui/src/t3/ без изменений и пишет таблицу псевдонимов.

Запуск: python3 -I ui/scripts/t3-sync.py [корень T3]. Ничего не ставит и не ходит в сеть.
Дерево повторяет T3: apps/web/src -> src/t3/web, packages/shared/src -> src/t3/shared,
packages/client-runtime/src -> src/t3/client-runtime. Список файлов — ui/src/t3/manifest.json.
Результат: сами файлы, src/t3/tsconfig.paths.json (paths и exclude для tsc), src/t3/aliases.json (для vite).
"""
import json, os, shutil, sys
HERE = os.path.dirname(os.path.abspath(__file__))
UI = os.path.dirname(HERE)
T3 = sys.argv[1] if len(sys.argv) > 1 else '/mnt/toshiba/alexandrie/third-party/t3code'
OUT = os.path.join(UI, 'src', 't3')
ROOTS = {'web': 'apps/web/src', 'shared': 'packages/shared/src', 'client-runtime': 'packages/client-runtime/src'}
PKG = {'shared': ('@t3tools/shared/', 'packages/shared'), 'client-runtime': ('@t3tools/client-runtime/', 'packages/client-runtime')}
manifest = json.load(open(os.path.join(OUT, 'manifest.json')))
paths, aliases, exclude, copied = {}, {}, [], []
for root, entries in manifest['files'].items():
    for entry in entries:
        name, deferred = (entry, False) if isinstance(entry, str) else (entry['file'], entry.get('deferred', False))
        src = os.path.join(T3, ROOTS[root], name); dst = os.path.join(OUT, root, name)
        os.makedirs(os.path.dirname(dst), exist_ok=True); shutil.copyfile(src, dst); copied.append((root, name, deferred))
        if deferred and not manifest.get('deferredEnabled', False): exclude.append(f'{root}/{name}')
for root, (prefix, pdir) in PKG.items():
    exports = json.load(open(os.path.join(T3, pdir, 'package.json')))['exports']
    for key, value in exports.items():
        target = value if isinstance(value, str) else (value.get('types') or value.get('import') or value.get('default'))
        rel = os.path.normpath(os.path.join(pdir, target)); base = os.path.join(T3, ROOTS[root].rsplit('/', 1)[0])
        name = os.path.relpath(os.path.join(T3, rel), os.path.join(T3, ROOTS[root]))
        if any(r == root and n == name for r, n, _ in copied):
            paths[prefix + key[2:]] = [f'./{root}/{name}']; aliases[prefix + key[2:]] = f'src/t3/{root}/{name}'
for stub, target in manifest['aliases'].items():
    paths[stub] = ['./' + target[len('src/t3/'):]]; aliases[stub] = target
json.dump({'compilerOptions': {'paths': {**paths, '~/*': ['./web/*']}}, 'exclude': [f'{e}' for e in exclude]}, open(os.path.join(OUT, 'tsconfig.paths.json'), 'w'), indent=2, ensure_ascii=False)
json.dump({'~': 'src/t3/web', **aliases}, open(os.path.join(OUT, 'aliases.json'), 'w'), indent=2, ensure_ascii=False)
print(f'copied {len(copied)} files, {len(exclude)} skipped by tsc (deferred), {len(aliases)} aliases')
