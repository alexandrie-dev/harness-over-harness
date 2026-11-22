# Пакеты T3 Code для рендеринга чата

Версии и число транзитивных пакетов — по `/mnt/toshiba/alexandrie/third-party/t3code/pnpm-lock.yaml` (раздел `snapshots`, рекурсивно по `dependencies` и `optionalDependencies`, без повторов, без учёта вариантов с разными peer-суффиксами). Лицензия «не проверено»: в `t3code` нет `node_modules`, а lock лицензий не хранит; проверка лицензий — работа supply-gate при установке. Ничего не установлено, `ui/package.json` и lock не менялись. Таблицу строит `ui/scripts/t3-deps.py`.

| Пакет | Версия | Лицензия | Зачем | Какой файл T3 импортирует | Транзитивных пакетов |
|---|---|---|---|---|---|
| `react-markdown` | 10.1.0 | не проверено | разбор и вывод Markdown (основа ChatMarkdown) | ChatMarkdown.tsx, MarkdownCodeBlock.logic.ts | 86 |
| `unified` | 11.0.5 | не проверено | конвейер разбора | markdownPipeline.ts, markdownRenderer.ts, markdownSyntaxGates.ts, markdown-incremental.ts, markdownMath.ts, markdownTaskList.ts, markdownWindowsPaths.ts, codexMarkdownDirectives.ts | 10 |
| `remark-parse` | 11.0.0 | не проверено | разбор Markdown в mdast | markdownPipeline.ts, markdownRenderer.ts, markdownMath.ts, markdownTaskList.ts, codexMarkdownDirectives.ts | 43 |
| `remark-gfm` | 4.0.1 | не проверено | таблицы, списки задач, зачёркивание | markdownPipeline.ts, markdownMath.ts, markdownTaskList.ts | 69 |
| `remark-math` | 6.0.0 | не проверено | формулы $…$ (подмена micromark-extension-math → micromark-extension-llm-math 3.1.0) | ChatMarkdown.tsx, markdownMath.ts | 54 |
| `micromark-extension-llm-math` | 3.1.0 | не проверено | расширение формул (T3 подставляет вместо micromark-extension-math, с патчем) | через remark-math | 9 |
| `remark-breaks` | 4.0.0 | не проверено | переводы строк как в чате | markdownPipeline.ts | 17 |
| `remark-rehype` | 11.1.2 | не проверено | mdast → hast | markdownPipeline.ts, markdownRenderer.ts | 25 |
| `rehype-raw` | 7.0.0 | не проверено | сырой HTML в ответе (по флагу) | markdownPipeline.ts | 34 |
| `rehype-sanitize` | 6.0.0 | не проверено | очистка HTML | markdownPipeline.ts | 5 |
| `vfile` | 6.0.3 | не проверено | файл для unified | markdownRenderer.ts | 3 |
| `hast-util-to-jsx-runtime` | 2.3.6 | не проверено | hast → элементы React | markdownRenderer.ts, HighlightedCodeLines.tsx | 70 |
| `hast-util-to-html` | 9.0.5 | не проверено | hast → HTML (копирование блока кода) | MarkdownCodeBlock.tsx, HighlightedCodeLines.tsx | 30 |
| `html-url-attributes` | 3.0.1 | не проверено | какие атрибуты содержат адреса | markdownRenderer.ts | 0 |
| `mdast-util-from-markdown` | 2.0.3 | не проверено | разбор для проверок синтаксиса | markdownSyntaxGates.ts | 35 |
| `micromark-util-combine-extensions` | 2.0.1 | не проверено | склейка расширений | markdownSyntaxGates.ts | 3 |
| `micromark-util-types` | 2.0.2 | не проверено | типы micromark | markdownSyntaxGates.ts, codexMarkdownDirectives.ts | 0 |
| `micromark-util-character` | 2.1.1 | не проверено | классы символов | codexMarkdownDirectives.ts | 2 |
| `micromark-util-decode-string` | 2.0.1 | не проверено | раскодирование экранирований | markdownWindowsPaths.ts, markdown-clipboard.ts | 6 |
| `micromark-extension-directive` | 4.0.0 | не проверено | директивы Codex в тексте | codexMarkdownDirectives.ts | 17 |
| `mdast-util-directive` | 3.1.0 | не проверено | то же | codexMarkdownDirectives.ts | 54 |
| `property-information` | 7.2.0 | не проверено | имена свойств HTML | markdownPipeline.ts | 0 |
| `@types/hast` | 3.0.5 | не проверено | типы (в T3 импорты `hast`) | markdownRenderer.ts и др. | 1 |
| `@types/mdast` | 4.0.4 | не проверено | типы (в T3 импорты `mdast`) | markdownPipeline.ts и др. | 1 |
| `katex` | 0.18.10 | не проверено | формулы (+ katex.min.css, шрифты KaTeX) | MathTypeset.tsx, MermaidDiagram.tsx | 1 |
| `dompurify` | 3.4.17 | не проверено | очистка SVG диаграмм | MermaidDiagram.tsx | 1 |
| `pele` | 1.8.0 | не проверено | диаграммы (pele/lazy) | MermaidDiagram.tsx | 0 |
| `@pierre/diffs` | 1.7.0-beta.4 | не проверено | подсветка кода (с патчем; тянет shiki 4.2.0, @shikijs/themes 4.2.0) | diffRendering.ts, incrementalHighlighting.ts, readableSyntaxTheme.ts, syntaxHighlighting.ts, assemblyHighlighting.ts | 54 |

