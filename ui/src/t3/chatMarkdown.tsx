// Рендер ответов Помощника на конвейере T3: react-markdown + remark-gfm + remark-math + разбор T3
// (shared/markdownPipeline.ts: списки, GitHub-врезки, переводы строк, пути Windows, директивы Codex).
// Отличия от ChatMarkdown T3 (записаны в T3-MAP.md): сырой HTML не разбирается, а остаётся видимым текстом;
// ссылки и картинки не открываются и не грузятся сами (кнопки через хост, см. lib/markdown.tsx);
// формулы рисует katex, подгружаемый только при первой формуле; подсветки кода нет (@pierre/diffs не поставлен).
import { Children, isValidElement, memo, useEffect, useMemo, useState, type ComponentProps, type ReactNode } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkMath from 'remark-math';
import { CHAT_MARKDOWN_REMARK_PLUGINS_WITH_BREAKS } from '@t3tools/shared/markdownPipeline';
import { ChatImage, ChatLink } from '../lib/markdown';
import { createIncrementalMarkdownPlugin } from './web/markdown-incremental';
import { splitMarkdownBlocks } from './web/components/chat/markdownStreamBlocks';

type MdNode = { type: string; value?: string; children?: MdNode[] };

/** Raw HTML is never parsed: it stays on the screen as the text the model wrote. */
function remarkHtmlAsText() {
  const walk = (node: MdNode) => {
    if (!node.children) return;
    node.children = node.children.map(child => {
      if (child.type === 'html') {
        const text: MdNode = { type: 'text', value: child.value ?? '' };
        return node.type === 'root' || node.type === 'blockquote' || node.type === 'listItem' ? { type: 'paragraph', children: [text] } : text;
      }
      walk(child);
      return child;
    });
  };
  return (tree: MdNode) => walk(tree);
}

const REMARK_PLUGINS = [...CHAT_MARKDOWN_REMARK_PLUGINS_WITH_BREAKS, [remarkMath, { singleDollarTextMath: true }], remarkHtmlAsText];

type Katex = { renderToString(tex: string, options?: Record<string, unknown>): string };
let katexPromise: Promise<Katex> | undefined;
const formulaCache = new Map<string, string>();
/** katex и его стили приходят отдельным куском при первой формуле. Вывод — MathML: у него нет встроенных style="…", которые запрещает CSP окна (style-src 'self'). */
function loadKatex(): Promise<Katex> {
  katexPromise ??= Promise.all([import('katex'), import('katex/dist/katex.min.css')]).then(([module]) => module.default as Katex);
  return katexPromise;
}

function Formula({ tex, display }: { tex: string; display: boolean }) {
  const key = (display ? 'display:' : 'inline:') + tex;
  const [rendered, setRendered] = useState<{ key: string; html: string } | null>(null);
  const html = formulaCache.get(key) ?? (rendered?.key === key ? rendered.html : null);
  useEffect(() => {
    if (html !== null) return;
    let live = true;
    void loadKatex().then(katex => {
      if (!live) return;
      try {
        const result = formulaCache.get(key) ?? katex.renderToString(tex, { displayMode: display, throwOnError: false, trust: false, output: 'mathml' });
        if (tex.length <= 1024 && result.length <= 8192 && !formulaCache.has(key)) {
          if (formulaCache.size >= 128) formulaCache.delete(formulaCache.keys().next().value!);
          formulaCache.set(key, result);
        }
        setRendered({ key, html: result });
      } catch { /* show the original source */ }
    }).catch(() => undefined);
    return () => { live = false; };
  }, [tex, display, key, html]);
  // Until katex is there (or when it can not draw the formula) the authored source is shown.
  if (html === null) {
    const source = display ? '$$' + tex + '$$' : '$' + tex + '$';
    return display ? <div className="chat-math chat-math-pending" data-math="pending">{source}</div> : <span className="chat-math-pending" data-math="pending">{source}</span>;
  }
  return display
    ? <div className="chat-math" data-math="display" dangerouslySetInnerHTML={{ __html: html }} />
    : <span className="chat-math" data-math="inline" dangerouslySetInnerHTML={{ __html: html }} />;
}

const textOf = (node: ReactNode): string => Children.toArray(node).map(child => typeof child === 'string' ? child : isValidElement<{ children?: ReactNode }>(child) ? textOf(child.props.children) : '').join('');
const isMath = (className?: string) => /\blanguage-math\b/.test(className ?? '');

const MarkdownBlock = memo(function MarkdownBlock({ text, streaming }: { text: string; streaming: boolean }) {
  const remarkPlugins = useMemo(() => streaming ? [...REMARK_PLUGINS, createIncrementalMarkdownPlugin()] : REMARK_PLUGINS, [streaming]);
  const components = useMemo<NonNullable<ComponentProps<typeof ReactMarkdown>['components']>>(() => {
    const raw = (node: { position?: { start: { offset?: number }; end: { offset?: number } } } | undefined) => {
      const start = node?.position?.start.offset; const end = node?.position?.end.offset;
      return start === undefined || end === undefined ? '' : text.slice(start, end);
    };
    return {
      a: ({ node, href, children }) => <ChatLink href={href ?? ''} raw={raw(node)}>{children}</ChatLink>,
      img: ({ node, src, alt }) => <ChatImage src={typeof src === 'string' ? src : ''} alt={alt ?? ''} raw={raw(node)} />,
      code: ({ className, children }) => isMath(className) && /math-inline/.test(className ?? '')
        ? <Formula tex={textOf(children)} display={false} />
        : <code className={className}>{children}</code>,
      pre: ({ node, children }) => {
        const code = node?.children?.[0] as { properties?: { className?: string[] } } | undefined;
        const classes = (code?.properties?.className ?? []).join(' ');
        return isMath(classes) ? <Formula tex={textOf(children)} display /> : <pre>{children}</pre>;
      },
      table: ({ children }) => <div className="chat-table-scroll"><table>{children}</table></div>,
      input: ({ checked }) => <span aria-label={checked ? 'Выполнено' : 'Не выполнено'}>{checked ? '☑ ' : '☐ '}</span>,
    };
  }, [text]);
  return <ReactMarkdown remarkPlugins={remarkPlugins as never} components={components} urlTransform={url => url}>{text}</ReactMarkdown>;
});

const MAX_LIVE_BLOCK = 4_096;
export const T3Markdown = memo(function T3Markdown({ text, streaming = false }: { text: string; streaming?: boolean }) {
  const split = streaming ? splitMarkdownBlocks(text) : null;
  const blocks = split ?? [text];
  return <div className="chat-markdown" data-markdown="safe" data-renderer="t3">
    {blocks.map((block, index) => {
      const live = streaming && index === blocks.length - 1;
      // Document-wide syntax or a single long block can still require a full parse.
      // Keep that live text responsive and render its final Markdown on settlement.
      return live && block.length >= MAX_LIVE_BLOCK
        ? <p key={index} style={{ whiteSpace: 'pre-wrap' }}>{block}</p>
        : <MarkdownBlock key={index} text={block} streaming={live} />;
    })}
  </div>;
});
