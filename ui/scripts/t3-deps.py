#!/usr/bin/env python3
"""Пишет ui/T3-DEPS.md: npm-пакеты T3 Code, версии и число транзитивных пакетов по snapshots pnpm-lock.yaml T3.

Два раздела: прежний (рендеринг чата, 20.11) и «HOH-40» — весь веб-клиент T3 как основа окна: пакеты видимой части
по замыканию импортов от apps/web/src/main.tsx (scripts/t3graph.py) и пакеты, которые заменяются заглушками.
Запуск: python3 -I ui/scripts/t3-deps.py [корень T3]. Только чтение; ничего не ставит и не ходит в сеть.
"""
import json, os, re, sys

def parse(path,start_re=r'^snapshots:'):
    lines=open(path).read().split('\n'); snaps={}; i=0
    # last 'snapshots:' top-level section
    idx=[n for n,l in enumerate(lines) if l=='snapshots:'][-1]
    cur=None; sect=None
    for l in lines[idx+1:]:
        if l and not l.startswith(' '): break
        m=re.match(r"^  '?([^\s'][^']*?)'?:\s*(\{\})?$",l)
        if m and not l.startswith('   '):
            cur=m.group(1); snaps[cur]=[]; sect=None; continue
        if cur is None: continue
        if re.match(r'^    (dependencies|optionalDependencies):',l): sect='d'; continue
        if re.match(r'^    \S',l): sect=None; continue
        m=re.match(r"^      '?([^\s'][^']*?)'?: (.+)$",l)
        if m and sect: snaps[cur].append((m.group(1),m.group(2).strip("'")))
    return snaps
def key(name,ver):
    # ver may be 'npm:alias@x' or '1.2.3(peer)'
    if ver.startswith('npm:'): return ver[4:]
    return name+'@'+ver
def closure(snaps,start):
    seen=set(); st=[start]
    while st:
        k=st.pop()
        if k in seen: continue
        seen.add(k)
        for n,v in snaps.get(k,[]):
            kk=key(n,v)
            if kk not in snaps:
                # try without peer suffix mismatch
                c=[s for s in snaps if s.split('(')[0]==kk.split('(')[0]]
                kk=c[0] if c else kk
            st.append(kk)
    return seen

