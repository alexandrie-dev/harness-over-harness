# Карта кода T3 Code в окне HoH

Источник: T3 Code, MIT License, Copyright (c) 2026 T3 Tools Inc. (`/mnt/toshiba/alexandrie/third-party/t3code`, коммит 803f21f6, только чтение). Полный текст лицензии — `ui/NOTICE.md`. Названий и знаков T3 в окне нет: перенесены только файлы разметки ответа, в сборку попадают только подключённые.

Файлы переносит `ui/scripts/t3-sync.py` по списку `ui/src/t3/manifest.json` (копия без правок; расхождений с источником сейчас: 0). Дерево повторяет T3: `apps/web/src` → `ui/src/t3/web`, `packages/shared/src` → `ui/src/t3/shared`, `packages/client-runtime/src` → `ui/src/t3/client-runtime`. Псевдонимы пакетов (`@t3tools/*`, `gt-react`, `effect/*`) прописывает тот же скрипт в `ui/src/t3/tsconfig.paths.json` (tsc) и `ui/src/t3/aliases.json` (vite). Карту обновляет `ui/scripts/t3-map.py`; целостность импортов проверяет `ui/scripts/t3-check.py`.

## Состояние

- **В сборке сейчас:** рендер ответов на конвейере T3 (marked удалён 20.11.2026) через переключатель `ui/src/t3/render.tsx`. Файлы T3 из первой таблицы проверены компилятором (`tsc --noEmit`), но чат их пока не вызывает.
- **Отложено:** вторая таблица — файлы T3, которым нужны пакеты, которых нет в lock окна. До установки tsc их пропускает (`deferredEnabled: false`), vite их не видит, Tailwind не сканирует (`@source not "../t3"` в tokens.css). Компилятором они не проверены: проверены только адресаты импортов (`t3-check.py`).
- **Основа HOH-40** (`src/t3/base`, раздел «Основа HOH-40» ниже): весь веб-клиент T3 перенесён файлами для этапов 3–5 плана `plans/hoh40-t3-base.md`. В сборку окна пока не входит: старая оболочка работает как раньше.
- **Не перенесено из запрошенного:** шаги инструментов (`TimelineWorkEntryRow` тянет за собой подтверждения и поле ввода T3; нынешние строки шагов окна остаются) и картинки-вложения (`ComposerImageThumbnail`, `GuestMessageAttachments`; в протоколе окна пока нет вложений). Файлов T3 для них в дереве нет.

## Перенесено без изменений и проверено tsc

| Путь в T3 | Путь у нас | Изменения |
|---|---|---|
| `packages/shared/src/markdownFrontmatter.ts` | `ui/src/t3/shared/markdownFrontmatter.ts` | без изменений |
| `packages/shared/src/textDirection.ts` | `ui/src/t3/shared/textDirection.ts` | без изменений |
| `packages/shared/src/mediaPreview.ts` | `ui/src/t3/shared/mediaPreview.ts` | без изменений |
| `packages/shared/src/markdownLinks.ts` | `ui/src/t3/shared/markdownLinks.ts` | без изменений |
| `packages/shared/src/fileLinks.ts` | `ui/src/t3/shared/fileLinks.ts` | без изменений |
| `packages/shared/src/path.ts` | `ui/src/t3/shared/path.ts` | без изменений |
| `packages/shared/src/image.ts` | `ui/src/t3/shared/image.ts` | без изменений |
| `packages/shared/src/video.ts` | `ui/src/t3/shared/video.ts` | без изменений |
| `packages/shared/src/markdownPipeline.ts` | `ui/src/t3/shared/markdownPipeline.ts` | без изменений |
| `packages/shared/src/markdownTaskList.ts` | `ui/src/t3/shared/markdownTaskList.ts` | без изменений |
| `packages/shared/src/markdownWindowsPaths.ts` | `ui/src/t3/shared/markdownWindowsPaths.ts` | без изменений |
| `packages/shared/src/markdownGithubAlerts.ts` | `ui/src/t3/shared/markdownGithubAlerts.ts` | без изменений |
| `packages/shared/src/markdownLeadingWhitespace.ts` | `ui/src/t3/shared/markdownLeadingWhitespace.ts` | без изменений |
| `packages/shared/src/markdownListIndentation.ts` | `ui/src/t3/shared/markdownListIndentation.ts` | без изменений |
| `packages/shared/src/markdownOrderedListNumbers.ts` | `ui/src/t3/shared/markdownOrderedListNumbers.ts` | без изменений |
| `packages/shared/src/codexMarkdownDirectives.ts` | `ui/src/t3/shared/codexMarkdownDirectives.ts` | без изменений |
| `packages/shared/src/codexArtifactTemplates.ts` | `ui/src/t3/shared/codexArtifactTemplates.ts` | без изменений |
| `packages/shared/src/codexFileCitations.ts` | `ui/src/t3/shared/codexFileCitations.ts` | без изменений |
| `packages/client-runtime/src/markdownImages.ts` | `ui/src/t3/client-runtime/markdownImages.ts` | без изменений |
| `packages/client-runtime/src/mediaReference.ts` | `ui/src/t3/client-runtime/mediaReference.ts` | без изменений |
| `packages/client-runtime/src/work-log/commandLabel.ts` | `ui/src/t3/client-runtime/work-log/commandLabel.ts` | без изменений |
| `apps/web/src/components/RenderErrorBoundary.tsx` | `ui/src/t3/web/components/RenderErrorBoundary.tsx` | без изменений |
| `apps/web/src/components/chat/MathTypeset.logic.ts` | `ui/src/t3/web/components/chat/MathTypeset.logic.ts` | без изменений |
| `apps/web/src/components/chat/hostImageUrl.ts` | `ui/src/t3/web/components/chat/hostImageUrl.ts` | без изменений |
| `apps/web/src/components/chat/markdownActionFailure.ts` | `ui/src/t3/web/components/chat/markdownActionFailure.ts` | без изменений |
| `apps/web/src/components/chat/markdownChipPunctuation.ts` | `ui/src/t3/web/components/chat/markdownChipPunctuation.ts` | без изменений |
| `apps/web/src/components/chat/markdownFindContext.ts` | `ui/src/t3/web/components/chat/markdownFindContext.ts` | без изменений |
| `apps/web/src/components/chat/markdownFragments.ts` | `ui/src/t3/web/components/chat/markdownFragments.ts` | без изменений |
| `apps/web/src/components/chat/markdownHast.logic.ts` | `ui/src/t3/web/components/chat/markdownHast.logic.ts` | без изменений |
| `apps/web/src/components/chat/markdownStreamBlocks.ts` | `ui/src/t3/web/components/chat/markdownStreamBlocks.ts` | без изменений |
| `apps/web/src/components/chat/markdownTextDirection.ts` | `ui/src/t3/web/components/chat/markdownTextDirection.ts` | без изменений |
| `apps/web/src/components/ui/button.tsx` | `ui/src/t3/web/components/ui/button.tsx` | без изменений |
| `apps/web/src/components/ui/collapsible.tsx` | `ui/src/t3/web/components/ui/collapsible.tsx` | без изменений |
| `apps/web/src/components/ui/scroll-area.tsx` | `ui/src/t3/web/components/ui/scroll-area.tsx` | без изменений |
| `apps/web/src/components/ui/tooltip.tsx` | `ui/src/t3/web/components/ui/tooltip.tsx` | без изменений |
| `apps/web/src/lib/lruCache.ts` | `ui/src/t3/web/lib/lruCache.ts` | без изменений |
| `apps/web/src/lib/observeResize.ts` | `ui/src/t3/web/lib/observeResize.ts` | без изменений |
| `apps/web/src/textDirection.ts` | `ui/src/t3/web/textDirection.ts` | без изменений |
| `apps/web/src/lib/windowControlsOverlay.ts` | `ui/src/t3/web/lib/windowControlsOverlay.ts` | без изменений |
| `apps/web/src/components/media/mediaContent.ts` | `ui/src/t3/web/components/media/mediaContent.ts` | без изменений |
| `apps/web/src/components/chat/externalLinkContextMenu.ts` | `ui/src/t3/web/components/chat/externalLinkContextMenu.ts` | без изменений |
| `apps/web/src/components/chat/markdownImageGallery.ts` | `ui/src/t3/web/components/chat/markdownImageGallery.ts` | без изменений |
| `apps/web/src/components/chat/ChatMarkdownMedia.tsx` | `ui/src/t3/web/components/chat/ChatMarkdownMedia.tsx` | без изменений |
| `apps/web/src/components/ui/closing-positioner.tsx` | `ui/src/t3/web/components/ui/closing-positioner.tsx` | без изменений |
| `apps/web/src/components/ui/input.tsx` | `ui/src/t3/web/components/ui/input.tsx` | без изменений |
| `apps/web/src/components/ui/spinner.tsx` | `ui/src/t3/web/components/ui/spinner.tsx` | без изменений |
| `apps/web/src/lib/visibleAnimation.ts` | `ui/src/t3/web/lib/visibleAnimation.ts` | без изменений |
| `apps/web/src/filePathDisplay.ts` | `ui/src/t3/web/filePathDisplay.ts` | без изменений |
| `apps/web/src/terminal-links.ts` | `ui/src/t3/web/terminal-links.ts` | без изменений |
| `apps/web/src/markdown-links.ts` | `ui/src/t3/web/markdown-links.ts` | без изменений |
| `apps/web/src/markdownFileHostPath.ts` | `ui/src/t3/web/markdownFileHostPath.ts` | без изменений |
| `apps/web/src/components/chat/runtimeModeConfig.ts` | `ui/src/t3/web/components/chat/runtimeModeConfig.ts` | без изменений |