**Всего уникальных пакетов** (прямые + транзитивные): **159**; из них уже есть в lock окна: 150, новых: **9**.

Патчи T3 в этой цепочке (`t3code/patches/`, `patchedDependencies`): micromark@4.0.2, micromark-extension-gfm-table@2.1.1, micromark-extension-llm-math@3.1.0, mdast-util-math@3.0.0, mdast-util-to-hast@13.2.1, @pierre/diffs@1.7.0-beta.4. Подмена `remark-math>micromark-extension-math` → `micromark-extension-llm-math@3.1.0` — в `overrides` T3.

## Новые пакеты целиком (9)

`@pierre/highlights@0.5.1`, `@types/trusted-types@2.0.7`, `commander@8.3.0`, `has-flag@4.0.0`, `katex@0.16.47`, `micromark-extension-math@micromark-extension-llm-math@3.1.0`, `react-dom@19.3.0`, `react@19.3.0`, `supports-color@8.1.1`

## HOH-40: весь веб-клиент T3 как основа окна

Строит `ui/scripts/t3-deps.py` (граф — `ui/scripts/t3graph.py`) по `/mnt/toshiba/alexandrie/third-party/t3code` @ 803f21f6: замыкание импортов от `apps/web/src/main.tsx`
(`AppRoot`, `router.ts`, `routeTree.gen.ts`, все маршруты) по статическим, динамическим и типовым импортам, через `~/…`, относительные пути
и `@t3tools/{contracts,shared,client-runtime}` (по `exports` их package.json). В файлы скрытых частей (таблица ниже) обход не заходит:
файлы остаются в дереве, а их пакеты заменяются заглушками. Версии — `importers` lock T3 (`apps/web`, затем `packages/shared`,
`packages/client-runtime`, `packages/contracts`); транзитивные — по `snapshots` без peer- и необязательных зависимостей (окно их не ставит: `autoInstallPeers: false`, `ignoredOptionalDependencies: '*'`), в скобках — сколько из них нет в lock окна. Патчи и закрепления —
`patchedDependencies` и `overrides` из `pnpm-workspace.yaml` T3; `--ignore-scripts` (в окне `ignoreScripts: true`).

