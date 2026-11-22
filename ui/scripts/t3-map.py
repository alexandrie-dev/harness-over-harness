#!/usr/bin/env python3
"""Пишет ui/T3-MAP.md: что взято из T3 Code (MIT), где лежит у нас, что заменено прослойкой. Только чтение дерева T3.

Запуск: python3 -I ui/scripts/t3-map.py [корень T3]. Таблицы строятся из ui/src/t3/manifest.json и первых строк прослоек;
раздел основы HOH-40 — ещё из src/t3/base/state.json и src/t3/base/tsconfig.json (их пишет scripts/t3-sync.py).
"""
import filecmp, json, os, re, sys
UI = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
T3 = sys.argv[1] if len(sys.argv) > 1 else '/mnt/toshiba/alexandrie/third-party/t3code'
OUT = os.path.join(UI, 'src', 't3')
ROOTS = {'web': 'apps/web/src', 'shared': 'packages/shared/src', 'client-runtime': 'packages/client-runtime/src'}
manifest = json.load(open(os.path.join(OUT, 'manifest.json')))
rows_ok, rows_def, changed = [], [], []
taken = set()
for root, entries in manifest['files'].items():
    for e in entries:
        name, deferred = (e, False) if isinstance(e, str) else (e['file'], e.get('deferred', False))
        taken.add(f'{root}/{name}')
        src, dst = os.path.join(T3, ROOTS[root], name), os.path.join(OUT, root, name)
        same = filecmp.cmp(src, dst, shallow=False)
        if not same: changed.append(f'{ROOTS[root]}/{name}')
        row = f'| `{ROOTS[root]}/{name}` | `ui/src/t3/{root}/{name}` | {"без изменений" if same else "ИЗМЕНЁН"} |'
        (rows_def if deferred else rows_ok).append(row)
shims = []
for root in ('web', 'shared', 'client-runtime', 'shims'):
    for dp, _, names in os.walk(os.path.join(OUT, root)):
        for n in sorted(names):
            rel = os.path.relpath(os.path.join(dp, n), OUT)
            if rel in taken or n.endswith('.d.ts') and False: continue
            head = open(os.path.join(dp, n)).readline()
            m = re.match(r'// Прослойка окна HoH вместо (\S+)(?: \([^)]*\))?:? ?(.*)', head)
            shims.append(f'| `ui/src/t3/{rel}` | {("`" + m.group(1) + "`") if m else "—"} | {(m.group(2) if m else head.lstrip("/ ").strip())} |')
