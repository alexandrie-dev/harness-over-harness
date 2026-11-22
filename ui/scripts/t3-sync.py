#!/usr/bin/env python3
"""Переносит файлы T3 Code (MIT, только чтение) в ui/src/t3/ без изменений и пишет таблицу псевдонимов.

Запуск: python3 -I ui/scripts/t3-sync.py [корень T3]. Ничего не ставит и не ходит в сеть.
Прежний перенос (рендер ответов старой оболочки): apps/web/src -> src/t3/web, packages/shared/src -> src/t3/shared,
packages/client-runtime/src -> src/t3/client-runtime; список файлов — "files" в ui/src/t3/manifest.json;
результат — сами файлы, src/t3/tsconfig.paths.json (paths и exclude для tsc), src/t3/aliases.json (для vite).
HOH-40 (раздел "base" манифеста): весь веб-клиент T3 в src/t3/base — см. sync_base() ниже.
"""
import json, os, re, shutil, subprocess, sys
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
# Основа HOH-40 (src/t3/base) — отдельный проект tsc со своими псевдонимами: прежний tsconfig её не видит.
json.dump({'compilerOptions': {'paths': {**paths, '~/*': ['./web/*']}}, 'exclude': [f'{e}' for e in exclude] + (['base'] if 'base' in manifest else [])}, open(os.path.join(OUT, 'tsconfig.paths.json'), 'w'), indent=2, ensure_ascii=False)
json.dump({'~': 'src/t3/web', **aliases}, open(os.path.join(OUT, 'aliases.json'), 'w'), indent=2, ensure_ascii=False)
print(f'copied {len(copied)} files, {len(exclude)} skipped by tsc (deferred), {len(aliases)} aliases')


# ───── HOH-40: весь веб-клиент T3 как основа окна (plans/hoh40-t3-base.md, этап 2) ─────
# Переносится apps/web/src целиком (без тестов) и файлы packages/{contracts,shared,client-runtime}/src, до которых
# дотягиваются его импорты (scripts/t3graph.py). Файлы копируются без правок; исключения перечислены в манифесте:
#   replaced — наш файл вместо файла T3 (знак T3 не берём), patched — файл T3 + наша правка из src/t3/base-patches/.
# Пакеты скрытых частей и пакеты T3, которые не берутся, заменяются заглушками (src/t3/base/stubs, пишутся здесь).
# Отложенные — файлы, в замыкании импортов которых есть пакет, которого нет в ui/package.json и ui/node_modules в версии, совместимой с lock T3:
# их не проверяет tsc (src/t3/base/tsconfig.json перечисляет только остальные). После установки пакетов список пуст.
STUB_HEAD = '// Заглушка окна HoH вместо `{spec}` (T3 Code, MIT): {why}. Пакет не ставится; файл пишет scripts/t3-sync.py.\n'
ABSENT = '''// Заглушки окна HoH (src/t3/base/stubs) — значения скрытых частей T3. Значение заглушки — функция; её вызов (и new)
// даёт объект, любое свойство которого — функция, возвращающая undefined (then — undefined: это не обещание; перебор —
// пустой). Так код скрытой части загружается и на уровне модуля (`export const { a, b } = make(...)`) не падает,
// но ничего не делает. Образец — пробная сборка u4 (HOH-40 веха 1). Файл пишет scripts/t3-sync.py.
/** Тип значения заглушки: его можно вызвать, создать через new, унаследовать и читать любые свойства. */
export type Anything = { (...args: any[]): any; new (...args: any[]): any; [key: string]: any };

const leaf = () => undefined;
const made: any = new Proxy({}, { get: (_target, key) => (key === 'then' ? undefined : key === Symbol.iterator ? function* () {} : leaf) });

export function absent(spec: string, name: string): Anything {
  const value = function () { return made; };
  Object.defineProperty(value, 'name', { value: `${spec}:${name}` });
  return value as unknown as Anything;
}
'''