Файлов T3 к переносу: **1987** (весь `apps/web/src` без тестов и нужные файлы packages), из них в видимом замыкании **1393**,
файлов скрытых частей, до которых доходит видимая часть: 168. Импортов без адресата в T3: 15
(`components/T3AppIcon.tsx` → `../../../../assets/prod/t3-black-web-apple-touch-180.png`; `i18n/locales.ts` → `../../gt.config.json`; `issue-tracker-linear/src/server/LinearApi.graphql.ts` → `../../schema/linear.graphql.ts`; `issue-tracker-linear/src/server/LinearApi.ts` → `../../schema/linear.graphql.ts`; `issue-tracker-linear/src/server/LinearApi.ts` → `../../schema/linear.graphql.ts`; `source-control-github/src/server/gitHubPullRequestJson.ts` → `../../schema/github.graphql.ts`; `source-control-github/src/server/issues.graphql.ts` → `../../schema/github.graphql.ts`; `source-control-github/src/server/pullRequestBatches.graphql.ts` → `../../schema/github.graphql.ts`; `source-control-github/src/server/pullRequestListings.graphql.ts` → `../../schema/github.graphql.ts`; `source-control-github/src/server/pullRequestMutations.graphql.ts` → `../../schema/github.graphql.ts`; `source-control-github/src/server/pullRequestReads.graphql.ts` → `../../schema/github.graphql.ts`; `source-control-github/src/server/pullRequestReviews.graphql.ts` → `../../schema/github.graphql.ts`; `source-control-github/src/server/sourceControlProvider.graphql.ts` → `../../schema/github.graphql.ts`; `source-control-github/src/server/stackActions.graphql.ts` → `../../schema/github.graphql.ts`; `ssh/src/launcherTestHarness.ts` → `../../../`).

### Пакеты к установке

Прямых пакетов: **71**; из них нет в окне или нужна другая версия: **7**. Уникальных пакетов с транзитивными: 248, из них новых для lock окна: **10**.