UI = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
T3 = sys.argv[1] if len(sys.argv) > 1 else '/mnt/toshiba/alexandrie/third-party/t3code'
S = parse(T3 + '/pnpm-lock.yaml'); O = parse(UI + '/pnpm-lock.yaml')
ours = {k.split('(')[0] for k in O}
# (имя, версия, зачем, файлы T3)
DIRECT = [
 ('react-markdown','10.1.0','разбор и вывод Markdown (основа ChatMarkdown)','ChatMarkdown.tsx, MarkdownCodeBlock.logic.ts'),
 ('unified','11.0.5','конвейер разбора','markdownPipeline.ts, markdownRenderer.ts, markdownSyntaxGates.ts, markdown-incremental.ts, markdownMath.ts, markdownTaskList.ts, markdownWindowsPaths.ts, codexMarkdownDirectives.ts'),
 ('remark-parse','11.0.0','разбор Markdown в mdast','markdownPipeline.ts, markdownRenderer.ts, markdownMath.ts, markdownTaskList.ts, codexMarkdownDirectives.ts'),
 ('remark-gfm','4.0.1','таблицы, списки задач, зачёркивание','markdownPipeline.ts, markdownMath.ts, markdownTaskList.ts'),
 ('remark-math','6.0.0','формулы $…$ (подмена micromark-extension-math → micromark-extension-llm-math 3.1.0)','ChatMarkdown.tsx, markdownMath.ts'),
 ('micromark-extension-llm-math','3.1.0','расширение формул (T3 подставляет вместо micromark-extension-math, с патчем)','через remark-math'),
 ('remark-breaks','4.0.0','переводы строк как в чате','markdownPipeline.ts'),
 ('remark-rehype','11.1.2','mdast → hast','markdownPipeline.ts, markdownRenderer.ts'),
 ('rehype-raw','7.0.0','сырой HTML в ответе (по флагу)','markdownPipeline.ts'),
 ('rehype-sanitize','6.0.0','очистка HTML','markdownPipeline.ts'),
 ('vfile','6.0.3','файл для unified','markdownRenderer.ts'),
 ('hast-util-to-jsx-runtime','2.3.6','hast → элементы React','markdownRenderer.ts, HighlightedCodeLines.tsx'),
 ('hast-util-to-html','9.0.5','hast → HTML (копирование блока кода)','MarkdownCodeBlock.tsx, HighlightedCodeLines.tsx'),
 ('html-url-attributes','3.0.1','какие атрибуты содержат адреса','markdownRenderer.ts'),
 ('mdast-util-from-markdown','2.0.3','разбор для проверок синтаксиса','markdownSyntaxGates.ts'),
 ('micromark-util-combine-extensions','2.0.1','склейка расширений','markdownSyntaxGates.ts'),
 ('micromark-util-types','2.0.2','типы micromark','markdownSyntaxGates.ts, codexMarkdownDirectives.ts'),
 ('micromark-util-character','2.1.1','классы символов','codexMarkdownDirectives.ts'),
 ('micromark-util-decode-string','2.0.1','раскодирование экранирований','markdownWindowsPaths.ts, markdown-clipboard.ts'),
 ('micromark-extension-directive','4.0.0','директивы Codex в тексте','codexMarkdownDirectives.ts'),
 ('mdast-util-directive','3.1.0','то же','codexMarkdownDirectives.ts'),
 ('property-information','7.2.0','имена свойств HTML','markdownPipeline.ts'),
 ('@types/hast','3.0.5','типы (в T3 импорты `hast`)','markdownRenderer.ts и др.'),
 ('@types/mdast','4.0.4','типы (в T3 импорты `mdast`)','markdownPipeline.ts и др.'),
 ('katex','0.18.10','формулы (+ katex.min.css, шрифты KaTeX)','MathTypeset.tsx, MermaidDiagram.tsx'),
 ('dompurify','3.4.17','очистка SVG диаграмм','MermaidDiagram.tsx'),
 ('pele','1.8.0','диаграммы (pele/lazy)','MermaidDiagram.tsx'),
 ('@pierre/diffs','1.7.0-beta.4','подсветка кода (с патчем; тянет shiki 4.2.0, @shikijs/themes 4.2.0)','diffRendering.ts, incrementalHighlighting.ts, readableSyntaxTheme.ts, syntaxHighlighting.ts, assemblyHighlighting.ts'),
]
def keys_of(name, ver):
    return sorted((k for k in S if k.split('(')[0] == f'{name}@{ver}'), key=lambda k: (-len(S[k]), k))
allset, rows = set(), []
for name, ver, why, files in DIRECT:
    cl = set()
    for k in keys_of(name, ver): cl |= closure(S, k)
    base = {k.split('(')[0] for k in cl} | {f'{name}@{ver}'}
    allset |= base
    trans = base - {f'{name}@{ver}'}
    rows.append(f'| `{name}` | {ver} | не проверено | {why} | {files} | {len(trans)} |')
new = sorted(allset - ours)
text = f"""# Пакеты T3 Code для рендеринга чата

Версии и число транзитивных пакетов — по `{T3}/pnpm-lock.yaml` (раздел `snapshots`, рекурсивно по `dependencies` и `optionalDependencies`, без повторов, без учёта вариантов с разными peer-суффиксами). Лицензия «не проверено»: в `t3code` нет `node_modules`, а lock лицензий не хранит; проверка лицензий — работа supply-gate при установке. Ничего не установлено, `ui/package.json` и lock не менялись. Таблицу строит `ui/scripts/t3-deps.py`.

| Пакет | Версия | Лицензия | Зачем | Какой файл T3 импортирует | Транзитивных пакетов |
|---|---|---|---|---|---|
""" + "\n".join(rows) + f"""

**Всего уникальных пакетов** (прямые + транзитивные): **{len(allset)}**; из них уже есть в lock окна: {len(allset & ours)}, новых: **{len(new)}**.

Патчи T3 в этой цепочке (`t3code/patches/`, `patchedDependencies`): micromark@4.0.2, micromark-extension-gfm-table@2.1.1, micromark-extension-llm-math@3.1.0, mdast-util-math@3.0.0, mdast-util-to-hast@13.2.1, @pierre/diffs@1.7.0-beta.4. Подмена `remark-math>micromark-extension-math` → `micromark-extension-llm-math@3.1.0` — в `overrides` T3.

## Новые пакеты целиком ({len(new)})

""" + ", ".join(f"`{n}`" for n in new) + "\n"