def stub_names(graph, files, spec):
    """{имя: члены} — что перенесённые файлы берут из spec (default, именованные, через import() и export … from)
    и к каким членам обращаются через точку (JSZip.JSZipObject): они нужны как типы в пространстве имён заглушки."""
    names, q = {}, re.escape(spec)
    for f in files:
        text = open(os.path.join(graph.t3, f), encoding='utf-8').read()
        local = {}
        for m in re.finditer(r'(?:^|[\s;}])(import|export)\s+(?:type\s+)?([^;]*?)\s+from\s+[\'"]' + q + r'[\'"]', text, re.S):
            clause = m.group(2).strip()
            head = re.match(r'^([\w$]+)', clause)
            if m.group(1) == 'import' and head and head.group(1) != 'type': local[head.group(1)] = 'default'
            for braces in re.findall(r'\{([^}]*)\}', clause):
                for item in braces.split(','):
                    item = re.sub(r'^\s*type\s+', '', item).strip()
                    if not item: continue
                    name, _, alias = item.partition(' as ')
                    names.setdefault(name.strip(), set()); local[(alias or name).strip()] = name.strip()
            ns = re.match(r'^\*\s+as\s+([\w$]+)', clause)
            if ns:
                for member in re.findall(r'\b' + re.escape(ns.group(1)) + r'\.([A-Za-z_$][\w$]*)', text): names.setdefault(member, set())
        for m in re.finditer(r'\{([^}]*)\}\s*\]?\s*=\s*await\s+(?:Promise\.all\(\[\s*)?import\(\s*[\'"]' + q + r'[\'"]', text):
            for i in m.group(1).split(','):
                if i.strip(): names.setdefault(i.split(':')[0].strip(), set())
        for m in re.finditer(r'import\(\s*[\'"]' + q + r'[\'"]\s*\)\s*\.then\(\s*\(\s*\{([^}]*)\}', text):
            for i in m.group(1).split(','):
                if i.strip(): names.setdefault(i.split(':')[0].strip(), set())
        if re.search(r'import\(\s*[\'"]' + q + r'[\'"]', text): names.setdefault('default', set())
        for alias, name in local.items():
            names.setdefault(name, set()).update(re.findall(r'(?<![\w$.])' + re.escape(alias) + r'\.([A-Za-z_$][\w$]*)', text))
    ident = re.compile(r'^[A-Za-z_$][\w$]*$')
    return {n: sorted(m for m in ms if ident.match(m)) for n, ms in sorted(names.items()) if ident.match(n)}


def stub_text(spec, why, names, absent_rel):
    generic = '<A = any, B = any, C = any, D = any>'
    out = [STUB_HEAD.format(spec=spec, why=why), f"import {{ absent }} from '{absent_rel}';\n"]
    for n, members in names.items():
        local, export = ('value', '') if n == 'default' else (n, 'export ')
        out.append(f"{export}const {local}: any = absent('{spec}', '{n}');\n")
        if members: out.append(f"{export}declare namespace {local} {{ " + ' '.join(f'type {m}{generic} = any;' for m in members) + ' }\n')
        out.append(f"{export}type {local}{generic} = any;\n")
        if n == 'default': out.append('export default value;\n')
    return ''.join(out)


def write_declarations(out, tsconfig, on_stubs):
    """Объявления файлов на заглушках: tsc --noCheck во временную папку /dev/shm/hoh, затем рядом с файлом X.hoh.d.ts и
    X.hoh.ts (реэкспорт); tsconfig получает moduleSuffixes ['.hoh', ''], а сами файлы на заглушках в files не входят."""
    import tempfile
    os.makedirs('/dev/shm/hoh', exist_ok=True)
    tmp = tempfile.mkdtemp(prefix='t3-sync-decl-', dir='/dev/shm/hoh')
    try:
        config = os.path.join(tmp, 'tsconfig.json')
        options = {**tsconfig['compilerOptions'], 'noEmit': False, 'declaration': True, 'emitDeclarationOnly': True, 'noCheck': True,
                   'outDir': os.path.join(tmp, 'out'), 'rootDir': out, 'baseUrl': out,
                   'typeRoots': [os.path.normpath(os.path.join(out, t)) for t in tsconfig['compilerOptions']['typeRoots']]}
        options['paths'] = {k: [os.path.join(out, v) for v in vs] for k, vs in options['paths'].items()}
        json.dump({'compilerOptions': options, 'files': [os.path.join(out, f) for f in on_stubs]}, open(config, 'w'))
        subprocess.run([os.path.join(UI, 'node_modules/.bin/tsc'), '-p', config], cwd=UI, capture_output=True)  # TS7056 у чужих файлов — не наши
        for f in on_stubs:
            stem = re.sub(r'\.tsx?$', '', f)
            decl = open(os.path.join(tmp, 'out', stem + '.d.ts')).read()
            head = f'// Объявления {f} (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.\n'
            open(os.path.join(out, stem + '.hoh.d.ts'), 'w').write(head + decl)
            name = os.path.basename(stem)
            default = f"export {{ default }} from './{name}.hoh.d';\n" if re.search(r'^export default |^export \{[^}]*\bdefault\b', decl, re.M) else ''
            open(os.path.join(out, stem + '.hoh.ts'), 'w').write(head + f"export * from './{name}.hoh.d';\n" + default)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    tsconfig['compilerOptions']['moduleSuffixes'] = ['.hoh', '']