| Пакет | Версия (lock T3) | В окне сейчас (ui/package.json) | Патч / закрепление T3 | Какой перенесённый файл импортирует | Транзитивных (новых) |
|---|---|---|---|---|---|
| `@base-ui/react` | 1.9.0 | есть | — | components/ui/menu.tsx, components/ui/badge.tsx, components/ui/input.tsx и ещё 33 | 8 (1) |
| `@dnd-kit/core` | 6.3.1 | есть | — | components/Sidebar.tsx, components/Sidebar.drag.ts, components/RightPanelTabs.tsx и ещё 3 | 3 (0) |
| `@dnd-kit/modifiers` | 9.0.0 | есть | — | components/Sidebar.tsx, components/RightPanelTabs.tsx, components/settings/EnvironmentRoutesList.tsx | 2 (0) |
| `@dnd-kit/sortable` | 10.0.0 | есть | — | components/Sidebar.tsx, components/Sidebar.drag.ts, components/Sidebar.logic.ts и ещё 4 | 2 (0) |
| `@dnd-kit/utilities` | 3.2.2 | есть | — | components/RightPanelTabs.tsx, components/Sidebar.pointer.ts, components/sidebar/SidebarThreadRow.tsx и ещё 2 | 1 (0) |
| `@effect/atom-react` | 4.1.2 | есть | закрепление @effect/atom-react → 4.1.2 | remoteOpen.ts, state/query.ts, state/shell.ts и ещё 119 | 0 (0) |
| `@fontsource-variable/inter` | 5.3.0 | есть | — | main.tsx | 0 (0) |
| `@legendapp/list` | 3.7.3 | есть 3.7.2, нужна 3.7.3 | закрепление @legendapp/list → 3.7.3 | components/ChatView.tsx, hooks/useLegendListRef.ts, components/CommandPalette.tsx и ещё 11 | 1 (0) |
| `@noble/hashes` | 1.8.0 | есть | — | shared/src/telemetryId.ts | 0 (0) |
| `@pierre/diffs` | 1.7.0-beta.4 | есть | патч @pierre/diffs@1.7.0-beta.4, mdast-util-to-hast@13.2.1; закрепление @pierre/diffs → 1.7.0-beta.4, mdast-util-to-hast → 13.2.1 | lib/diffRendering.ts, lib/syntaxHighlighting.ts, lib/readableSyntaxTheme.ts и ещё 9 | 51 (1) |
| `@pierre/trees` | 1.0.0-beta.6 | есть | закрепление @pierre/trees>@pierre/theming → 1.0.2 | pierre-icons.ts, components/files/FileBrowserPanel.tsx, components/files/fileTreePathReconciliation.ts | 3 (0) |
| `@tanstack/react-pacer` | 0.19.4 | есть | — | lib/storage.ts, uiStateStore.ts, components/chat/useTimelineFollow.ts | 5 (0) |
| `@tanstack/react-router` | 1.170.10 | есть | — | router.ts, AppRoot.tsx, appInit.tsx и ещё 77 | 9 (0) |
| `@tiptap/core` | 3.31.3 | есть | — | composer-rich-text-doc.ts, composer-text-direction.ts, composer-markdown-source.ts и ещё 3 | 0 (0) |
| `@tiptap/extension-blockquote` | 3.31.3 | есть | — | composer-rich-text-doc.ts | 0 (0) |
| `@tiptap/extension-bold` | 3.31.3 | есть | — | composer-rich-text-doc.ts | 0 (0) |
| `@tiptap/extension-code` | 3.31.3 | есть | — | composer-rich-text-doc.ts | 0 (0) |
| `@tiptap/extension-code-block` | 3.31.3 | есть | — | composer-rich-text-doc.ts | 0 (0) |
| `@tiptap/extension-heading` | 3.31.3 | есть | — | composer-rich-text-doc.ts | 0 (0) |
| `@tiptap/extension-horizontal-rule` | 3.31.3 | есть | — | composer-rich-text-doc.ts | 0 (0) |
| `@tiptap/extension-italic` | 3.31.3 | есть | — | composer-rich-text-doc.ts | 0 (0) |
| `@tiptap/extension-list` | 3.31.3 | есть | — | composer-rich-text-doc.ts | 0 (0) |
| `@tiptap/extension-strike` | 3.31.3 | есть | — | composer-rich-text-doc.ts | 0 (0) |
| `@tiptap/extension-task-item` | 3.31.3 | есть | — | composer-rich-text-doc.ts | 0 (0) |
| `@tiptap/extension-task-list` | 3.31.3 | есть | — | composer-rich-text-doc.ts | 0 (0) |
| `@tiptap/pm` | 3.31.3 | есть | — | composer-code-block.ts, composer-rich-text-doc.ts, composer-undo-grouping.ts и ещё 4 | 16 (0) |
| `@tiptap/react` | 3.31.3 | есть | — | components/ComposerPromptEditorTiptap.tsx, components/chat/ComposerCodeBlockNodeView.tsx | 3 (0) |
| `@tiptap/starter-kit` | 3.31.3 | есть | — | components/ComposerPromptEditorTiptap.tsx | 41 (0) |
| `@types/culori` | 4.0.1 | есть | — | themePalette.ts (только типы) | 0 (0) |
| `@types/hast` | 3.0.5 | есть | — | components/chat/markdownRenderer.ts (только типы) | 1 (0) |
| `@types/mdast` | 4.0.4 | есть | — | shared/src/markdownMath.ts, markdown-incremental.ts, shared/src/markdownPipeline.ts и ещё 6 (только типы) | 1 (0) |
| `@types/react` | 19.3.0 | есть | — | main.tsx, state/query.ts, state/queries.ts и ещё 431 (только типы) | 1 (0) |
| `@types/react-dom` | 19.3.0 | есть | — | main.tsx, lib/observeResize.ts, components/chat/ChatComposer.tsx и ещё 10 (только типы) | 0 (0) |
| `@types/use-sync-external-store` | 0.0.6 | есть | — | hooks/useSettings.ts (только типы) | 0 (0) |
| `class-variance-authority` | 0.7.1 | есть | — | lib/utils.ts, components/ui/alert.tsx, components/ui/badge.tsx и ещё 12 | 1 (0) |
| `culori` | 4.0.2 | есть | — | themePalette.ts | 0 (0) |
| `dompurify` | 3.4.17 | есть | — | components/chat/MermaidDiagram.tsx | 0 (0) |
| `effect` | 4.1.3 | есть | закрепление effect → 4.1.3 | appInit.tsx, lib/utils.ts, state/vcs.ts и ещё 436 | 0 (0) |
| `get-east-asian-width` | 1.6.0 | есть | — | shared/src/terminalOutput.ts | 0 (0) |
| `hast-util-to-html` | 9.0.5 | есть | патч mdast-util-to-hast@13.2.1; закрепление mdast-util-to-hast → 13.2.1 | components/chat/MarkdownCodeBlock.tsx, components/chat/HighlightedCodeLines.tsx | 30 (0) |
| `hast-util-to-jsx-runtime` | 2.3.6 | есть | патч micromark@4.0.2; закрепление micromark → 4.0.2 | components/chat/markdownRenderer.ts, components/chat/HighlightedCodeLines.tsx | 68 (0) |
| `html-url-attributes` | 3.0.1 | есть | — | components/chat/markdownRenderer.ts | 0 (0) |
| `katex` | 0.18.10 | есть | — | components/chat/MathTypeset.tsx, components/chat/MermaidDiagram.tsx | 1 (0) |
| `lucide-react` | 0.564.0 | есть 1.71.0, нужна 0.564.0 | — | routes/__root.tsx, components/Sidebar.tsx, components/ui/menu.tsx и ещё 203 | 0 (0) |
| `mdast-util-directive` | 3.1.0 | есть | патч micromark@4.0.2; закрепление micromark → 4.0.2 | shared/src/codexMarkdownDirectives.ts | 52 (0) |
| `mdast-util-from-markdown` | 2.0.3 | есть | патч micromark@4.0.2; закрепление micromark → 4.0.2 | components/chat/markdownSyntaxGates.ts | 33 (0) |
| `mdast-util-to-hast` | 13.2.1 | есть | патч mdast-util-to-hast@13.2.1; закрепление mdast-util-to-hast → 13.2.1 | shared/src/markdownMath.ts (только типы) | 19 (0) |
| `micromark-extension-directive` | 4.0.0 | есть | — | shared/src/codexMarkdownDirectives.ts | 17 (0) |
| `micromark-util-character` | 2.1.1 | есть | — | shared/src/codexMarkdownDirectives.ts | 2 (0) |
| `micromark-util-combine-extensions` | 2.0.1 | есть | — | components/chat/markdownSyntaxGates.ts | 3 (0) |
| `micromark-util-decode-string` | 2.0.1 | есть | — | shared/src/markdownWindowsPaths.ts | 6 (0) |
| `micromark-util-types` | 2.0.2 | есть | — | shared/src/codexMarkdownDirectives.ts, components/chat/markdownSyntaxGates.ts (только типы) | 0 (0) |
| `pdfjs-dist` | 6.4.299 | есть 6.5.438, нужна 6.4.299 | — | components/files/PdfViewer.tsx, components/files/pdfDocument.ts | 0 (0) |
| `pele` | 1.8.0 | есть | — | components/chat/MermaidDiagram.tsx | 0 (0) |
| `property-information` | 7.2.0 | есть | — | shared/src/markdownPipeline.ts | 0 (0) |
| `react` | 19.3.0 | есть 19.3.1, нужна 19.3.0 | — | main.tsx, state/query.ts, state/queries.ts и ещё 431 | 0 (0) |
| `react-dom` | 19.3.0 | есть 19.3.1, нужна 19.3.0 | — | main.tsx, lib/observeResize.ts, components/chat/ChatComposer.tsx и ещё 10 | 1 (0) |
| `react-markdown` | 10.1.0 | есть | патч mdast-util-to-hast@13.2.1, micromark@4.0.2; закрепление mdast-util-to-hast → 13.2.1, micromark → 4.0.2 | components/ChatMarkdown.tsx, components/chat/MarkdownCodeBlock.logic.ts | 81 (0) |
| `rehype-raw` | 7.0.0 | есть | патч mdast-util-to-hast@13.2.1; закрепление mdast-util-to-hast → 13.2.1 | shared/src/markdownPipeline.ts | 34 (0) |
| `rehype-sanitize` | 6.0.0 | есть | — | shared/src/markdownPipeline.ts | 5 (0) |
| `remark-breaks` | 4.0.0 | есть | — | shared/src/markdownPipeline.ts | 17 (0) |
| `remark-gfm` | 4.0.1 | есть | патч micromark-extension-gfm-table@2.1.1, micromark@4.0.2; закрепление micromark → 4.0.2, micromark-extension-gfm-table → 2.1.1 | shared/src/markdownMath.ts, shared/src/markdownPipeline.ts, shared/src/markdownTaskList.ts и ещё 1 | 67 (0) |
| `remark-math` | 6.0.0 | есть | патч mdast-util-math@3.0.0, micromark@4.0.2; закрепление mdast-util-math → 3.0.0, micromark → 4.0.2, remark-math>micromark-extension-math → npm:micromark-extension-llm-math@3.1.0 | shared/src/markdownMath.ts, components/ChatMarkdown.tsx | 52 (1) |
| `remark-parse` | 11.0.0 | есть | патч micromark@4.0.2; закрепление micromark → 4.0.2 | shared/src/markdownMath.ts, shared/src/threadFindText.ts, shared/src/markdownPipeline.ts и ещё 4 | 41 (0) |
| `remark-rehype` | 11.1.2 | есть | патч mdast-util-to-hast@13.2.1; закрепление mdast-util-to-hast → 13.2.1 | shared/src/threadFindText.ts, shared/src/markdownPipeline.ts, components/chat/markdownRenderer.ts | 25 (0) |
| `tailwind-merge` | 3.6.0 | есть 3.7.0, нужна 3.6.0 | — | lib/utils.ts | 0 (0) |
| `unbash` | 5.0.0 | есть | — | client-runtime/src/work-log/embeddedScripts.ts | 0 (0) |
| `unified` | 11.0.5 | есть | — | shared/src/markdownMath.ts, markdown-incremental.ts, shared/src/threadFindText.ts и ещё 11 | 10 (0) |
| `use-sync-external-store` | 1.6.0 | есть | — | hooks/useSettings.ts | 0 (0) |
| `vfile` | 6.0.3 | есть | — | components/chat/markdownRenderer.ts | 3 (0) |
| `zustand` | 5.0.14 | есть 5.1.0, нужна 5.0.14 | — | uiStateStore.ts, session-logic.ts, closedViewStore.ts и ещё 21 | 0 (0) |