deps = [
 ('react-markdown', '10.1.0', 'разбор и вывод Markdown (основа ChatMarkdown)', 'ChatMarkdown.tsx, MarkdownCodeBlock.logic.ts', 'да: основа рендера чата; прежний marked удалён 20.11.2026'),
 ('unified', '11.0.5', 'конвейер разбора', 'markdownPipeline.ts, markdownRenderer.ts, markdownSyntaxGates.ts, markdownMath.ts, markdownTaskList.ts, markdownWindowsPaths.ts, markdown-incremental.ts, codexMarkdownDirectives.ts и др.', 'нет'),
 ('remark-parse', '11.0.0', 'разбор Markdown в mdast', 'markdownPipeline.ts, markdownRenderer.ts, markdownMath.ts, markdownTaskList.ts, codexMarkdownDirectives.ts', 'нет'),
 ('remark-gfm', '4.0.1', 'таблицы, списки задач, зачёркивание', 'markdownPipeline.ts, markdownMath.ts, markdownTaskList.ts', 'да: GFM-разбор рендера чата; прежний marked удалён 20.11.2026'),
 ('remark-math', '6.0.0', 'формулы $…$ (с подменой micromark-extension-math → micromark-extension-llm-math 3.1.0)', 'ChatMarkdown.tsx, markdownMath.ts', 'нет'),
 ('remark-breaks', '4.0.0', 'переводы строк как в чате', 'markdownPipeline.ts', 'нет (не было в списке ведущего)'),
 ('remark-rehype', '11.1.2', 'mdast → hast', 'markdownPipeline.ts, markdownRenderer.ts', 'нет'),
 ('rehype-raw', '7.0.0', 'сырой HTML в ответе (T3 включает по флагу)', 'markdownPipeline.ts', 'не нужен, если держать parseRawHtml выключенным; код его всё равно импортирует (не было в списке ведущего)'),
 ('rehype-sanitize', '6.0.0', 'очистка HTML', 'markdownPipeline.ts', 'нет (не было в списке ведущего)'),
 ('vfile', '6.0.3', 'файл для unified', 'markdownRenderer.ts', 'нет'),
 ('hast-util-to-jsx-runtime', '2.3.6', 'hast → элементы React', 'markdownRenderer.ts, HighlightedCodeLines.tsx', 'нет'),
 ('hast-util-to-html', '9.0.5', 'hast → HTML (копирование блока кода)', 'MarkdownCodeBlock.tsx, HighlightedCodeLines.tsx', 'нет'),
 ('html-url-attributes', '3.0.1', 'какие атрибуты содержат адреса', 'markdownRenderer.ts', 'нет'),
 ('mdast-util-from-markdown', '2.0.3', 'разбор для проверок синтаксиса', 'markdownSyntaxGates.ts', 'нет'),
 ('micromark-util-combine-extensions', '2.0.1', 'склейка расширений', 'markdownSyntaxGates.ts', 'нет'),
 ('micromark-util-types', '2.0.2', 'типы micromark', 'markdownSyntaxGates.ts, codexMarkdownDirectives.ts', 'только типы'),
 ('micromark-util-character', '2.1.1', 'классы символов', 'codexMarkdownDirectives.ts', 'нет (не было в списке ведущего)'),
 ('micromark-util-decode-string', '2.0.1', 'раскодирование экранирований', 'markdownWindowsPaths.ts, markdown-clipboard.ts', 'нет (не было в списке ведущего)'),
 ('micromark-extension-directive', '4.0.0', 'директивы Codex в тексте', 'codexMarkdownDirectives.ts', 'можно убрать вместе с codexMarkdownDirectives (в окне не нужны), но markdownPipeline.ts его импортирует'),
 ('mdast-util-directive', '3.1.0', 'то же', 'codexMarkdownDirectives.ts', 'то же'),
 ('property-information', '7.2.0', 'имена свойств HTML', 'markdownPipeline.ts', 'нет (не было в списке ведущего)'),
 ('@types/hast, @types/mdast', '3.0.5, 4.0.4', 'типы (в коде T3 это импорты `hast`, `mdast`)', 'markdownRenderer.ts, markdownPipeline.ts и др.', 'только типы'),
 ('katex', '0.18.10', 'формулы (+ katex.min.css и шрифты KaTeX)', 'MathTypeset.tsx, MermaidDiagram.tsx', 'нет. Версия 0.16.47 из списка ведущего — не из окна T3: apps/web просит ^0.18.2, в lock 0.18.10'),
 ('dompurify', '3.4.17', 'очистка SVG диаграмм', 'MermaidDiagram.tsx', 'нет'),
 ('pele', '1.8.0', 'диаграммы (pele/lazy)', 'MermaidDiagram.tsx', 'нет'),
 ('@pierre/diffs', '1.7.0-beta.4 (с патчем)', 'подсветка кода; тянет shiki 4.2.0 и @shikijs/themes 4.2.0', 'diffRendering.ts, incrementalHighlighting.ts, readableSyntaxTheme.ts, syntaxHighlighting.ts, assemblyHighlighting.ts', 'нет'),
]
patches = ['micromark@4.0.2', 'micromark-extension-gfm-table@2.1.1', 'micromark-extension-llm-math@3.1.0', 'mdast-util-math@3.0.0', 'mdast-util-to-hast@13.2.1', '@pierre/diffs@1.7.0-beta.4']
text = f"""# Карта кода T3 Code в окне HoH

Источник: T3 Code, MIT License, Copyright (c) 2026 T3 Tools Inc. (`/mnt/toshiba/alexandrie/third-party/t3code`, коммит 803f21f6, только чтение). Полный текст лицензии — `ui/NOTICE.md`. Названий и знаков T3 в окне нет: перенесены только файлы разметки ответа, в сборку попадают только подключённые.

Файлы переносит `ui/scripts/t3-sync.py` по списку `ui/src/t3/manifest.json` (копия без правок; расхождений с источником сейчас: {len(changed)}). Дерево повторяет T3: `apps/web/src` → `ui/src/t3/web`, `packages/shared/src` → `ui/src/t3/shared`, `packages/client-runtime/src` → `ui/src/t3/client-runtime`. Псевдонимы пакетов (`@t3tools/*`, `gt-react`, `effect/*`) прописывает тот же скрипт в `ui/src/t3/tsconfig.paths.json` (tsc) и `ui/src/t3/aliases.json` (vite). Карту обновляет `ui/scripts/t3-map.py`; целостность импортов проверяет `ui/scripts/t3-check.py`.

## Состояние

- **В сборке сейчас:** рендер ответов на конвейере T3 (marked удалён 20.11.2026) через переключатель `ui/src/t3/render.tsx`. Файлы T3 из первой таблицы проверены компилятором (`tsc --noEmit`), но чат их пока не вызывает.
- **Отложено:** вторая таблица — файлы T3, которым нужны пакеты, которых нет в lock окна. До установки tsc их пропускает (`deferredEnabled: false`), vite их не видит, Tailwind не сканирует (`@source not "../t3"` в tokens.css). Компилятором они не проверены: проверены только адресаты импортов (`t3-check.py`).
- **Основа HOH-40** (`src/t3/base`, раздел «Основа HOH-40» ниже): весь веб-клиент T3 перенесён файлами для этапов 3–5 плана `plans/hoh40-t3-base.md`. В сборку окна пока не входит: старая оболочка работает как раньше.
- **Не перенесено из запрошенного:** шаги инструментов (`TimelineWorkEntryRow` тянет за собой подтверждения и поле ввода T3; нынешние строки шагов окна остаются) и картинки-вложения (`ComposerImageThumbnail`, `GuestMessageAttachments`; в протоколе окна пока нет вложений). Файлов T3 для них в дереве нет.

## Перенесено без изменений и проверено tsc

| Путь в T3 | Путь у нас | Изменения |
|---|---|---|
""" + "\n".join(rows_ok) + """

## Перенесено без изменений, отложено (нужны пакеты)

| Путь в T3 | Путь у нас | Изменения |
|---|---|---|
""" + "\n".join(rows_def) + """

## Прослойки (наши файлы вместо зависимостей T3)

Это не код T3, а тонкие замены того, чего у окна нет: состояния и хоста T3, Effect, gt-react (i18n), contracts. Они лежат по тем же путям, что и заменённые файлы, поэтому перенесённый код читает их без правок.

| Наш файл | Заменяет в T3 | Что делает |
|---|---|---|
""" + "\n".join(shims) + f"""

## Зависимости для установки

Установку решает основатель через supply-gate. Версии — по `t3code/pnpm-lock.yaml` (apps/web). Пакетов, которых нет в окне и которые нужны отложенным файлам:

| Пакет | Версия | Зачем | Какой файл T3 импортирует | Можно ли обойтись нашим |
|---|---|---|---|---|
""" + "\n".join(f'| `{n}` | {v} | {w} | {f} | {a} |' for n, v, w, f, a in deps) + f"""

**Патчи T3 в этой цепочке** (`t3code/patches/`, подключены через `patchedDependencies`): {', '.join('`' + p + '`' for p in patches)}. Без них поведение формул, таблиц и подсветки может отличаться от T3; решение о переносе патчей (в `ui/patches/` и `pnpm-workspace.yaml`) тоже за основателем. Подмена `remark-math>micromark-extension-math → micromark-extension-llm-math@3.1.0` задаётся там же (`overrides`).

Уже есть у окна и подходит: `react`, `react-dom`, `lucide-react`, `@base-ui/react` (кнопка, подсказка, меню T3 стоят на нём), `class-variance-authority`, `tailwind-merge`, `zustand`. `uplot` T3 не использует (`marked` из окна удалён 20.11.2026). `@legendapp/list` у T3 тот же, что у нас.

Прочие импорты T3 (`effect`, `@effect/atom-react`, `gt-react`, `@t3tools/*`, `@tiptap/*`, `@tanstack/*`) заменены прослойками или в перенесённую часть не попадают.
"""
base = manifest.get('base')
if base:
    sys.dont_write_bytecode = True
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import t3graph
    out = os.path.join(UI, base['out']); state = json.load(open(os.path.join(out, 'state.json')))
    checked = set(json.load(open(os.path.join(out, 'tsconfig.json')))['files'])
    r = t3graph.analyse(T3)
    rel = lambda f: next(f'{k}/{f[len(v) + 1:]}' for k, v in t3graph.ROOTS.items() if f.startswith(v + '/'))
    edited = set(base.get('replaced', {})) | set(base.get('patched', {}))
    dir_rows, same_total, diff_unexpected = [], 0, []
    for root, src in t3graph.ROOTS.items():
        fs = [f for f in r['transfer'] if f.startswith(src + '/')]
        code = [f for f in fs if f.endswith(t3graph.CODE)]
        same = [f for f in fs if os.path.isfile(os.path.join(out, rel(f))) and filecmp.cmp(os.path.join(T3, f), os.path.join(out, rel(f)), shallow=False)]
        diff_unexpected += [rel(f) for f in fs if f not in same and rel(f) not in edited]
        same_total += len(same)
        deferred = [f for f in code if rel(f) not in checked]
        dir_rows.append(f'| `{src}` | `ui/{base["out"]}/{root}` | {len(fs)} | {len(code)} | {len(same)} | {len([f for f in fs if rel(f) in edited])} | {len(deferred)} |')
    edit_rows = [f'| `{x}` | замена (наш файл) | {why} |' for x, why in base.get('replaced', {}).items()]
    edit_rows += [f'| `{x}` | правка (`ui/{base["patches"]}/{x}.patch`) | {why} |' for x, why in base.get('patched', {}).items()]
    switch_rows = [f'| {a} | {b} | {c} |' for a, b, c in base.get('switches', [])]
    stub_rows = []
    for kind, sub in (('прослойка (наш файл)', 'shims'), ('прослойка типов (наш файл)', 'types'), ('заглушка (создана t3-sync.py)', 'stubs')):
        for dp, _, ns in sorted(os.walk(os.path.join(out, sub))):
            for n in sorted(ns):
                path = os.path.join(dp, n); x = os.path.relpath(path, out)
                if n in ('absent.ts', 'absent-url.ts'): continue
                head = open(path).readline()
                m = re.search(r'вместо `([^`]+)` \(T3 Code, MIT\): (.*?)(?:\. Пакет не ставится|$)', head)
                stub_rows.append(f'| `{m.group(1) if m else x}` | {kind} | `ui/{base["out"]}/{x}` | {(m.group(2).strip() if m else head.lstrip("/ ").strip())} |')
    sizes = {}
    for f in r['transfer']:
        ext = os.path.splitext(f)[1] or os.path.basename(f)
        sizes[ext] = sizes.get(ext, 0) + os.path.getsize(os.path.join(T3, f))
    mb = lambda b: f'{b / 1e6:.1f} МБ'
    big = ', '.join(f'{e} — {mb(b)}' for e, b in sorted(sizes.items(), key=lambda x: -x[1])[:6])
    missing = ', '.join(f'`{p}` ({n})' for p, n in state['missing_packages'].items()) or 'нет'
    text += f"""
## Основа HOH-40: весь веб-клиент T3 (`ui/{base['out']}`)

План — `plans/hoh40-t3-base.md` (этап 2). Переносит `ui/scripts/t3-sync.py` по разделу `base` манифеста: весь `apps/web/src` без тестов
(`*.test.*`, `*.browser.test.*`, `*.bench.*`) и файлы `packages/{{contracts,shared,client-runtime}}/src`, до которых дотягиваются его импорты
(граф — `ui/scripts/t3graph.py`), плюс `apps/web/gt.config.json` → `ui/{base['out']}/gt.config.json` (его читает `i18n/locales.ts`).
Свой проект tsc — `ui/{base['out']}/tsconfig.json` (настройки T3, псевдонимы `~/*` и `@t3tools/*` по `exports`; прежний tsconfig основу
не видит), псевдонимы для vite — `ui/{base['out']}/aliases.json`. В сборку окна основа пока не входит (э3), Tailwind её не сканирует
(`@source not "../t3"`). Проверка импортов — `ui/scripts/t3-check.py` (оба набора).

Файлов перенесено: **{state['transferred']}**, из них без изменений: **{same_total}**, правок файлов T3: **{len(edited)}** (таблица ниже){'' if not diff_unexpected else ', НЕОЖИДАННО ОТЛИЧАЮТСЯ: ' + ', '.join(diff_unexpected)}.
Отложено (не проверяет tsc: в замыкании импортов есть пакет, которого нет в `ui/package.json` и `ui/node_modules` в версии, совместимой с lock T3: тот же major, для 0.x — тот же minor): **{state['deferred']}** из {state['code']} файлов кода;
не хватает пакетов (сколько файлов T3 их импортирует): {missing}. Когда пакеты из `ui/T3-DEPS.md` (раздел HOH-40) стоят, отложенных нет —
`t3-sync.py` пересчитывает список сам. tsc основы: `noImplicitAny` выключен — код скрытых частей получает из заглушек значения `any`,
и его колбэки без явных типов (у T3 этот код проверен с настоящими пакетами); остальные настройки — как в T3 (`strict`,
`noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`).

| Папка T3 | Папка у нас | Файлов | Из них кода | Без изменений | Правок | Отложено |
|---|---|---|---|---|---|---|
""" + '\n'.join(dir_rows) + """

### Правки файлов T3

| Файл (в `ui/""" + base['out'] + """`) | Как | Зачем |
|---|---|---|
""" + '\n'.join(edit_rows) + """

Имя продукта: `APP_BASE_NAME` и `APP_DISPLAY_NAME` (`web/branding.ts`) T3 берёт из `window.desktopBridge.getAppBranding()` — имя HoH передаст
хост (э3), правка не нужна. В текстах T3 имя «T3 Code» встречается и буквально: в коде `web` — """ + str(sum(1 for dp, _, ns in os.walk(os.path.join(out, 'web')) for n in ns if n.endswith(('.ts', '.tsx')) and 'i18n' not in dp for line in open(os.path.join(dp, n), encoding='utf-8') if 'T3 Code' in line and not line.lstrip().startswith(('//', '*', '/*')))) + """ строк, в русском переводе `web/i18n/translations/ru.json` — """ + str(open(os.path.join(out, 'web/i18n/translations/ru.json'), encoding='utf-8').read().count('T3 Code')) + """; большая часть — в скрытых частях. Замена в видимых текстах — э4 (через перевод, без правки файлов кода).

### Скрытые части: чем выключены

Файлы скрытых частей остаются в дереве. Выключатель — правка из таблицы выше, возможность окружения (`ExecutionEnvironmentCapabilities`
в `contracts/environment.ts`), которую объявляет хост HoH (э3), или переменная сборки, которой в окне нет.

| Часть | Чем выключена | Состояние |
|---|---|---|
""" + '\n'.join(switch_rows) + """

### Заглушки и прослойки пакетов

Пакеты скрытых частей и пакеты T3, которые не берутся (список и причины — `ui/T3-DEPS.md`, «Заглушено вместо установки»). Созданная
заглушка отдаёт значения, которые можно вызвать, создать через `new` и читать (`stubs/absent.ts`); типы — `any` с пространством имён
для обращений через точку. Где коду нужны точные типы или поведение, вместо созданной лежит наша прослойка (`shims/`, `types/`).
Адреса файлов пакетов (`?url`) указывают на `stubs/absent-url.ts`. Для видимых импортёров (`@clerk/react`, `@t3tools/provider-*`,
`@t3tools/source-control-*`, `@t3tools/issue-tracker-linear`, `@t3tools/secret-provider-definitions`) в э3 нужны рабочие прослойки.

| Пакет (путь импорта) | Что | Файл | Почему |
|---|---|---|---|
""" + '\n'.join(stub_rows) + f"""

### Тема, шрифты, значки

- Тема — `web/index.css` T3 без правок (переменные, цвета, радиусы, тени, размеры текста, отступы; `@import "tailwindcss"`), подключается
  из `web/main.tsx` как у T3; других CSS-импортов у неё нет (`components/files/pdfViewer.css` — у просмотра PDF).
- Шрифт — Inter Variable из пакета `@fontsource-variable/inter` (лицензия SIL OFL 1.1: встраивать и распространять с программой можно),
  его импортирует `web/main.tsx`; пакет уже есть в окне. Моноширинный — системный (`--font-mono` T3). Шрифт значков терминала
  `web/terminal/ghostty/fonts/SymbolsNerdFontMono-Regular.woff2` — MIT (Ryan L McIntyre, файл `LICENSE` рядом), перенесён с терминалом (скрыт).
- Значки — `lucide-react` (ISC) и встроенные SVG компонентов T3; логотипы MCP-приложений (`web/components/mcp/logos/*.png`) перенесены
  с MCP Apps (скрыты). Значки приложения T3 (`apps/web/public/*`, `assets/prod/*`) не берутся: знак — эмблема HoH (`src/assets/emblem-*.png`).
- Объём перенесённого: {mb(sum(sizes.values()))}; крупнее всего — {big}. Переводы T3 (`web/i18n/translations/*.json`) — все 10 языков, как у T3.
"""

open(os.path.join(UI, 'T3-MAP.md'), 'w').write(text)
print('T3-MAP.md:', len(rows_ok), 'в сборке,', len(rows_def), 'отложено,', len(shims), 'прослоек,', len(changed), 'изменено' + (f"; основа: {state['transferred']} файлов, {same_total} без изменений, {len(edited)} правок, {state['deferred']} отложено" if base else ''))
