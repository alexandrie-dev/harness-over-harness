// ЕДИНСТВЕННЫЙ ПЕРЕКЛЮЧАТЕЛЬ рендера ответов Помощника. Чат берёт разметку отсюда и больше ниоткуда.
//
// Сейчас: рендер на marked (src/lib/markdown.tsx), потому что отложенные файлы T3 (manifest.json, deferred)
// требуют пакетов, которых нет в lock окна (список — ui/T3-MAP.md, раздел «Зависимости для установки»).
//
// После установки пакетов через supply-gate (одно место, три действия подряд):
//   1. в src/t3/manifest.json поставить "deferredEnabled": true и выполнить python3 -I ui/scripts/t3-sync.py
//      (tsc перестанет пропускать отложенные файлы);
//   2. в src/styles/tokens.css убрать строку `@source not "../t3";` (иначе Tailwind не увидит классы T3);
//   3. здесь заменить тело на строки, оставленные в комментарии ниже.
//
//   import T3ChatMarkdown from './web/components/ChatMarkdown';
//   export function ChatMarkdown({ text, streaming, cwd }: ChatMarkdownProps) {
//     return <T3ChatMarkdown text={text} cwd={cwd} isStreaming={streaming} lineBreaks />;
//   }
import { Markdown } from '../lib/markdown';

// Действия ссылок (открыть, показать в папке, объяснить отказ) задаёт окно; от рендера они не зависят.
export { LinkContext, type LinkActions } from '../lib/markdown';

export interface ChatMarkdownProps { readonly text: string; readonly streaming?: boolean; readonly cwd?: string }
export function ChatMarkdown({ text }: ChatMarkdownProps) { return <Markdown text={text} />; }