# ───── HOH-40: весь веб-клиент T3 (plans/hoh40-t3-base.md, этап 1) ─────
sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import t3graph

def peers(path):
    """{имя@версия: имена peerDependencies} из раздела packages: — peer ставит тот, кто использует пакет, а не он сам."""
    lines = open(path).read().split('\n')
    start = [n for n, l in enumerate(lines) if l == 'packages:'][-1]
    out, cur, inside = {}, None, False
    for l in lines[start + 1:]:
        if l and not l.startswith(' '): break
        m = re.match(r"^  '?([^\s'][^']*?)'?:$", l)
        if m: cur = m.group(1); inside = False; continue
        if re.match(r'^    peerDependencies:', l): inside = True; continue
        if re.match(r'^    \S', l): inside = False; continue
        m = re.match(r"^      '?([^\s'][^']*?)'?: ", l)
        if m and inside and cur: out.setdefault(cur, set()).add(m.group(1))
    return out

def optionals(path):
    """{снимок: имена optionalDependencies} из snapshots: lock T3 (окно их не ставит: ignoredOptionalDependencies '*')."""
    lines = open(path).read().split('\n')
    start = [n for n, l in enumerate(lines) if l == 'snapshots:'][-1]
    out, cur, inside = {}, None, False
    for l in lines[start + 1:]:
        if l and not l.startswith(' '): break
        m = re.match(r"^  '?([^\s'][^']*?)'?:\s*(\{\})?$", l)
        if m and not l.startswith('   '): cur = m.group(1); inside = False; continue
        if re.match(r'^    optionalDependencies:', l): inside = True; continue
        if re.match(r'^    \S', l): inside = False; continue
        m = re.match(r"^      '?([^\s'][^']*?)'?: ", l)
        if m and inside and cur: out.setdefault(cur, set()).add(m.group(1))
    return out

def closure_no_peers(start):
    seen, stack = set(), [start]
    while stack:
        k = stack.pop()
        if k in seen: continue
        seen.add(k)
        skip = PEERS.get(k.split('(')[0], set()) | OPTIONAL.get(k, set())
        for n, v in S.get(k, []):
            if n in skip: continue
            kk = key(n, v)
            if kk not in S:
                c = [x for x in S if x.split('(')[0] == kk.split('(')[0]]
                kk = c[0] if c else kk
            stack.append(kk)
    return seen

def lock_overrides(path):
    text = open(path).read()
    part = text[text.rindex('\noverrides:\n') + 12:]
    return dict((k.strip("'"), v.strip("'")) for k, v in re.findall(r"^  ('[^']+'|[^\s:]+): (\S+)$", part.split('\n\n')[0], re.M))

IMP = t3graph.importers(T3 + '/pnpm-lock.yaml')
ORDER = t3graph.IMPORTER_ORDER
PEERS = peers(T3 + '/pnpm-lock.yaml'); OPTIONAL = optionals(T3 + '/pnpm-lock.yaml')
def t3_full(name):
    return next((IMP[i][name] for i in ORDER if name in IMP.get(i, {})), None)
def t3_version(name):
    full = t3_full(name)
    return full.split('(')[0] if full else None
def types_pkg(name):
    return '@types/' + (name[1:].replace('/', '__') if name.startswith('@') else name)
WINDOW_PKG = json.load(open(os.path.join(UI, 'package.json')))
def window_version(name):
    """Версия в ui/package.json (закоммиченное состояние окна; node_modules может меняться во время установки)."""
    return {**WINDOW_PKG.get('devDependencies', {}), **WINDOW_PKG.get('dependencies', {})}.get(name)