## Перенесено без изменений, отложено (нужны пакеты)

| Путь в T3 | Путь у нас | Изменения |
|---|---|---|
| `packages/shared/src/markdownMath.ts` | `ui/src/t3/shared/markdownMath.ts` | без изменений |
| `packages/client-runtime/src/assemblyHighlighting.ts` | `ui/src/t3/client-runtime/assemblyHighlighting.ts` | без изменений |
| `apps/web/src/components/ChatMarkdown.tsx` | `ui/src/t3/web/components/ChatMarkdown.tsx` | без изменений |
| `apps/web/src/components/chat/MarkdownCodeBlock.tsx` | `ui/src/t3/web/components/chat/MarkdownCodeBlock.tsx` | без изменений |
| `apps/web/src/components/chat/MarkdownCodeBlock.logic.ts` | `ui/src/t3/web/components/chat/MarkdownCodeBlock.logic.ts` | без изменений |
| `apps/web/src/components/chat/MarkdownDetails.tsx` | `ui/src/t3/web/components/chat/MarkdownDetails.tsx` | без изменений |
| `apps/web/src/components/chat/MarkdownMath.tsx` | `ui/src/t3/web/components/chat/MarkdownMath.tsx` | без изменений |
| `apps/web/src/components/chat/MarkdownMermaidCodeBlock.tsx` | `ui/src/t3/web/components/chat/MarkdownMermaidCodeBlock.tsx` | без изменений |
| `apps/web/src/components/chat/MarkdownTable.tsx` | `ui/src/t3/web/components/chat/MarkdownTable.tsx` | без изменений |
| `apps/web/src/components/chat/MathTypeset.tsx` | `ui/src/t3/web/components/chat/MathTypeset.tsx` | без изменений |
| `apps/web/src/components/chat/MermaidDiagram.tsx` | `ui/src/t3/web/components/chat/MermaidDiagram.tsx` | без изменений |
| `apps/web/src/components/chat/markdownRenderer.ts` | `ui/src/t3/web/components/chat/markdownRenderer.ts` | без изменений |
| `apps/web/src/components/chat/markdownSyntaxGates.ts` | `ui/src/t3/web/components/chat/markdownSyntaxGates.ts` | без изменений |
| `apps/web/src/components/chat/HighlightedCodeLines.tsx` | `ui/src/t3/web/components/chat/HighlightedCodeLines.tsx` | без изменений |
| `apps/web/src/markdown-clipboard.ts` | `ui/src/t3/web/markdown-clipboard.ts` | без изменений |
| `apps/web/src/markdown-incremental.ts` | `ui/src/t3/web/markdown-incremental.ts` | без изменений |
| `apps/web/src/lib/syntaxHighlighting.ts` | `ui/src/t3/web/lib/syntaxHighlighting.ts` | без изменений |
| `apps/web/src/lib/incrementalHighlighting.ts` | `ui/src/t3/web/lib/incrementalHighlighting.ts` | без изменений |
| `apps/web/src/lib/diffRendering.ts` | `ui/src/t3/web/lib/diffRendering.ts` | без изменений |
| `apps/web/src/lib/readableSyntaxTheme.ts` | `ui/src/t3/web/lib/readableSyntaxTheme.ts` | без изменений |
| `apps/web/src/lib/contextChipAccents.ts` | `ui/src/t3/web/lib/contextChipAccents.ts` | без изменений |
| `apps/web/src/components/ui/menu.tsx` | `ui/src/t3/web/components/ui/menu.tsx` | без изменений |

## Прослойки (наши файлы вместо зависимостей T3)

Это не код T3, а тонкие замены того, чего у окна нет: состояния и хоста T3, Effect, gt-react (i18n), contracts. Они лежат по тем же путям, что и заменённые файлы, поэтому перенесённый код читает их без правок.