Для сборки (не импорты кода): `vite`, `@vitejs/plugin-react` (T3 собирает с `react({ compiler: true })`, React Compiler), `oxc-transform-react`,
`tailwindcss` и `@tailwindcss/vite` — уже есть в окне. `@tanstack/router-plugin` не нужен: `routeTree.gen.ts` лежит в дереве T3 готовым.


### Заглушено вместо установки

Пакеты, которые импортируют только скрытые части, и пакеты T3, которые не берутся (план: провайдеры, source-control, секреты, трекеры).
Заглушка — наш файл через `ui/src/t3/base/aliases.json` и `tsconfig.paths.json` (как `gt-react` у прежнего переноса). Где есть видимые
импортёры, заглушка должна быть рабочей (отвечать «части нет»), а не пустой.

| Пакет | Версия (lock T3) | Скрытая часть | Кто импортирует | Видимые импортёры |
|---|---|---|---|---|
| `@betteroffice/docx` | 0.5.1 | офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) | components/files/office/fonts.ts, components/files/office/word-text.ts, components/files/office/word-glyphs.ts и ещё 2 | — |
| `@betteroffice/fonts` | 0.4.0 | офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) | components/files/office/fonts.ts, components/files/office/slide-fonts.ts | — |
| `@betteroffice/pptx` | 0.5.1 | офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) | components/files/office/slide-text.ts, components/files/office/slide-fonts.ts, components/files/office/presentation.worker.ts | — |
| `@betteroffice/xlsx` | 0.5.1 | офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) | components/files/office/sheet-cells.ts, components/files/office/sheet-fonts.ts, components/files/office/sheet-frame.ts и ещё 4 | — |
| `@clerk/electron` | 0.1.10 | облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh | components/clerk/ElectronManagedAuthShell.tsx | — |
| `@clerk/react` | 6.18.6 | MCP Apps; облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh | cloud/managedAuth.tsx, cloud/useCloudLinkController.ts, components/HostedEmptyState.tsx и ещё 12 | components/onboarding/WelcomePage.tsx |
| `@daypicker/react` | 10.0.1 | запланированные задачи и отложенные разговоры (snooze) | components/ui/calendar.tsx | — |
| `@noble/curves` | 1.9.1 | облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh | shared/src/dpop.ts | — |
| `canvas-confetti` | 1.9.4 | GitHub-звезда и конфетти | components/GitHubStarPrompt.tsx | — |
| `entities` | 8.1.0 | офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) | components/files/extendedDocumentPreview.ts | — |
| `fast-xml-parser` | 5.7.3 | офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) | components/files/extendedDocumentPreview.ts | — |
| `gt-i18n` | 1.0.40 | i18n T3 (General Translation): настоящий не ставим (решение CEO) — прослойка окна shims/gt-i18n.ts, без сети, английские строки T3 как есть; облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh | shared/src/speechCopy.ts, shared/src/promptLinksCopy.ts, shared/src/threadLinksCopy.ts и ещё 99 | shared/src/promptLinksCopy.ts, shared/src/threadLinksCopy.ts, shared/src/usageLimitsCopy.ts и ещё 83 |
| `gt-react` | 11.4.18 | GitHub-звезда и конфетти; MCP Apps; git и source control, запросы на слияние, задачи и трекеры, review, worktree, клон проекта; i18n T3 (General Translation): настоящий не ставим (решение CEO) — прослойка окна shims/gt-react.tsx, без сети, английские строки T3 как есть; вложения (загрузка) и снимки экрана (snap-shot); голосовой ввод; запланированные задачи и отложенные разговоры (snooze); импорт и экспорт тем VS Code / Open VSX; использование, телеметрия, диагностика, ресурсы; облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh; офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic); панель изменений (diff); превью и встроенный браузер; секреты и 1Password; терминал | localApi.ts, lib/utils.ts, cloud/dpop.ts и ещё 703 | localApi.ts, lib/utils.ts, threadSync.ts и ещё 444 |
| `heic-to` | 1.5.2 | вложения (загрузка) и снимки экрана (snap-shot); офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) | lib/imageCompression.ts, components/files/imageDecode.worker.ts | — |
| `jsonc-parser` | 3.3.1 | импорт и экспорт тем VS Code / Open VSX | openVsxThemes.ts, vscodeThemeImport.ts | — |
| `jszip` | 3.10.1 | импорт и экспорт тем VS Code / Open VSX; офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) | openVsxThemes.ts, client-runtime/src/zip/boundedZip.ts, components/files/officeArchive.ts и ещё 1 | — |
| `saxes` | 6.0.0 | офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) | components/files/officeArchive.ts | — |
| `sql.js` | 1.15.0 | офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) | components/files/binaryPreview.worker.ts | — |
| `three` | 0.180.0 | облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh | client-runtime/src/device/duoSnap.ts, client-runtime/src/device/duoScene.ts, client-runtime/src/device/duoViewer.ts и ещё 7 | — |
| `tiff` | 7.1.3 | офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) | components/files/imageDecode.worker.ts | — |
| `vite-plus` | 1.4.0 | сборка T3 (Vite+): окно собирает обычным vite, типы `vite-plus/client` дают `vite/client` | vite-env.d.ts | vite-env.d.ts |