ws_text = open(T3 + '/pnpm-workspace.yaml').read()
PATCHES = dict(re.findall(r'^  "?([^":\s]+@[^":\s]+)"?: (patches/\S+)$', ws_text.split('patchedDependencies:')[1].split('\n\n')[0], re.M))
OVERRIDES = lock_overrides(T3 + '/pnpm-lock.yaml')

r = t3graph.analyse(T3)
def short(f): return f.replace('apps/web/src/', '').replace('packages/', '')
def who(files, limit=3):
    fs = sorted(files, key=lambda f: (len(f), f)); more = len(fs) - limit
    return ', '.join(short(f) for f in fs[:limit]) + (f' и ещё {more}' if more > 0 else '')

install = dict(r['install'])
for name in list(install):
    t = types_pkg(name)
    if t not in install and any(t in IMP.get(i, {}) for i in ORDER): install[t] = {f: {'type'} for f in install[name]}
rows, union, no_version = [], set(), []
for name in sorted(install):
    ver = t3_version(name)
    if not ver: no_version.append(name); continue
    start = f'{name}@{t3_full(name)}'
    cl = closure_no_peers(start if start in S else (keys_of(name, ver) or [start])[0])
    base = {k.split('(')[0] for k in cl} | {f'{name}@{ver}'}
    union |= base
    trans = base - {f'{name}@{ver}'}
    names = {b.rsplit('@', 1)[0] for b in base}
    patched = sorted(k for k in PATCHES if k in base)
    def applies(k):
        *parent, target = k.split('>')
        return (target in names and (not parent or parent[0] in names)) or (parent and parent[0] == name)
    pinned = sorted(f'{k} → {v}' for k, v in OVERRIDES.items() if applies(k))
    note = '; '.join(([f'патч {", ".join(patched)}'] if patched else []) + ([f'закрепление {", ".join(pinned)}'] if pinned else [])) or '—'
    kinds = set().union(*install[name].values())
    users = who(install[name]) + (' (только типы)' if kinds == {'type'} else '')
    have = window_version(name)
    state = 'нет' if not have else ('есть' if have == ver else f'есть {have}, нужна {ver}')
    rows.append(f'| `{name}` | {ver} | {state} | {note} | {users} | {len(trans)} ({len(trans - ours)}) |')
new_union = sorted(union - ours)
to_add = [x for x in rows if '| нет |' in x or 'нужна' in x]

# Файл без своей скрытой части относится к тем скрытым частям, из файлов которых он достижим.
reach = {}
for h in r['transfer']:
    part = t3graph.hidden_part(h)
    if not part: continue
    for f in r['graph'].closure([h], cut=lambda x, part=part: t3graph.hidden_part(x) not in (None, part))[0]:
        if not t3graph.hidden_part(f): reach.setdefault(f, set()).add(part)
def parts_of(f, name):
    own = t3graph.hidden_part(f)
    return {own} if own else (reach.get(f) or {t3graph.FORCE_STUB.get(name, 'не достижим из видимой части')})
stub_rows = []
for name, files in sorted(r['stubbed'].items()):
    if name in t3graph.FORCE_STUB:  # видимые импортёры есть: часть — та, что назначена, и собственные части скрытых импортёров
        parts = sorted({t3graph.FORCE_STUB[name]} | {t3graph.hidden_part(f) for f in files if t3graph.hidden_part(f)})
    else:
        parts = sorted(set().union(*(parts_of(f, name) for f in files)))
    seen = [f for f in files if f in r['visible']]
    stub_rows.append(f'| `{name}` | {t3_version(name) or "—"} | {"; ".join(parts)} | {who(files)} | {who(seen) if seen else "—"} |')
for name, files in sorted(r['ws'].items()):
    seen = [f for f in files if f in r['visible']]
    specs = sorted({s for v in files.values() for s in v})
    stub_rows.append(f'| `{name}` ({", ".join("`" + x + "`" for x in specs)}) | пакет T3 | {t3graph.ws_reason(name)} | {who(files)} | {who(seen) if seen else "—"} |')