| Наш файл | Заменяет в T3 | Что делает |
|---|---|---|
| `ui/src/t3/web/browserHistoryStore.ts` | `apps/web/src/browserHistoryStore.ts` | истории встроенного браузера нет. |
| `ui/src/t3/web/contextMenuSource.ts` | `apps/web/src/contextMenuSource.ts` |  |
| `ui/src/t3/web/editorLabels.ts` | `apps/web/src/editorLabels.ts` | внешних редакторов нет. |
| `ui/src/t3/web/editorPreferences.ts` | `apps/web/src/editorPreferences.ts` | внешних редакторов нет. |
| `ui/src/t3/web/env.ts` | `apps/web/src/env.ts` | окно не Electron и не использует его мост. |
| `ui/src/t3/web/localApi.ts` | `apps/web/src/localApi.ts` | моста настольного приложения T3 нет; системные действия окно просит у ядра Tauri отдельно. |
| `ui/src/t3/web/pierre-icons.ts` | `apps/web/src/pierre-icons.ts` | значков файлов Pierre нет. |
| `ui/src/t3/web/remoteOpen.ts` | `apps/web/src/remoteOpen.ts` | удалённого открытия файлов нет. |
| `ui/src/t3/web/rightPanelStore.ts` | `apps/web/src/rightPanelStore.ts` | у окна нет правой панели файлов, |
| `ui/src/t3/web/state/assets.ts` | `apps/web/src/state/assets.ts` | окружений хоста T3 в окне нет; любое обращение возвращает пустое значение. |
| `ui/src/t3/web/state/clientHostEnvironment.ts` | `apps/web/src/state/clientHostEnvironment.ts` | окно всегда на машине пользователя. |
| `ui/src/t3/web/state/entities.ts` | `apps/web/src/state/entities.ts` | проектов и потоков T3 нет. |
| `ui/src/t3/web/state/filesystem.ts` | `apps/web/src/state/filesystem.ts` | окружений хоста T3 в окне нет; любое обращение возвращает пустое значение. |
| `ui/src/t3/web/state/issueTrackers.ts` | `apps/web/src/state/issueTrackers.ts` | окружений хоста T3 в окне нет; любое обращение возвращает пустое значение. |
| `ui/src/t3/web/state/preview.ts` | `apps/web/src/state/preview.ts` | окружений хоста T3 в окне нет; любое обращение возвращает пустое значение. |
| `ui/src/t3/web/state/primaryEnvironment.ts` | `apps/web/src/state/primaryEnvironment.ts` | основного окружения T3 нет. |
| `ui/src/t3/web/state/projects.ts` | `apps/web/src/state/projects.ts` | окружений хоста T3 в окне нет; любое обращение возвращает пустое значение. |
| `ui/src/t3/web/state/query.ts` | `apps/web/src/state/query.ts` | запросов к хосту T3 нет. |
| `ui/src/t3/web/state/server.ts` | `apps/web/src/state/server.ts` | окружений хоста T3 в окне нет; любое обращение возвращает пустое значение. |
| `ui/src/t3/web/state/session.ts` | `apps/web/src/state/session.ts` | сессий подключения T3 нет; прав на операции хоста нет. |
| `ui/src/t3/web/state/shell.ts` | `apps/web/src/state/shell.ts` | окружений хоста T3 в окне нет; любое обращение возвращает пустое значение. |
| `ui/src/t3/web/state/use-atom-command.ts` | `apps/web/src/state/use-atom-command.ts` | команд хоста T3 нет: команда возвращает отказ. |
| `ui/src/t3/web/state/use-atom-query-runner.ts` | `apps/web/src/state/use-atom-query-runner.ts` | запросов хоста T3 нет. |
| `ui/src/t3/web/components/media/MediaActions.tsx` | `apps/web/src/components/media/MediaActions.tsx` |  |
| `ui/src/t3/web/components/media/MediaVideoPlayer.tsx` | `apps/web/src/components/media/MediaVideoPlayer.tsx` | обычный <video>. |
| `ui/src/t3/web/components/pullRequest/PullRequestLinkPreview.tsx` | `apps/web/src/components/pullRequest/PullRequestLinkPreview.tsx` | запросов на слияние нет. |
| `ui/src/t3/web/components/files/InlineAudioPreview.tsx` | `apps/web/src/components/files/InlineAudioPreview.tsx` | обычный <audio>. |
| `ui/src/t3/web/components/issue/IssueLinkPreview.tsx` | `apps/web/src/components/issue/IssueLinkPreview.tsx` | трекеров задач нет. |
| `ui/src/t3/web/components/chat/AssistantCitationChip.tsx` | `apps/web/src/components/chat/AssistantCitationChip.tsx` | цитат T3 нет. |
| `ui/src/t3/web/components/chat/ExpandedImageDialog.tsx` | `apps/web/src/components/chat/ExpandedImageDialog.tsx` | разворот изображения окну пока не нужен. |
| `ui/src/t3/web/components/chat/ExpandedImagePreview.ts` | `apps/web/src/components/chat/ExpandedImagePreview.ts` |  |
| `ui/src/t3/web/components/chat/MarkdownExternalLink.tsx` | `apps/web/src/components/chat/MarkdownExternalLink.tsx` | значок сайта берёт T3 у системы контроля версий; здесь — просто текст ссылки. |
| `ui/src/t3/web/components/chat/MarkdownFileLink.logic.ts` | `apps/web/src/components/chat/MarkdownFileLink.logic.ts` |  |
| `ui/src/t3/web/components/chat/MarkdownFileLink.tsx` | `apps/web/src/components/chat/MarkdownFileLink.tsx` | чип файла T3 опирается на хост (редактор, проводник, значки Pierre); здесь файл показывается кодом. |
| `ui/src/t3/web/components/chat/MarkdownThreadLink.tsx` | `apps/web/src/components/chat/MarkdownThreadLink.tsx` | ссылок на потоки нет. |
| `ui/src/t3/web/components/chat/PierreEntryIcon.tsx` | `apps/web/src/components/chat/PierreEntryIcon.tsx` | значков файлов Pierre нет. |
| `ui/src/t3/web/components/chat/SkillInlineText.tsx` | `apps/web/src/components/chat/SkillInlineText.tsx` | навыков T3 нет. |
| `ui/src/t3/web/components/ui/toast.tsx` | `apps/web/src/components/ui/toast.tsx` | всплывающих сообщений T3 нет; ошибки показывает вызывающий код в разметке. |
| `ui/src/t3/web/components/preview/fileExplorerLabel.ts` | `apps/web/src/components/preview/fileExplorerLabel.ts` |  |
| `ui/src/t3/web/rpc/atomRegistry.ts` | `apps/web/src/rpc/atomRegistry.ts` | реестра атомов нет. |
| `ui/src/t3/web/i18n/messages.ts` | `apps/web/src/i18n/messages.ts` | русский текст из словаря. |
| `ui/src/t3/web/assets/assetUrls.ts` | `apps/web/src/assets/assetUrls.ts` | ресурсов сервера T3 у окна нет, |
| `ui/src/t3/web/hooks/useCopyToClipboard.ts` | `apps/web/src/hooks/useCopyToClipboard.ts` | копирование через navigator.clipboard без сообщений T3. |
| `ui/src/t3/web/hooks/usePullRequestLinking.ts` | `apps/web/src/hooks/usePullRequestLinking.ts` | запросов на слияние нет. |
| `ui/src/t3/web/hooks/useSettings.ts` | `apps/web/src/hooks/useSettings.ts` | настройки вида окна HoH: перенос строк в коде выключен, формулы включены, ссылки открывает система. |
| `ui/src/t3/web/hooks/useTheme.ts` | `apps/web/src/hooks/useTheme.ts` | у окна одна тёмная тема. |
| `ui/src/t3/web/browser/browserLinkTarget.ts` | `apps/web/src/browser/browserLinkTarget.ts` | встроенного браузера нет. |
| `ui/src/t3/web/browser/openFileInPreview.ts` | `apps/web/src/browser/openFileInPreview.ts` | встроенного просмотра нет. |
| `ui/src/t3/web/browser/previewRuntime.ts` | `apps/web/src/browser/previewRuntime.ts` | встроенного просмотра нет. |
| `ui/src/t3/web/browser/useOpenLink.ts` | `apps/web/src/browser/useOpenLink.ts` | ссылки открывает система. |
| `ui/src/t3/web/lib/openIssueLink.ts` | `apps/web/src/lib/openIssueLink.ts` | трекеров задач нет. |
| `ui/src/t3/web/lib/openPullRequestLink.ts` | `apps/web/src/lib/openPullRequestLink.ts` | запросов на слияние нет. |
| `ui/src/t3/web/lib/utils.ts` | `apps/web/src/lib/utils.ts` | оставлены только функции, которые нужны перенесённому коду. |
| `ui/src/t3/shared/assistantCitations.ts` | `packages/shared/src/assistantCitations.ts` | цитат-ссылок T3 в ответах Помощника нет. |
| `ui/src/t3/shared/codexArtifactTemplatesCopy.ts` | `packages/shared/src/codexArtifactTemplatesCopy.ts` | шаблоны артефактов Codex в окне не показываются. |
| `ui/src/t3/shared/composerContextClipboard.ts` | `packages/shared/src/composerContextClipboard.ts` | копирование с контекстом поля ввода T3 не нужно; копируется обычный текст. |
| `ui/src/t3/shared/composerContextReferences.ts` | `packages/shared/src/composerContextReferences.ts` | вставок контекста из поля ввода T3 в окне нет. |
| `ui/src/t3/shared/threadLinks.ts` | `packages/shared/src/threadLinks.ts` | в окне нет связанных потоков, ссылки на них не разбираются. |
| `ui/src/t3/client-runtime/changeRequestLinks.ts` | `packages/client-runtime/src/changeRequestLinks.ts` | запросов на слияние в окне нет. |
| `ui/src/t3/client-runtime/environment.ts` | `packages/client-runtime/src/environment/index.ts` |  |
| `ui/src/t3/client-runtime/errors.ts` | `packages/client-runtime/src/errors/index.ts` |  |
| `ui/src/t3/client-runtime/issueLinks.ts` | `packages/client-runtime/src/issueLinks.ts` | трекеров задач в окне нет. |
| `ui/src/t3/client-runtime/sourceControlClients.ts` | `packages/client-runtime/src/sourceControlClients.ts` | систем контроля версий в окне нет. |
| `ui/src/t3/client-runtime/workspaceBasenameLookup.ts` | `packages/client-runtime/src/workspaceBasenameLookup.ts` | поиска файлов по имени в рабочей папке через хост T3 нет. |
| `ui/src/t3/client-runtime/state/runtime.ts` | `packages/client-runtime/src/state/runtime.ts` | команд хоста T3 (атомы) в окне нет. |
| `ui/src/t3/shims/atom-react.ts` | `@effect/atom-react` | атомов состояния T3 у окна нет. |
| `ui/src/t3/shims/contract-labels.ts` | `@t3tools/client-runtime/contract-labels` | ограничения «организации» (managed policy) в окне нет, |
| `ui/src/t3/shims/contracts.ts` | `@t3tools/contracts` | только те типы и значения, |
| `ui/src/t3/shims/global.d.ts` | — | Мост Electron из T3 Code в окне HoH отсутствует; тип нужен только перенесённому коду. |
| `ui/src/t3/shims/gt-react.tsx` | `gt-react` | тексты T3 написаны по-английски, |
| `ui/src/t3/shims/ru.ts` | — | Русские тексты для перенесённого кода T3 (английская строка -> русская). Ключ — точная строка из кода T3. |
| `ui/src/t3/shims/stub.ts` | — | Заглушка для объектов хоста T3, которых в окне HoH нет: любое обращение возвращает функцию, дающую undefined. |
| `ui/src/t3/shims/effect/Cause.ts` | `effect/Cause` | пакета effect у окна нет. |
| `ui/src/t3/shims/effect/Function.ts` | `effect/Function` | только то, что нужно перенесённому коду. |
| `ui/src/t3/shims/effect/Option.ts` | `effect/Option` | пакета effect у окна нет. |
| `ui/src/t3/shims/effect/Schema.ts` | `effect/Schema` | пакета effect у окна нет. |
| `ui/src/t3/shims/effect/reactivity.ts` | `effect/reactivity` | атомов состояния T3 у окна нет; фабрики возвращают пустые значения. |

## Зависимости для установки

Установку решает основатель через supply-gate. Версии — по `t3code/pnpm-lock.yaml` (apps/web). Пакетов, которых нет в окне и которые нужны отложенным файлам:

