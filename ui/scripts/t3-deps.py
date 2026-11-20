#!/usr/bin/env python3
"""Пишет ui/T3-DEPS.md: npm-пакеты T3 Code для рендеринга чата, версии и число транзитивных пакетов по snapshots pnpm-lock.yaml T3.

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
open(os.path.join(UI, 'T3-DEPS.md'), 'w').write(text)
print('T3-DEPS.md:', len(allset), 'unique,', len(new), 'new')
