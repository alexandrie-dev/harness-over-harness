// ЕДИНСТВЕННОЕ МЕСТО, откуда чат берёт рендер ответов Помощника. Отображение как в T3 (решение основателя 20.11):
// конвейер T3 (react-markdown + remark-*, ./chatMarkdown.tsx). Сам кусок разметки подгружается лениво,
// формулы katex — отдельным куском при первой формуле. Пока кусок грузится, показан простой текст ответа.
import { lazy, Suspense } from 'react';

// Действия ссылок (открыть, показать в папке, объяснить отказ) задаёт окно; от рендера они не зависят.
export { LinkContext, type LinkActions } from '../lib/markdown';

const T3Markdown = lazy(() => import('./chatMarkdown').then(m => ({ default: m.T3Markdown })));

export interface ChatMarkdownProps { readonly text: string; readonly streaming?: boolean; readonly cwd?: string }
export function ChatMarkdown({ text }: ChatMarkdownProps) {
  return <Suspense fallback={<div className="chat-markdown" data-markdown="safe" data-renderer="pending" style={{ whiteSpace: 'pre-wrap' }}>{text}</div>}><T3Markdown text={text} /></Suspense>;
}