| Пакет | Версия | Зачем | Какой файл T3 импортирует | Можно ли обойтись нашим |
|---|---|---|---|---|
| `react-markdown` | 10.1.0 | разбор и вывод Markdown (основа ChatMarkdown) | ChatMarkdown.tsx, MarkdownCodeBlock.logic.ts | да: основа рендера чата; прежний marked удалён 20.11.2026 |
| `unified` | 11.0.5 | конвейер разбора | markdownPipeline.ts, markdownRenderer.ts, markdownSyntaxGates.ts, markdownMath.ts, markdownTaskList.ts, markdownWindowsPaths.ts, markdown-incremental.ts, codexMarkdownDirectives.ts и др. | нет |
| `remark-parse` | 11.0.0 | разбор Markdown в mdast | markdownPipeline.ts, markdownRenderer.ts, markdownMath.ts, markdownTaskList.ts, codexMarkdownDirectives.ts | нет |
| `remark-gfm` | 4.0.1 | таблицы, списки задач, зачёркивание | markdownPipeline.ts, markdownMath.ts, markdownTaskList.ts | да: GFM-разбор рендера чата; прежний marked удалён 20.11.2026 |
| `remark-math` | 6.0.0 | формулы $…$ (с подменой micromark-extension-math → micromark-extension-llm-math 3.1.0) | ChatMarkdown.tsx, markdownMath.ts | нет |
| `remark-breaks` | 4.0.0 | переводы строк как в чате | markdownPipeline.ts | нет (не было в списке ведущего) |
| `remark-rehype` | 11.1.2 | mdast → hast | markdownPipeline.ts, markdownRenderer.ts | нет |
| `rehype-raw` | 7.0.0 | сырой HTML в ответе (T3 включает по флагу) | markdownPipeline.ts | не нужен, если держать parseRawHtml выключенным; код его всё равно импортирует (не было в списке ведущего) |
| `rehype-sanitize` | 6.0.0 | очистка HTML | markdownPipeline.ts | нет (не было в списке ведущего) |
| `vfile` | 6.0.3 | файл для unified | markdownRenderer.ts | нет |
| `hast-util-to-jsx-runtime` | 2.3.6 | hast → элементы React | markdownRenderer.ts, HighlightedCodeLines.tsx | нет |
| `hast-util-to-html` | 9.0.5 | hast → HTML (копирование блока кода) | MarkdownCodeBlock.tsx, HighlightedCodeLines.tsx | нет |
| `html-url-attributes` | 3.0.1 | какие атрибуты содержат адреса | markdownRenderer.ts | нет |
| `mdast-util-from-markdown` | 2.0.3 | разбор для проверок синтаксиса | markdownSyntaxGates.ts | нет |
| `micromark-util-combine-extensions` | 2.0.1 | склейка расширений | markdownSyntaxGates.ts | нет |
| `micromark-util-types` | 2.0.2 | типы micromark | markdownSyntaxGates.ts, codexMarkdownDirectives.ts | только типы |
| `micromark-util-character` | 2.1.1 | классы символов | codexMarkdownDirectives.ts | нет (не было в списке ведущего) |
| `micromark-util-decode-string` | 2.0.1 | раскодирование экранирований | markdownWindowsPaths.ts, markdown-clipboard.ts | нет (не было в списке ведущего) |
| `micromark-extension-directive` | 4.0.0 | директивы Codex в тексте | codexMarkdownDirectives.ts | можно убрать вместе с codexMarkdownDirectives (в окне не нужны), но markdownPipeline.ts его импортирует |
| `mdast-util-directive` | 3.1.0 | то же | codexMarkdownDirectives.ts | то же |
| `property-information` | 7.2.0 | имена свойств HTML | markdownPipeline.ts | нет (не было в списке ведущего) |
| `@types/hast, @types/mdast` | 3.0.5, 4.0.4 | типы (в коде T3 это импорты `hast`, `mdast`) | markdownRenderer.ts, markdownPipeline.ts и др. | только типы |
| `katex` | 0.18.10 | формулы (+ katex.min.css и шрифты KaTeX) | MathTypeset.tsx, MermaidDiagram.tsx | нет. Версия 0.16.47 из списка ведущего — не из окна T3: apps/web просит ^0.18.2, в lock 0.18.10 |
| `dompurify` | 3.4.17 | очистка SVG диаграмм | MermaidDiagram.tsx | нет |
| `pele` | 1.8.0 | диаграммы (pele/lazy) | MermaidDiagram.tsx | нет |
| `@pierre/diffs` | 1.7.0-beta.4 (с патчем) | подсветка кода; тянет shiki 4.2.0 и @shikijs/themes 4.2.0 | diffRendering.ts, incrementalHighlighting.ts, readableSyntaxTheme.ts, syntaxHighlighting.ts, assemblyHighlighting.ts | нет |

**Патчи T3 в этой цепочке** (`t3code/patches/`, подключены через `patchedDependencies`): `micromark@4.0.2`, `micromark-extension-gfm-table@2.1.1`, `micromark-extension-llm-math@3.1.0`, `mdast-util-math@3.0.0`, `mdast-util-to-hast@13.2.1`, `@pierre/diffs@1.7.0-beta.4`. Без них поведение формул, таблиц и подсветки может отличаться от T3; решение о переносе патчей (в `ui/patches/` и `pnpm-workspace.yaml`) тоже за основателем. Подмена `remark-math>micromark-extension-math → micromark-extension-llm-math@3.1.0` задаётся там же (`overrides`).

Уже есть у окна и подходит: `react`, `react-dom`, `lucide-react`, `@base-ui/react` (кнопка, подсказка, меню T3 стоят на нём), `class-variance-authority`, `tailwind-merge`, `zustand`. `uplot` T3 не использует (`marked` из окна удалён 20.11.2026). `@legendapp/list` у T3 тот же, что у нас.

Прочие импорты T3 (`effect`, `@effect/atom-react`, `gt-react`, `@t3tools/*`, `@tiptap/*`, `@tanstack/*`) заменены прослойками или в перенесённую часть не попадают.

## Основа HOH-40: весь веб-клиент T3 (`ui/src/t3/base`)

План — `plans/hoh40-t3-base.md` (этап 2). Переносит `ui/scripts/t3-sync.py` по разделу `base` манифеста: весь `apps/web/src` без тестов
(`*.test.*`, `*.browser.test.*`, `*.bench.*`) и файлы пакетов T3 `packages/*/src`, до которых дотягиваются его импорты (граф —
`ui/scripts/t3graph.py`): `contracts`, `shared`, `client-runtime` и клиентские определения (`provider-core/client`, `provider-*/client`,
`source-control-*/shared`, `secret-provider-*`, `issue-tracker-linear/shared`) — видимый код строит по ним списки на уровне модуля.
Плюс `apps/web/gt.config.json` → `ui/src/t3/base/gt.config.json` (его читает `i18n/locales.ts`).
Свой проект tsc — `ui/src/t3/base/tsconfig.json` (настройки T3, псевдонимы `~/*` и `@t3tools/*` по `exports`; прежний tsconfig основу
не видит), псевдонимы для vite — `ui/src/t3/base/aliases.json`. В окно основу подключает вход u4 (`ui/src/t3-entry`); отдельная сборка
основы для проверки — `ui/src/t3/base/vite.config.ts` (`index.html` → их `web/bootstrap.ts`; с `T3_BASE_QA=1` ещё `qa.html`: T3 на хосте
u3 с пустым движком), снимки — `ui/scripts/t3-snapshot.mjs`; точка подключения описана в `ui/src/t3/base/README-HOH.md`.
Tailwind прежней оболочки её не сканирует (`@source not "../t3"`). Проверка импортов — `ui/scripts/t3-check.py` (оба набора).

Не переносятся (раздел `exclude` манифеста; файлов: **13**):

| Шаблон пути T3 | Почему |
|---|---|
| `^apps/web/src/components/device/models/.*\.glb$` | 3D-модели устройств скрытой части «устройства и пары» (~12 МБ): не переносятся (решение ведущего); импорты `?url` — адрес-заглушка stubs/absent-url.ts |
| `^apps/web/src/i18n/translations/(?!(en\|ru)\.json$)[^/]+$` | переводы, кроме en и ru, не переносятся (решение ведущего); список языков сужен правкой gt.config.json |

Файлов перенесено: **1974**, из них без изменений: **1924**, правок файлов T3: **51** (таблица ниже), НЕОЖИДАННО ОТЛИЧАЮТСЯ: web/components/device/models/ipad-pro-13-m5-magic-keyboard.glb, web/components/device/models/ipad-pro-13-m5.glb, web/components/device/models/iphone-18-pro-max.glb, web/components/device/models/iphone-18-pro.glb, web/components/device/models/iphone-duo.glb, web/i18n/translations/de.json, web/i18n/translations/es.json, web/i18n/translations/fr.json, web/i18n/translations/ja.json, web/i18n/translations/ko.json, web/i18n/translations/pt-BR.json, web/i18n/translations/sv.json, web/i18n/translations/zh.json.
Отложено (не проверяет tsc: в замыкании импортов есть пакет, которого нет в `ui/package.json` и `ui/node_modules` в версии, совместимой с lock T3: тот же major, для 0.x — тот же minor): **0** из 1949 файлов кода;
не хватает пакетов (сколько файлов T3 их импортирует): нет. Когда пакеты из `ui/T3-DEPS.md` (раздел HOH-40) стоят, отложенных нет —
`t3-sync.py` пересчитывает список сам. tsc основы — с настройками T3 (`strict`, `noImplicitAny`, `noUncheckedIndexedAccess`,
`exactOptionalPropertyTypes`); в проекте **1900** файлов. На заглушках: **81** файлов скрытых частей, из
которых через файлы скрытых частей достижим пакет-заглушка (офис, sql.js, three, heic/tiff, Clerk, daypicker и т.п.). Их тела без
настоящих пакетов не проверить (значения заглушек — `any`, у колбэков нет типов), поэтому tsc берёт вместо них объявления:
`X.hoh.d.ts` (tsc `--noCheck`) и `X.hoh.ts` с реэкспортом рядом с файлом, `moduleSuffixes: [".hoh", ""]`; все их импортёры, в том числе
видимые, проверяются полностью. Видимых файлов среди них нет. Пишет их `t3-sync.py`; vite эти файлы не берёт.