Всего заглушено: **21** (21 npm-пакетов и 0 пакетов T3).

### Скрытые части (обход в них не заходит)

| Часть | Файлы T3 (шаблоны путей) |
|---|---|
| git и source control, запросы на слияние, задачи и трекеры, review, worktree, клон проекта | `components/(GitActionsControl|BranchToolbar|BranchPicker|PullRequestThreadDialog|ProjectCloneDialog|CloneProject|Worktree|SourceControl|ReviewGuide|Review)[^/]*$`, `components/(pullRequest|issue)/`, `routes/_chat\.(pull-requests|issues)\.tsx$`, `(sourceControlPresentation|worktreeCleanup|reviewCommentContext)\.`, `lib/open(PullRequest|Issue)Link\.`, `hooks/use(PullRequest|Issue)[^/]*$` |
| терминал | `terminal/`, `components/terminal/`, `components/[^/]*Terminal[^/]*$`, `terminal[A-Z][^/]*$` |
| превью и встроенный браузер | `browser/`, `components/preview/`, `(preview[A-Z]|browser[A-Z]|portDiscovery)[^/]*$` |
| облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh | `cloud/`, `components/(clerk|cloud|device|guest|auth)/`, `(guestPage|hostedPairing)\.`, `components/(Hosted|ConnectDeviceApproval)[^/]*$`, `components/chat/Guest[^/]*$`, `components/desktop/SshPasswordPrompt[^/]*$`, `routes/(connect|connect-agent|approve-agent|pair|guest)\.tsx$`, `client-runtime/src/device/` |
| запланированные задачи и отложенные разговоры (snooze) | `components/[^/]*Scheduled[^/]*$`, `components/settings/[^/]*Scheduled[^/]*$`, `components/(CustomSnoozeDialog|Sidebar\.snooze)[^/]*$`, `components/settings/SnoozePresets[^/]*$` |
| MCP Apps | `components/mcp/`, `components/chat/McpApp[^/]*$` |
| панель изменений (diff) | `components/(DiffPanel|DiffWorkerPoolProvider)[^/]*$`, `components/diffs/`, `(diffPanelStore|diffFileActions)\.` |
| офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) | `components/files/office/`, `components/files/(officeArchive|extendedDocumentPreview|binaryPreview\.worker|imageDecode\.worker)\.`, `components/files/(BinaryViewer|BinaryFilePreview|binaryPreviewProtocol|readBinaryPreview)[^/]*$` |
| вложения (загрузка) и снимки экрана (snap-shot) | `lib/imageCompression\.`, `components/files/AttachmentFilePreview[^/]*$`, `components/chat/(ComposerAttachmentStrip|SnapShot)[^/]*$`, `components/desktop/SnapShot[^/]*$` |
| использование, телеметрия, диагностика, ресурсы | `components/usage/`, `routes/_chat\.usage\.tsx$`, `observability/` |
| импорт и экспорт тем VS Code / Open VSX | `(openVsxThemes|vscodeThemeImport)\.` |
| секреты и 1Password | `components/settings/Secret[^/]*$` |
| GitHub-звезда и конфетти | `components/GitHubStar[^/]*$` |
| голосовой ввод | `speech/`, `components/settings/Microphone[^/]*$` |
| разделы настроек, кроме общих, вида, чата, провайдеров и доступа | `routes/_chat\.settings\.(about|archived|beta|browser|connections|devices|diagnostics|diffs|integrations|keybindings|labs|maintenance|open-source-licenses|projects|scheduled-tasks|secrets|snap-shot|source-control|storage|threads|voice|worktrees)\.tsx$` |

Новые пакеты целиком (10): `@babel/runtime@7.29.7`, `@legendapp/list@3.7.3`, `@pierre/highlights@0.5.1`, `lucide-react@0.564.0`, `micromark-extension-math@micromark-extension-llm-math@3.1.0`, `pdfjs-dist@6.4.299`, `react-dom@19.3.0`, `react@19.3.0`, `tailwind-merge@3.6.0`, `zustand@5.0.14`

### Типы, которых нет у окна

`@types/trusted-types` 2.0.7 — у T3 необязательная зависимость `dompurify` (его d.ts импортирует `trusted-types/lib/index.js`;
без них `DOMPurify(window)` в `components/chat/MermaidDiagram.tsx` не проходит tsc). Пока не поставлен — объявления
`src/t3/base/types/trusted-types/lib/index.d.ts` (typePaths манифеста).