hidden_rows = '\n'.join(f'| {name} | ' + ', '.join('`' + p.replace('apps/web/src/', '').replace('packages/', '') + '`' for p in pats) + ' |' for name, pats in t3graph.HIDDEN)
missing = r['graph'].missing()

hoh40 = f"""
## HOH-40: весь веб-клиент T3 как основа окна

Строит `ui/scripts/t3-deps.py` (граф — `ui/scripts/t3graph.py`) по `{T3}` @ 803f21f6: замыкание импортов от `apps/web/src/main.tsx`
(`AppRoot`, `router.ts`, `routeTree.gen.ts`, все маршруты) по статическим, динамическим и типовым импортам, через `~/…`, относительные пути
и `@t3tools/{{contracts,shared,client-runtime}}` (по `exports` их package.json). В файлы скрытых частей (таблица ниже) обход не заходит:
файлы остаются в дереве, а их пакеты заменяются заглушками. Версии — `importers` lock T3 (`apps/web`, затем `packages/shared`,
`packages/client-runtime`, `packages/contracts`); транзитивные — по `snapshots` без peer- и необязательных зависимостей (окно их не ставит: `autoInstallPeers: false`, `ignoredOptionalDependencies: '*'`), в скобках — сколько из них нет в lock окна. Патчи и закрепления —
`patchedDependencies` и `overrides` из `pnpm-workspace.yaml` T3; `--ignore-scripts` (в окне `ignoreScripts: true`).

Файлов T3 к переносу: **{len(r['transfer'])}** (весь `apps/web/src` без тестов и нужные файлы packages), из них в видимом замыкании **{len(r['visible'])}**,
файлов скрытых частей, до которых доходит видимая часть: {len(r['cut_reached'])}. Импортов без адресата в T3: {len(missing)}
({'; '.join(f'`{short(f)}` → `{s}`' for f, s in missing) or 'нет'}).

### Пакеты к установке

Прямых пакетов: **{len(rows)}**; из них нет в окне или нужна другая версия: **{len(to_add)}**. Уникальных пакетов с транзитивными: {len(union)}, из них новых для lock окна: **{len(new_union)}**.

| Пакет | Версия (lock T3) | В окне сейчас (ui/package.json) | Патч / закрепление T3 | Какой перенесённый файл импортирует | Транзитивных (новых) |
|---|---|---|---|---|---|
""" + '\n'.join(rows) + f"""

Для сборки (не импорты кода): `vite`, `@vitejs/plugin-react` (T3 собирает с `react({{ compiler: true }})`, React Compiler), `oxc-transform-react`,
`tailwindcss` и `@tailwindcss/vite` — уже есть в окне. `@tanstack/router-plugin` не нужен: `routeTree.gen.ts` лежит в дереве T3 готовым.
{('Нет версии в lock T3: ' + ', '.join(no_version) + '.') if no_version else ''}

### Заглушено вместо установки

Пакеты, которые импортируют только скрытые части, и пакеты T3, которые не берутся (план: провайдеры, source-control, секреты, трекеры).
Заглушка — наш файл через `ui/src/t3/base/aliases.json` и `tsconfig.paths.json` (как `gt-react` у прежнего переноса). Где есть видимые
импортёры, заглушка должна быть рабочей (отвечать «части нет»), а не пустой.

| Пакет | Версия (lock T3) | Скрытая часть | Кто импортирует | Видимые импортёры |
|---|---|---|---|---|
""" + '\n'.join(stub_rows) + f"""

Всего заглушено: **{len(stub_rows)}** ({len(r['stubbed'])} npm-пакетов и {len(r['ws'])} пакетов T3).

### Скрытые части (обход в них не заходит)

| Часть | Файлы T3 (шаблоны путей) |
|---|---|
""" + hidden_rows + f"""

Новые пакеты целиком ({len(new_union)}): """ + ', '.join(f'`{n}`' for n in new_union) + '\n'

open(os.path.join(UI, 'T3-DEPS.md'), 'w').write(text + hoh40)
print('T3-DEPS.md:', len(allset), 'unique,', len(new), 'new; HOH-40:', len(rows), 'direct,', len(to_add), 'to add,', len(new_union), 'new with transitive,', len(stub_rows), 'stubbed')