| Папка T3 | Папка у нас | Файлов | Из них кода | Без изменений | Правок | Отложено | На заглушках |
|---|---|---|---|---|---|---|---|
| `apps/web/src` | `ui/src/t3/base/web` | 1473 | 1436 | 1418 | 42 | 0 | 70 |
| `packages/contracts/src` | `ui/src/t3/base/contracts` | 79 | 79 | 77 | 2 | 0 | 0 |
| `packages/shared/src` | `ui/src/t3/base/shared` | 126 | 125 | 126 | 0 | 0 | 1 |
| `packages/client-runtime/src` | `ui/src/t3/base/client-runtime` | 267 | 267 | 263 | 4 | 0 | 10 |
| `packages/issue-tracker-linear/src` | `ui/src/t3/base/issue-tracker-linear` | 1 | 1 | 1 | 0 | 0 | 0 |
| `packages/provider-acp-registry/src` | `ui/src/t3/base/provider-acp-registry` | 2 | 2 | 2 | 0 | 0 | 0 |
| `packages/provider-core/src` | `ui/src/t3/base/provider-core` | 2 | 2 | 2 | 0 | 0 | 0 |
| `packages/provider-cursor/src` | `ui/src/t3/base/provider-cursor` | 2 | 2 | 2 | 0 | 0 | 0 |
| `packages/provider-fx/src` | `ui/src/t3/base/provider-fx` | 2 | 2 | 2 | 0 | 0 | 0 |
| `packages/provider-grok/src` | `ui/src/t3/base/provider-grok` | 2 | 2 | 2 | 0 | 0 | 0 |
| `packages/provider-kiro/src` | `ui/src/t3/base/provider-kiro` | 2 | 2 | 2 | 0 | 0 | 0 |
| `packages/provider-muse/src` | `ui/src/t3/base/provider-muse` | 2 | 2 | 2 | 0 | 0 | 0 |
| `packages/provider-opencode/src` | `ui/src/t3/base/provider-opencode` | 2 | 2 | 2 | 0 | 0 | 0 |
| `packages/provider-pi/src` | `ui/src/t3/base/provider-pi` | 2 | 2 | 2 | 0 | 0 | 0 |
| `packages/secret-provider-core/src` | `ui/src/t3/base/secret-provider-core` | 1 | 1 | 1 | 0 | 0 | 0 |
| `packages/secret-provider-definitions/src` | `ui/src/t3/base/secret-provider-definitions` | 1 | 1 | 1 | 0 | 0 | 0 |
| `packages/secret-provider-onepassword/src` | `ui/src/t3/base/secret-provider-onepassword` | 1 | 1 | 1 | 0 | 0 | 0 |
| `packages/source-control-azure-devops/src` | `ui/src/t3/base/source-control-azure-devops` | 2 | 2 | 2 | 0 | 0 | 0 |
| `packages/source-control-bitbucket/src` | `ui/src/t3/base/source-control-bitbucket` | 2 | 2 | 2 | 0 | 0 | 0 |
| `packages/source-control-core/src` | `ui/src/t3/base/source-control-core` | 2 | 2 | 2 | 0 | 0 | 0 |
| `packages/source-control-cursor-origin/src` | `ui/src/t3/base/source-control-cursor-origin` | 2 | 2 | 2 | 0 | 0 | 0 |
| `packages/source-control-definitions/src` | `ui/src/t3/base/source-control-definitions` | 1 | 1 | 1 | 0 | 0 | 0 |
| `packages/source-control-forgejo/src` | `ui/src/t3/base/source-control-forgejo` | 2 | 2 | 2 | 0 | 0 | 0 |
| `packages/source-control-gitcafe/src` | `ui/src/t3/base/source-control-gitcafe` | 2 | 2 | 2 | 0 | 0 | 0 |
| `packages/source-control-github/src` | `ui/src/t3/base/source-control-github` | 4 | 4 | 4 | 0 | 0 | 0 |
| `packages/source-control-gitlab/src` | `ui/src/t3/base/source-control-gitlab` | 3 | 3 | 3 | 0 | 0 | 0 |

### Правки файлов T3

