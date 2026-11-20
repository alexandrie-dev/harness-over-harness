#!/usr/bin/env python3
"""Пишет ui/T3-MAP.md: что взято из T3 Code (MIT), где лежит у нас, что заменено прослойкой. Только чтение дерева T3.

Запуск: python3 -I ui/scripts/t3-map.py [корень T3]. Таблицы строятся из ui/src/t3/manifest.json и первых строк прослоек.
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
open(os.path.join(UI, 'T3-MAP.md'), 'w').write(text)
print('T3-MAP.md:', len(rows_ok), 'в сборке,', len(rows_def), 'отложено,', len(shims), 'прослоек,', len(changed), 'изменено')