def sync_base(base):
    sys.dont_write_bytecode = True
    sys.path.insert(0, HERE)
    import t3graph
    out = os.path.join(UI, base['out'])
    r = t3graph.analyse(T3); g = r['graph']
    rel = lambda f: next(f'{k}/{f[len(v) + 1:]}' for k, v in t3graph.ROOTS.items() if f.startswith(v + '/'))
    replaced, patched = base.get('replaced', {}), base.get('patched', {})
    # Не переносятся (манифест, exclude: шаблон пути T3 -> причина): файлы данных скрытых частей и лишние переводы.
    excluded = [f for f in r['transfer'] if any(re.search(p, f) for p in base.get('exclude', {}))]
    r['transfer'] = [f for f in r['transfer'] if f not in excluded]
    keep = {rel(f) for f in r['transfer']} | set(replaced)
    # Перенос: всё из списка, кроме наших замен; лишнее из прежних переносов удаляется (кроме наших файлов).
    for root in t3graph.ROOTS:
        for dp, _, names in os.walk(os.path.join(out, root)):
            for n in names:
                x = os.path.relpath(os.path.join(dp, n), out)
                # web/hoh-design — наш код раунда дизайна главной (HOH-41): не из T3, не удаляется.
                if x not in keep and not x.startswith('web/hoh-design/'): os.remove(os.path.join(dp, n))
    for f in r['transfer']:
        x = rel(f)
        if x in replaced: continue
        dst = os.path.join(out, x); os.makedirs(os.path.dirname(dst), exist_ok=True); shutil.copyfile(os.path.join(T3, f), dst)
    for dst, src in base.get('extra', {}).items():
        shutil.copyfile(os.path.join(T3, src), os.path.join(out, dst))
    patches = os.path.join(UI, base['patches'])
    for x in patched:
        subprocess.run(['patch', '-s', '-p1', '--forward', '--no-backup-if-mismatch', '-d', out, '-i', os.path.join(patches, x + '.patch')], check=True)
    # Заглушки: по одному файлу на каждый импортируемый путь пакета (без ?url и т.п.: их типы даёт vite/client).
    stubs_dir = os.path.join(out, 'stubs'); shutil.rmtree(stubs_dir, ignore_errors=True); os.makedirs(stubs_dir)
    open(os.path.join(stubs_dir, 'absent.ts'), 'w').write(ABSENT)
    open(os.path.join(stubs_dir, 'absent-url.ts'), 'w').write("// Заглушка окна HoH для адресов файлов (`?url`) пакетов скрытых частей. Файл пишет scripts/t3-sync.py.\nexport default '';\n")
    stubbed = set(r['stubbed']) | set(r['ws'])
    specs = {}
    for f in r['transfer']:
        for spec, _, target in g.edges.get(f, ()):
            if target[0] in ('npm', 'ws') and target[1] in stubbed: specs.setdefault(spec, set()).add(f)
    paths, aliases, stub_pkgs = {}, {}, set()
    for spec, files in sorted(specs.items()):
        pkg = t3graph.root_pkg(spec.split('?')[0])
        if '?' in spec:
            aliases[spec] = f"{base['out']}/stubs/absent-url.ts"; continue
        why = t3graph.ws_reason(pkg) if pkg.startswith('@t3tools/') else (t3graph.FORCE_STUB.get(pkg) or '; '.join(sorted({t3graph.hidden_part(f) or 'скрытые части' for f in files})))
        if os.path.isfile(os.path.join(out, 'types', spec, 'index.d.ts')):
            continue  # /// <reference types>: paths на них не действуют, прослойка лежит в types/ и находится через typeRoots
        shim = next((f'shims/{spec}{e}' for e in ('.ts', '.tsx') if os.path.isfile(os.path.join(out, 'shims', spec + e))), None)
        if shim:
            # Рукописная прослойка (наш файл с типами или поведением) вместо созданной заглушки.
            paths[spec] = ['./' + shim]; aliases[spec] = f"{base['out']}/{shim}"; continue
        target = f'stubs/{spec}.ts'
        os.makedirs(os.path.dirname(os.path.join(out, target)), exist_ok=True)
        absent_rel = os.path.relpath(os.path.join(stubs_dir, 'absent'), os.path.dirname(os.path.join(out, target)))
        open(os.path.join(out, target), 'w').write(stub_text(spec, why, stub_names(g, files, spec), absent_rel if absent_rel.startswith('.') else './' + absent_rel))
        paths[spec] = ['./' + target]; aliases[spec] = f"{base['out']}/{target}"; stub_pkgs.add(pkg)
    # Импорты непереносимых файлов (`./models/x.glb?url`) — адрес-заглушка; типы `?url` даёт vite/client.
    for f in r['transfer']:
        for spec, _, target in g.edges.get(f, ()):
            if target[0] == 'file' and target[1] in excluded: aliases[spec] = f"{base['out']}/stubs/absent-url.ts"
    # Пакеты T3, которые переносятся: псевдонимы по exports их package.json — только на перенесённые файлы.
    for spec, target in sorted(g.exports.items()):
        if target in g.set and rel(target) in keep:
            paths[spec] = ['./' + rel(target)]; aliases[spec] = f"{base['out']}/{rel(target)}"
    # Отложенные: файл, из которого достижим файл с импортом пакета, которого нет в окне в версии, совместимой с lock T3.
    # Пакет есть, если он записан в ui/package.json (закоммиченное состояние окна) и стоит в ui/node_modules: так список
    # отложенных один и тот же у всех, кто собирает с этим package.json, даже если node_modules у кого-то шире.
    want = t3graph.t3_versions(T3)
    window = json.load(open(os.path.join(UI, 'package.json'))); listed = {**window.get('devDependencies', {}), **window.get('dependencies', {})}
    def have(pkg):
        if pkg in stubbed: return True
        if pkg.startswith('node:'): pkg = '@types/node'
        if pkg not in listed: return False
        try: v = json.load(open(os.path.join(UI, 'node_modules', pkg, 'package.json')))['version']
        except OSError: return False
        if pkg not in want or pkg == '@types/node' or pkg in t3graph.WINDOW_VERSION: return True
        # Совместимая версия (тот же major, для 0.x — тот же minor) годится для проверки типов; точную ставит ведущий по T3-DEPS.md.
        a, b = (re.findall(r'\d+', x)[:2] for x in (v, want[pkg]))
        return a[:1] == b[:1] and (a[0] != '0' or a[1:2] == b[1:2])
    missing, bad, moved = {}, set(), set(r['transfer'])
    for f, es in g.edges.items():
        if f not in moved: continue
        for _, _, target in es:
            if target[0] == 'npm' and not have(target[1]): missing.setdefault(target[1], set()).add(f); bad.add(f)
    back = {}
    for f, es in g.edges.items():
        for _, _, target in es:
            if target[0] == 'file': back.setdefault(target[1], set()).add(f)
    deferred, stack = set(), list(bad)
    while stack:
        f = stack.pop()
        if f in deferred: continue
        deferred.add(f); stack.extend(back.get(f, ()))
    if base.get('deferredEnabled'): deferred = set()
    code = [f for f in r['transfer'] if f.endswith(t3graph.CODE)]
    # На заглушках: файлы скрытых частей, из которых через файлы скрытых частей достижим пакет-заглушка (без прослойки).
    # Их тела без настоящих пакетов не проверить (значения заглушек — any, колбэки без контекста типов), как и у
    # отложенных. tsc берёт вместо них объявления (X.hoh.d.ts из tsc --noCheck, X.hoh.ts реэкспортирует их; moduleSuffixes),
    # так что все их импортёры, включая видимые, проверяются полностью. Видимые файлы на заглушках не бывают.
    hidden_code = [f for f in code if f not in r['visible'] and f not in deferred]
    on_stubs = {f for f in hidden_code if any(t[0] == 'npm' and t[1] in stub_pkgs for _, _, t in g.edges.get(f, ()))}
    grew = True
    while grew:
        grew = False
        for f in hidden_code:
            if f not in on_stubs and any(t[0] == 'file' and t[1] in on_stubs for _, _, t in g.edges.get(f, ())):
                on_stubs.add(f); grew = True
    on_stubs = {f for f in on_stubs if rel(f) not in replaced}
    checked = sorted(rel(f) for f in code if f not in deferred and f not in on_stubs)
    checked += sorted(os.path.relpath(os.path.join(dp, n), out) for d in ('stubs', 'shims') for dp, _, ns in os.walk(os.path.join(out, d)) for n in ns)
    checked += sorted(x for x in replaced if not any(rel(f) == x and f in deferred for f in code))
    tsconfig = {
        '//': 'Проект tsc основы HOH-40 (src/t3/base). Пишет scripts/t3-sync.py; настройки — как apps/web/tsconfig.json и tsconfig.base.json T3. files — неотложенные файлы.',
        'compilerOptions': {
            'target': 'ES2023', 'module': 'Preserve', 'moduleResolution': 'Bundler', 'jsx': 'react-jsx', 'noEmit': True,
            'lib': ['ES2023', 'ES2024.Promise', 'ESNext.Disposable', 'DOM', 'DOM.Iterable'], 'types': ['vite-plus/client', 'node'],
            'typeRoots': ['./types', '../../../node_modules/@types'],
            'allowImportingTsExtensions': True, 'strict': True, 'noUncheckedIndexedAccess': True, 'exactOptionalPropertyTypes': True,
            'noImplicitOverride': True, 'useDefineForClassFields': True, 'forceConsistentCasingInFileNames': True, 'skipLibCheck': True,
            'allowSyntheticDefaultImports': True, 'esModuleInterop': True, 'resolveJsonModule': True, 'isolatedModules': True,
            'paths': {'~/*': ['./web/*'], **dict(sorted(paths.items())), **{k: ['./' + v] for k, v in base.get('typePaths', {}).items()}},
        },
        'files': checked,
    }
    write_declarations(out, tsconfig, sorted(rel(f) for f in on_stubs))
    json.dump(tsconfig, open(os.path.join(out, 'tsconfig.json'), 'w'), indent=2, ensure_ascii=False)
    json.dump({'~': f"{base['out']}/web", **dict(sorted(aliases.items()))}, open(os.path.join(out, 'aliases.json'), 'w'), indent=2, ensure_ascii=False)
    state = {'transferred': len(r['transfer']), 'excluded': len(excluded), 'code': len(code), 'deferred': len([f for f in code if f in deferred]),
             'replaced': sorted(replaced), 'patched': sorted(patched), 'stubs': sorted(specs),
             'missing_packages': {p: len(fs) for p, fs in sorted(missing.items())}, 'checked': len(checked),
             'on_stubs': sorted(rel(f) for f in on_stubs)}
    json.dump(state, open(os.path.join(out, 'state.json'), 'w'), indent=2, ensure_ascii=False)
    print(f"base: {len(r['transfer'])} files ({len(code)} code, {state['deferred']} deferred, {len(on_stubs)} on stubs), {len(replaced)} replaced, {len(patched)} patched, {len(specs)} stub specs, tsc files {len(checked)}")


if 'base' in manifest:
    sync_base(manifest['base'])