| Файл (в `ui/src/t3/base`) | Как | Зачем |
|---|---|---|
| `web/components/T3Wordmark.tsx` | замена (наш файл) | знак T3 не берём: на его месте эмблема HoH (прозрачные растры src/assets/emblem-brand-*.png из emblem-a.webp, srcset 1×/1.5×/2×/3×) |
| `web/components/T3AppIcon.tsx` | замена (наш файл) | значок приложения T3 не берём: на его месте эмблема HoH; файл T3 ссылается на картинку вне apps/web/src |
| `contracts/orchestrationV2.ts` | правка (`ui/src/t3/base-patches/contracts/orchestrationV2.ts.patch`) | режим доступа закрепляется за отправляемым сообщением |
| `client-runtime/operations/commands.ts` | правка (`ui/src/t3/base-patches/client-runtime/operations/commands.ts.patch`) | режим композера сопровождает каждое отправленное и поставленное в очередь сообщение |
| `web/components/ChatView.tsx` | правка (`ui/src/t3/base-patches/web/components/ChatView.tsx.patch`) | текущий режим хода и выбор для следующего хода показаны раздельно |
| `web/components/chat/ComposerControlsBar.tsx` | правка (`ui/src/t3/base-patches/web/components/chat/ComposerControlsBar.tsx.patch`) | режим всегда остаётся в строке ввода, включая значение по умолчанию |
| `web/components/chat/ComposerFooterControls.tsx` | правка (`ui/src/t3/base-patches/web/components/chat/ComposerFooterControls.tsx.patch`) | чип показывает фактический режим и выбранный для следующего хода |
| `contracts/assets.ts` | правка (`ui/src/t3/base-patches/contracts/assets.ts.patch`) | превью изображений: локальные Blob URL разрешены контрактом assets.createUrl без сетевого источника в CSP; отказ нативной политики несёт ограниченную безопасную причину |
| `client-runtime/markdownImages.ts` | правка (`ui/src/t3/base-patches/client-runtime/markdownImages.ts.patch`) | безопасность превью: data URI из ответа ассистента не передаётся напрямую в img |
| `client-runtime/state/assets.ts` | правка (`ui/src/t3/base-patches/client-runtime/state/assets.ts.patch`) | превью изображений: ограниченная причина отказа assets.createUrl сохраняется в состоянии ресурса для видимой заглушки; история URL ограничена LRU |
| `web/components/ChatMarkdown.tsx` | правка (`ui/src/t3/base-patches/web/components/ChatMarkdown.tsx.patch`) | превью изображений: обычная markdown-ссылка на локальный растровый файл использует штатные T3 ChatMarkdownAssetImage и крупный просмотр |
| `web/components/chat/ChatHeader.tsx` | правка (`ui/src/t3/base-patches/web/components/chat/ChatHeader.tsx.patch`) | состояние доверия к настройкам папки и действие подтверждения окна |
| `web/components/chat/ChatMarkdownMedia.tsx` | правка (`ui/src/t3/base-patches/web/components/chat/ChatMarkdownMedia.tsx.patch`) | превью изображений: заглушка Image unavailable показывает ограниченную безопасную причину отказа нативной политики |
| `web/components/chat/ChatComposer.tsx` | правка (`ui/src/t3/base-patches/web/components/chat/ChatComposer.tsx.patch`) | скрепка в поле ввода доступна для поддерживаемых Claude изображений без объявления generic file attachments |
| `web/components/chat/ComposerAttachmentStrip.tsx` | правка (`ui/src/t3/base-patches/web/components/chat/ComposerAttachmentStrip.tsx.patch`) | диалог скрепки принимает только GIF, JPEG, PNG и WebP, показывает лимит 10 МиБ и использует общий путь addComposerAttachments |
| `web/components/chat/SkillInlineText.tsx` | правка (`ui/src/t3/base-patches/web/components/chat/SkillInlineText.tsx.patch`) | скрывает View instructions у навыка Claude: движок даёт имя, но не безопасный путь к файлу |
| `web/components/ComposerPromptEditorTiptap.tsx` | правка (`ui/src/t3/base-patches/web/components/ComposerPromptEditorTiptap.tsx.patch`) | скрывает открытие файла для навыка Claude с внутренним hoh-skill идентификатором |
| `web/routeTree.gen.ts` | правка (`ui/src/t3/base-patches/web/routeTree.gen.ts.patch`) | выключатель: маршруты скрытых частей (вход и пары, usage, pull requests, issues, скрытые разделы настроек) не регистрируются; типы маршрутов остаются |
| `web/components/settings/settingsSearch.ts` | правка (`ui/src/t3/base-patches/web/components/settings/settingsSearch.ts.patch`) | выключатель: разделы настроек скрытых частей не показываются в боковой колонке и не ищутся (HOH_SETTINGS_PATHS) |
| `web/components/sidebar/SidebarChrome.tsx` | правка (`ui/src/t3/base-patches/web/components/sidebar/SidebarChrome.tsx.patch`) | имя продукта в шапке боковой колонки — HoH вместо «Code» при знаке T3 |
| `web/terminalUiStateStore.ts` | правка (`ui/src/t3/base-patches/web/terminalUiStateStore.ts.patch`) | выключатель: терминал — ящик терминала не открывается (normalizeThreadTerminalUiState держит terminalOpen: false), клавиши и действия ничего не открывают |
| `web/components/chat/PanelLayoutControls.tsx` | правка (`ui/src/t3/base-patches/web/components/chat/PanelLayoutControls.tsx.patch`) | выключатель: терминал — кнопки ящика терминала нет (showTerminalControl по умолчанию false) |
| `web/components/settings/ThemeSettings.tsx` | правка (`ui/src/t3/base-patches/web/components/settings/ThemeSettings.tsx.patch`) | выключатель: импорт тем VS Code / Open VSX — кнопки «Add theme» нет, диалог импорта не открывается |
| `gt.config.json` | правка (`ui/src/t3/base-patches/gt.config.json.patch`) | языки интерфейса: английский и русский (остальные переводы не переносятся); gt.config.json T3 читает web/i18n/locales.ts |
| `web/branding.ts` | правка (`ui/src/t3/base-patches/web/branding.ts.patch`) | имя продукта: HoH вместо «T3 Code» (заголовок окна, о программе); у T3 его меняет только desktopBridge, которого у окна нет |
| `web/components/chat/turnDetailAnnouncement.ts` | правка (`ui/src/t3/base-patches/web/components/chat/turnDetailAnnouncement.ts.patch`) | имя продукта: подпись автора ответа для экранного чтеца — APP_BASE_NAME (HoH) вместо «T3 Code» |
| `web/components/ChatView.logic.ts` | правка (`ui/src/t3/base-patches/web/components/ChatView.logic.ts.patch`) | термин нового разговора: видимый заголовок черновика — New chat вместо New thread |
| `web/components/chat/DraftHeroHeadline.tsx` | правка (`ui/src/t3/base-patches/web/components/chat/DraftHeroHeadline.tsx.patch`) | стабильная пустая главная: проект HoH с постоянным id всегда показывает продуктовый вопрос без подписи scratch-проекта |
| `web/components/CommandPalette.tsx` | правка (`ui/src/t3/base-patches/web/components/CommandPalette.tsx.patch`) | термин нового разговора: видимые действия палитры — New chat вместо New thread |
| `web/components/chat/runtimeModeConfig.ts` | правка (`ui/src/t3/base-patches/web/components/chat/runtimeModeConfig.ts.patch`) | режим доступа: только Supervised и Full access выбираются; Auto-accept edits и Auto видны как Coming Soon |
| `web/components/chat/MessagesTimeline.logic.ts` | правка (`ui/src/t3/base-patches/web/components/chat/MessagesTimeline.logic.ts.patch`) | остановленный пользователем ход сохраняет подпись You stopped после перезапуска и после следующих ходов |
| `client-runtime/state/projects.ts` | правка (`ui/src/t3/base-patches/client-runtime/state/projects.ts.patch`) | идентичность проекта HoH: No project определяется по id, даже если настоящий проект совпал с папкой запуска |
| `web/components/threadActionMenu.logic.ts` | правка (`ui/src/t3/base-patches/web/components/threadActionMenu.logic.ts.patch`) | меню без неподдерживаемых действий: Mark unread, удаление, смена статуса, перегенерация названия; закрепление сохраняется |
| `web/components/chat/ThreadDetailsItemsMenu.tsx` | правка (`ui/src/t3/base-patches/web/components/chat/ThreadDetailsItemsMenu.tsx.patch`) | в настройках сведений о разговоре остаётся Context window |
| `web/components/settings/ThreadDetailsItemsRow.tsx` | правка (`ui/src/t3/base-patches/web/components/settings/ThreadDetailsItemsRow.tsx.patch`) | счётчик Thread details учитывает только Context window |
| `web/components/chat/TimelineAssistantRows.tsx` | правка (`ui/src/t3/base-patches/web/components/chat/TimelineAssistantRows.tsx.patch`) | скрыты неподдерживаемые fork и reply |
| `web/components/chat/TimelineUserMessageRow.tsx` | правка (`ui/src/t3/base-patches/web/components/chat/TimelineUserMessageRow.tsx.patch`) | скрыты неподдерживаемые edit from here и ссылка на запрос |
| `web/components/chat/threadDetailsItems.ts` | правка (`ui/src/t3/base-patches/web/components/chat/threadDetailsItems.ts.patch`) | неподдерживаемые сведения скрыты даже при сохранённых настройках |
| `web/components/Sidebar.tsx` | правка (`ui/src/t3/base-patches/web/components/Sidebar.tsx.patch`) | массовое меню без неподдерживаемых действий; закрепление сохраняется |
| `web/components/files/FilePreviewPanel.tsx` | правка (`ui/src/t3/base-patches/web/components/files/FilePreviewPanel.tsx.patch`) | неподдерживаемый просмотр файлов и открытие в редакторе обозначены Coming Soon; вложения сохраняют просмотр |
| `web/components/chat/useThreadLifecycle.tsx` | правка (`ui/src/t3/base-patches/web/components/chat/useThreadLifecycle.tsx.patch`) | не регистрирует неподдерживаемые сочетания закрепления, смены статуса и перегенерации |
| `web/components/onboarding/WelcomePage.tsx` | правка (`ui/src/t3/base-patches/web/components/onboarding/WelcomePage.tsx.patch`) | установка и вход агента без терминального API обозначены Coming Soon |
| `web/hooks/useThreadActionMenu.ts` | правка (`ui/src/t3/base-patches/web/hooks/useThreadActionMenu.ts.patch`) | настройки проекта предлагаются только при наличии проекта |
| `web/components/chat/MarkdownFileLink.tsx` | правка (`ui/src/t3/base-patches/web/components/chat/MarkdownFileLink.tsx.patch`) | ссылки на поддерживаемые изображения открывают просмотр изображений, минуя неподключённый просмотр файлов |
| `web/components/chat/WorkspaceEntryIcon.tsx` | правка (`ui/src/t3/base-patches/web/components/chat/WorkspaceEntryIcon.tsx.patch`) | значок и подпись файла не вызывают неподдерживаемый filesystem.getMetadata |
| `web/hooks/useT3ProjectFileScripts.ts` | правка (`ui/src/t3/base-patches/web/hooks/useT3ProjectFileScripts.ts.patch`) | не запрашивает неподдерживаемый файл настроек проекта через projects.readFile |
| `web/lib/t3ProjectFileDefaults.ts` | правка (`ui/src/t3/base-patches/web/lib/t3ProjectFileDefaults.ts.patch`) | не запрашивает неподдерживаемый файл настроек проекта через projects.readFile |
| `web/components/settings/SettingsScopeContext.tsx` | правка (`ui/src/t3/base-patches/web/components/settings/SettingsScopeContext.tsx.patch`) | не запрашивает неподдерживаемый файл настроек проекта через projects.readFile |
| `web/state/projectClones.ts` | правка (`ui/src/t3/base-patches/web/state/projectClones.ts.patch`) | скрытая функция возвращает пустое состояние без неподдерживаемой подписки |
| `web/state/device.ts` | правка (`ui/src/t3/base-patches/web/state/device.ts.patch`) | скрытая функция возвращает пустое состояние без неподдерживаемой подписки |
| `web/appInit.tsx` | правка (`ui/src/t3/base-patches/web/appInit.tsx.patch`) | не запускает скрытый просмотр устройств и неподдерживаемую подписку на них |
| `web/components/settings/ArchivedThreadsPanel.tsx` | правка (`ui/src/t3/base-patches/web/components/settings/ArchivedThreadsPanel.tsx.patch`) | в архиве удаление обозначено Coming Soon до поддержки движком |

Имя продукта: `APP_BASE_NAME` (`web/branding.ts`) T3 берёт из `window.desktopBridge.getAppBranding()`, иначе «T3 Code»; окно
`desktopBridge` не задаёт (режим браузера, `ui/src/hoh-host/README.md`), поэтому имя HoH — правкой `web/branding.ts`. В текстах T3 имя «T3 Code» встречается и буквально: в коде `web` — 136 строк, в русском переводе `web/i18n/translations/ru.json` — 144; большая часть — в скрытых частях. Замена в видимых текстах — э4 (через перевод, без правки файлов кода).

### Скрытые части: чем выключены

Файлы скрытых частей остаются в дереве. Выключатель — правка из таблицы выше, возможность окружения (`ExecutionEnvironmentCapabilities`
в `contracts/environment.ts`), которую объявляет хост HoH (э3), или переменная сборки, которой в окне нет.

