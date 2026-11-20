# Карта кода T3 Code в окне HoH

Источник: T3 Code, MIT License, Copyright (c) 2026 T3 Tools Inc. (`/mnt/toshiba/alexandrie/third-party/t3code`, коммит 803f21f6, только чтение). Полный текст лицензии — `ui/NOTICE.md`. Названий и знаков T3 в окне нет: перенесены только файлы разметки ответа, в сборку попадают только подключённые.

Файлы переносит `ui/scripts/t3-sync.py` по списку `ui/src/t3/manifest.json` (копия без правок; расхождений с источником сейчас: 0). Дерево повторяет T3: `apps/web/src` → `ui/src/t3/web`, `packages/shared/src` → `ui/src/t3/shared`, `packages/client-runtime/src` → `ui/src/t3/client-runtime`. Псевдонимы пакетов (`@t3tools/*`, `gt-react`, `effect/*`) прописывает тот же скрипт в `ui/src/t3/tsconfig.paths.json` (tsc) и `ui/src/t3/aliases.json` (vite). Карту обновляет `ui/scripts/t3-map.py`; целостность импортов проверяет `ui/scripts/t3-check.py`.

## Состояние

- **В сборке сейчас:** рендер ответов на marked (`ui/src/lib/markdown.tsx`) через переключатель `ui/src/t3/render.tsx`. Файлы T3 из первой таблицы проверены компилятором (`tsc --noEmit`), но чат их пока не вызывает.
- **Отложено:** вторая таблица — файлы T3, которым нужны пакеты, которых нет в lock окна. До установки tsc их пропускает (`deferredEnabled: false`), vite их не видит, Tailwind не сканирует (`@source not "../t3"` в tokens.css). Компилятором они не проверены: проверены только адресаты импортов (`t3-check.py`).
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

## Перенесено без изменений, отложено (нужны пакеты)

| Путь в T3 | Путь у нас | Изменения |
|---|---|---|
| `packages/shared/src/markdownPipeline.ts` | `ui/src/t3/shared/markdownPipeline.ts` | без изменений |
| `packages/shared/src/markdownMath.ts` | `ui/src/t3/shared/markdownMath.ts` | без изменений |
| `packages/shared/src/markdownTaskList.ts` | `ui/src/t3/shared/markdownTaskList.ts` | без изменений |
| `packages/shared/src/markdownWindowsPaths.ts` | `ui/src/t3/shared/markdownWindowsPaths.ts` | без изменений |
| `packages/shared/src/markdownGithubAlerts.ts` | `ui/src/t3/shared/markdownGithubAlerts.ts` | без изменений |
| `packages/shared/src/markdownLeadingWhitespace.ts` | `ui/src/t3/shared/markdownLeadingWhitespace.ts` | без изменений |
| `packages/shared/src/markdownListIndentation.ts` | `ui/src/t3/shared/markdownListIndentation.ts` | без изменений |
| `packages/shared/src/markdownOrderedListNumbers.ts` | `ui/src/t3/shared/markdownOrderedListNumbers.ts` | без изменений |
| `packages/shared/src/codexMarkdownDirectives.ts` | `ui/src/t3/shared/codexMarkdownDirectives.ts` | без изменений |
| `packages/shared/src/codexArtifactTemplates.ts` | `ui/src/t3/shared/codexArtifactTemplates.ts` | без изменений |
| `packages/shared/src/codexFileCitations.ts` | `ui/src/t3/shared/codexFileCitations.ts` | без изменений |
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
| `react-markdown` | 10.1.0 | разбор и вывод Markdown (основа ChatMarkdown) | ChatMarkdown.tsx, MarkdownCodeBlock.logic.ts | частично: marked 18.2.0 (нынешний рендер) даёт блоки, но не hast-слой, на котором стоят код, таблицы, формулы и ссылки T3 |
| `unified` | 11.0.5 | конвейер разбора | markdownPipeline.ts, markdownRenderer.ts, markdownSyntaxGates.ts, markdownMath.ts, markdownTaskList.ts, markdownWindowsPaths.ts, markdown-incremental.ts, codexMarkdownDirectives.ts и др. | нет |
| `remark-parse` | 11.0.0 | разбор Markdown в mdast | markdownPipeline.ts, markdownRenderer.ts, markdownMath.ts, markdownTaskList.ts, codexMarkdownDirectives.ts | нет |
| `remark-gfm` | 4.0.1 | таблицы, списки задач, зачёркивание | markdownPipeline.ts, markdownMath.ts, markdownTaskList.ts | marked разбирает GFM сам, но вывод T3 строится на этом плагине |
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

Уже есть у окна и подходит: `react`, `react-dom`, `lucide-react`, `@base-ui/react` (кнопка, подсказка, меню T3 стоят на нём), `class-variance-authority`, `tailwind-merge`, `zustand`. `marked` и `uplot` T3 не использует. `@legendapp/list` у T3 тот же, что у нас.

Прочие импорты T3 (`effect`, `@effect/atom-react`, `gt-react`, `@t3tools/*`, `@tiptap/*`, `@tanstack/*`) заменены прослойками или в перенесённую часть не попадают.

## Известные отличия от T3

- katex для micromark-extension-llm-math закреплён на 0.18.10 вместо 0.16.47 из lock T3 (GHSA-238p-pmpm-9mq7; html-часть llm-math у нас не используется), решение CEO 20.11.2026.
- @pierre/diffs не поставлен: не проходит выдержку 7 дней (minimumReleaseAge), решение CEO 20.11.2026; вид диффов — на имеющихся компонентах.
- Патчи T3 разбора Markdown (5 шт., ui/patches/) применены через patchedDependencies без изменений.