| Часть | Чем выключена | Состояние |
|---|---|---|
| git и source control, запросы на слияние, задачи и трекеры, review, worktree, клон проекта | маршруты /pull-requests и /issues и разделы настроек не регистрируются (правка routeTree.gen.ts, settingsSearch.ts); кнопки в боковой колонке и палитре — по возможностям окружения (usePullRequestsSupported, useIssuesSupported, capabilities.pullRequests/issues/vcsBranchStatus): хост их не объявляет | правка + хост (э3) |
| терминал | ящик терминала не открывается (правка terminalUiStateStore.ts: terminalOpen всегда false), кнопки ящика нет (правка PanelLayoutControls.tsx); пункт «Terminal» в меню правой панели — по области AuthTerminalOperateScope от хоста (как Browser, Diff, Device): хост её не даёт; terminal.* хост отвечает «не поддерживается» | правка + хост (э3) |
| превью и встроенный браузер | capabilities.serverBrowser и находимые порты — от хоста; раздел настроек «Браузер» не регистрируется | правка + хост (э3) |
| облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh | оболочка входа грузится только при VITE_CLERK_PUBLISHABLE_KEY (main.tsx) — в окне пусто; маршруты /connect, /connect-agent, /approve-agent, /pair, /guest не регистрируются; capabilities.peerLinks — от хоста; @clerk/* — заглушки | правка + переменная сборки + хост (э3) |
| запланированные задачи и отложенные разговоры (snooze) | раздел настроек не регистрируется; capabilities.threadSnooze, oneTimeSchedules — от хоста | правка + хост (э3) |
| MCP Apps | появляются только в ответах сервера с MCP-приложением: хост таких не шлёт | хост (э3) |
| панель изменений (diff) | вкладка правой панели RightPanelTabs.tsx и раздел настроек «Diffs»; раздел не регистрируется, вкладка — по данным хоста (изменений нет) | правка + хост (э3) |
| офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) | просмотр включается по типу файла в панели файлов; пакеты — заглушки (jszip — прослойка с типами), файлы таких типов не открываются | заглушки; отказ — э3 |
| вложения (загрузка) и снимки экрана (snap-shot) | capabilities.fileAttachments, attachmentUploads, questionAttachments — от хоста; раздел «SnapShots» не регистрируется | правка + хост (э3) |
| использование, телеметрия, диагностика, ресурсы | маршрут /usage и разделы «Диагностика», «Хранилище» не регистрируются; capabilities.usageLimitSources — от хоста; трассы уходят только при VITE_RELAY_OTLP_TRACES_URL — в окне пусто | правка + переменная сборки + хост (э3) |
| импорт и экспорт тем VS Code / Open VSX | кнопки «Add theme» в настройках вида нет (правка ThemeSettings.tsx) — диалог импорта и поиск тем (ThemeImportDialog.tsx, ThemeSearchSection.tsx) не открываются; пакеты — заглушки | правка |
| секреты и 1Password | раздел есть в навигации только при T3CODE_FEATURE_SECRETS (в окне пусто) и не регистрируется; capabilities.secretSourceCheck — от хоста | правка + переменная сборки |
| GitHub-звезда и конфетти | GitHubStarPrompt.tsx никто не импортирует (у T3 тоже); звезда — через source control, которого нет | не подключено у T3 |
| голосовой ввод | capabilities.voiceTranscription, cloudVoiceTranscription — от хоста; раздел «Голос» не регистрируется | правка + хост (э3) |
| разделы настроек, кроме общих, вида, чата, провайдеров и архива | не регистрируются (routeTree.gen.ts), не показываются и не ищутся (settingsSearch.ts, HOH_SETTINGS_PATHS) | правка |
| обновления приложения | capabilities.desktopAppUpdate, serverSelfUpdate, npmReleaseLookup — от хоста | хост (э3) |
| ACP-реестр, установка и вход провайдеров T3 | @t3tools/provider-* — заглушки; список провайдеров даёт providerClients.ts из client-runtime — в э3 рабочая прослойка с исполнителями HoH | заглушки; прослойка — э3 |
| импорт сессий агентов | раздел «Интеграции» не регистрируется; импорт сессий остаётся в карточке провайдера (AcpSessionManagementSection.tsx из ProviderInstanceCard.tsx) и на странице приветствия (WelcomePage.tsx) — выключит хост, не объявив сессий агентов | правка; остаток — э3/э4 |

### Заглушки и прослойки пакетов

Пакеты скрытых частей и пакеты T3, которые не берутся (список и причины — `ui/T3-DEPS.md`, «Заглушено вместо установки»). Созданная
заглушка — функция: её вызов и `new` дают объект, любое свойство которого — функция, возвращающая `undefined` (`stubs/absent.ts`,
образец — пробная сборка u4: так проходит `export const { a } = make(...)` на уровне модуля); типы — `any` с пространством имён
для обращений через точку. Где коду нужны точные типы или поведение, вместо созданной лежит наша прослойка (`shims/`, `types/`).
Адреса файлов пакетов (`?url`) и непереносимых файлов (модели `.glb`) указывают на `stubs/absent-url.ts`. Пакеты T3 (`@t3tools/*`)
не заглушаются: их клиентские файлы переносятся. i18n (`gt-react`, `gt-i18n`) — прослойка окна: строки T3 по-английски как есть,
подстановка переменных ICU, перевод — по словарю языка (`registerDictionary`, пока пусто), без сети.

| Пакет (путь импорта) | Что | Файл | Почему |
|---|---|---|---|
| `shims/gt-i18n.ts` | прослойка (наш файл) | `ui/src/t3/base/shims/gt-i18n.ts` | Прослойка окна HoH вместо gt-i18n (General Translation, i18n T3 Code). Настоящий пакет не ставим (решение CEO): |
| `shims/gt-react.tsx` | прослойка (наш файл) | `ui/src/t3/base/shims/gt-react.tsx` | Прослойка окна HoH вместо gt-react (General Translation, i18n T3 Code). Настоящий пакет не ставим (решение CEO): |
| `jszip` | прослойка (наш файл) | `ui/src/t3/base/shims/jszip.ts` | офисные и двоичные форматы и импорт тем VS Code скрыты, архивы |
| `shims/gt-i18n/types.ts` | прослойка (наш файл) | `ui/src/t3/base/shims/gt-i18n/types.ts` | Прослойка окна HoH вместо gt-i18n/types (General Translation, i18n T3 Code): тип функции сообщений. |
| `types/trusted-types/lib/index.d.ts` | прослойка типов (наш файл) | `ui/src/t3/base/types/trusted-types/lib/index.d.ts` | Типы Trusted Types для dompurify (его d.ts импортирует `trusted-types/lib/index.js` из @types/trusted-types — у T3 он |
| `vite-plus/client` | прослойка типов (наш файл) | `ui/src/t3/base/types/vite-plus/client/index.d.ts` | T3 собирает через Vite+, окно — обычным vite, типы те же. |
| `canvas-confetti` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/canvas-confetti.ts` | GitHub-звезда и конфетти |
| `entities` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/entities.ts` | офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) |
| `fast-xml-parser` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/fast-xml-parser.ts` | офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) |
| `jsonc-parser` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/jsonc-parser.ts` | импорт и экспорт тем VS Code / Open VSX |
| `saxes` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/saxes.ts` | офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) |
| `sql.js` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/sql.js.ts` | офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) |
| `three` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/three.ts` | облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh |
| `tiff` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/tiff.ts` | офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) |
| `@betteroffice/fonts` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/@betteroffice/fonts.ts` | офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) |
| `@betteroffice/pptx` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/@betteroffice/pptx.ts` | офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) |
| `@betteroffice/xlsx` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/@betteroffice/xlsx.ts` | офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) |
| `@betteroffice/docx/editor` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/@betteroffice/docx/editor.ts` | офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) |
| `@betteroffice/docx/layout` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/@betteroffice/docx/layout.ts` | офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) |
| `@betteroffice/docx/yrs` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/@betteroffice/docx/yrs.ts` | офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) |
| `@betteroffice/docx/layout/render` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/@betteroffice/docx/layout/render.ts` | офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) |
| `@betteroffice/xlsx/headless` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/@betteroffice/xlsx/headless.ts` | офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) |
| `@clerk/react` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/@clerk/react.ts` | облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh |
| `@clerk/electron/passkeys` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/@clerk/electron/passkeys.ts` | облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh |
| `@clerk/electron/react` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/@clerk/electron/react.ts` | облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh |
| `@daypicker/react` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/@daypicker/react.ts` | скрытые части |
| `@noble/curves/nist` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/@noble/curves/nist.ts` | скрытые части |
| `heic-to/csp` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/heic-to/csp.ts` | вложения (загрузка) и снимки экрана (snap-shot); офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic) |
| `three/addons/environments/RoomEnvironment.js` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/three/addons/environments/RoomEnvironment.js.ts` | облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh |
| `three/addons/loaders/GLTFLoader.js` | заглушка (создана t3-sync.py) | `ui/src/t3/base/stubs/three/addons/loaders/GLTFLoader.js.ts` | облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh |

### Тема, шрифты, значки

- Тема — `web/index.css` T3 без правок (переменные, цвета, радиусы, тени, размеры текста, отступы; `@import "tailwindcss"`), подключается
  из `web/main.tsx` как у T3; других CSS-импортов у неё нет (`components/files/pdfViewer.css` — у просмотра PDF).
- Шрифт — Inter Variable из пакета `@fontsource-variable/inter` (лицензия SIL OFL 1.1: встраивать и распространять с программой можно),
  его импортирует `web/main.tsx`; пакет уже есть в окне. Моноширинный — системный (`--font-mono` T3). Шрифт значков терминала
  `web/terminal/ghostty/fonts/SymbolsNerdFontMono-Regular.woff2` — MIT (Ryan L McIntyre, файл `LICENSE` рядом), перенесён с терминалом (скрыт).
- Значки — `lucide-react` (ISC) и встроенные SVG компонентов T3; логотипы MCP-приложений (`web/components/mcp/logos/*.png`) перенесены
  с MCP Apps (скрыты). Значки приложения T3 (`apps/web/public/*`, `assets/prod/*`) не берутся: знак — эмблема HoH (`src/assets/emblem-*.png`).
- Объём перенесённого: 38.7 МБ; крупнее всего — .glb — 12.1 МБ, .ts — 9.3 МБ, .tsx — 7.8 МБ, .json — 7.5 МБ, .woff2 — 1.2 МБ, .wasm — 0.7 МБ. Переводы T3 (`web/i18n/translations/*.json`) — все 10 языков, как у T3.

Главная (HOH-41 В2б) — пустой разговор T3 целиком, поле ввода их: `/` у T3 открывает черновик (`routes/_chat.index.tsx` не тронут), в нём `ChatView.tsx` с чипом модели и уровнями, Permissions, выбором проекта, отправкой. Подстановки при сборке (`vite.ts`, `chatViewHome`, `noProjectsHero`): (1) `components/ChatView.tsx` — пустой draft hero становится стеком `hoh-draft-hero`, перед нативной composer-группой вставляется `<HomeBody/>` (`src/t3-entry/hoh-screens/HomeBody.tsx`); пустой компактный стек центрируется по высоте как у T3 (и опускается вниз при `max-height:34rem`), занятая главная остаётся снизу с прокручиваемым телом; (2) `components/NoProjectsHero.tsx` — заголовок «What should we work on?» → приветствие с датой (`Greeting`), эмблема, «Add project» и «Start without a project» их. `HomeBody` (карточки — вариант A, выбор основателя; B/C и переключатель убраны): значок Claude (T3 `ClaudeAI`) у каждого агента, субагенты — Task-вызовы Claude (`host.home`, `agents`); приветствие с датой, «Now» (карточки открывают `QuickViewDialog`, внутри тот же T3 `ChatView(threadId)` в embedded-режиме: лента, композер и подтверждения используют общий store; полный переход остаётся обычной навигацией), «Done», «Needs your answer» (вид запроса T3: «Command approval» + команда/файл, Approve/Decline — тот же ответ, что в разговоре). Экспорт `openQuickView(threadId)` доступен карточкам раскрытых папок проектов. Данные — `window.__HOH_HOME__` (хост, `host.home`), не контракты T3. Раскладка — `home.css` (токены темы), оболочка быстрого просмотра — `quick-view.css`.

Выравнивание HOH-45 A: внешняя геометрия окна снова берётся из T3 без переопределений — нативная/изменяемая ширина sidebar, `--chat-content-max-width: 46rem`, поля `.chat-composer-lane` (`.75rem`, с `sm` — `3rem`) и её вычисленный content-box; `HomeBody`, Learning и Plugins повторяют эту колонку, корешок следует живой composer-полосе (включая асимметричный inset панели деталей), T3 Dialog остаётся центрирован по горизонтальной оси полного viewport. Пока Quick View открыт, только соседняя с `HomeBody` полоса основного composer скрыта, чтобы не проступать сквозь backdrop; composer внутри Dialog не затрагивается. Golden Canon задаёт только внутренний ритм наших карточек/папок. Матрицу размеров, состояний sidebar, экранов, папки-disclosure и Quick View, ось корешка и вертикальный центр пустой группы измеряет `scripts/check-align.mjs`, который вызывается `scripts/build.sh`.

Группировка главной (HOH-44B) — `ProjectGroupingSetting.tsx` подставлен в Appearance → Themes через `vite.ts`; локальная настройка переживает перезапуск. Выключено — прежние `Card` одним списком; включено — `HomeBody` группирует их в папки проектов (включая «No project»), цвет берёт из хеша пути `host.home.projectKey`, а те же `Card` раскрывает по hover/click/Enter/Space в ограниченном экраном portal (Esc/клик снаружи закрывает, reduced motion без движения). Вид — `home.css`, логика/store — `hoh-screens/grouping.ts`.

Providers (HOH-47 U4) — build-time подстановка `providerSettingsPanel` в `src/t3-entry/vite.ts` оставляет Claude единственным исполнителем и показывает неподвижную строку «Channel plugins are disabled in the window» без переключателя: движок сам накладывает запрет только на канальные плагины Claude, а пользовательские настройки остальных плагинов остаются как в терминале.

Боковая панель (HOH-41, состав раунда 12: `design/round12/shots/home.png`) — подстановки при сборке (`vite.ts`, `sidebarList`, `sidebarChrome`), файлы T3 не правятся: (1) `components/Sidebar.tsx` — вместо `SidebarThreadHeader` (поиск, фильтр, «Add project», «New thread») рисуется `SidebarTop` (`src/t3-entry/hoh-screens/SidebarTop.tsx`: «New chat» с сочетанием клавиш T3, группа «Team» с «+» без строк — движка сотрудников нет, «+» выключена, «Recent chats» над списком T3); полка Working выключена (`useClientSettings(…sidebarWorkingShelfEnabled)` → `false`), все разговоры — строки в один ряд «slim» (без строки проекта; `isCard` → `false`); (2) тот же файл: в «Recent chats» не попадает разговор без первого сообщения (пустой черновик; хост даёт ему `latestUserMessageAt = null`, `threads.ts`); (3) `components/sidebar/SidebarChrome.tsx` — у «Settings» внизу рядом со значком подпись. Заголовки полок Working/Snoozed/Settled и кнопки «Settle»/«Snooze» в строке скрыты правилами `HIDDEN_SELECTORS` (`features.ts`). Две кнопки справа в шапке разговора («Toggle thread details panel», «Toggle right panel») и вся шапка с «No project / New thread» на главной (пустой черновик) скрыты теми же правилами. «Learning» открывает пустую страницу «Coming soon»; «Plugins» на месте прежнего «Widgets» использует карточки списка и полную страницу деталей выбранного плагина, а до появления engine API показывает честное пустое состояние без образцов и переключателя вариантов (`hoh-screens/SidebarPages.tsx`, `PluginsPage.tsx`, `plugins.css`, `pages.ts`). Оба экрана — слой поверх области справа от панели, не маршруты T3; поведение проверяет `scripts/check-grid.mjs`, матрицу снимков — `scripts/check-align.mjs`. `SidebarThreadHeader` больше не в `HIDDEN_COMPONENTS`.
# Режим сообщения U0: `contracts/orchestrationV2.ts`, `client-runtime/operations/commands.ts` и `web/components/ChatView.tsx` переносят выбранный режим с сообщением и различают текущий и следующий ход.

- **HOH-47 U1:** Sharing & access скрыт; в настройках Thread details оставлен Context window, меню значка среды и Mark unread скрыты. Team, Learning и установка Plugins обозначены Coming Soon; неподдерживаемые действия fork, reply, edit и ссылка на запрос в ленте скрыты; Open in editor в просмотре файла обозначен Coming Soon, поскольку хост его не поддерживает.
- Меню разговоров скрывает неподдерживаемые смену статуса и перегенерацию названия; удаление обозначено Coming Soon.
- Сочетания клавиш смены статуса и перегенерации разговора не регистрируются до поддержки хостом; закрепление доступно.
- При отсутствии Claude установка и вход без терминального API обозначены Coming Soon; окно не предлагает неподключённый терминал.
- Настройки проекта в меню разговора доступны только при наличии соответствующего проекта.
- Просмотр произвольных файлов без `projects.readFile`/`projects.listEntries` показывает Coming Soon; ссылки на GIF/JPEG/PNG/WebP и вложения сохраняют поддерживаемый просмотр изображений.
- Значок файла в ответе не запрашивает неподдерживаемую метаинформацию; имя и значок по расширению остаются.
- Не запрашиваем неподдерживаемый T3 project file в деталях, настройках и при создании разговора; используются доступные настройки окна и проекта.
- Скрытые устройства и клонирование получают пустое состояние без молчащих subscribeDeviceState и subscribeProjectClones.
- Запуск скрытых device/preview-наблюдателей отключён, поэтому подписка на устройства не открывается при старте окна.

- fix1: Rename, Pin/Unpin и Archive сохраняются в постоянных метаданных окна; Delete показывает Coming Soon до появления удаления сессий в движке.
- Архив доступен в Settings: список берётся из сохранённого состояния хоста, Unarchive восстанавливает разговор, Delete обозначен Coming Soon.

Длительность хода сохраняет время отправки в метаданных окна по идентификатору сообщения двигателя; остановленный ход без шагов сворачивается с подписью You stopped, карточка готового ответа не показывает прогресс.

HOH-47 S4: чип Permissions постоянно находится в строке ввода; при несовпадении фактического и следующего режима его подпись — Current / Next. Отдельная подпись черновика удалена. Патчи: `ChatView.tsx`, `ChatComposer.tsx`, `ComposerControlsBar.tsx`, `ComposerFooterControls.tsx`.
