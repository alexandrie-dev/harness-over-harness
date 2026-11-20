import { createContext, memo, type ReactNode, useContext, useMemo } from 'react';
import { Lexer, type Token } from 'marked';
import { parseFileUrlHref, splitFilePathPosition } from '@t3tools/shared/fileLinks';

type TableCell = { tokens: Token[] };
type ListItem = { task?: boolean; checked?: boolean; tokens: Token[] };
type WithInline = { tokens?: Token[]; text?: string; raw?: string };
/** Host of an http(s) address, or '' for anything else (javascript:, data:, file:, relative…). */
function webHost(href: string): string {
  try { const url = new URL(href); return url.protocol === 'https:' || url.protocol === 'http:' ? url.host : ''; } catch { return ''; }
}
/** What a link does is decided by the window around the answer: open in the system, show in the folder, or explain a refusal. */
export interface LinkActions {
  /** Working folder of the chat, as the engine reports it (absolute); '' when unknown. */
  folder: string;
  open(kind: 'url' | 'file' | 'reveal', target: string): void;
  refuse(target: string): void;
  blocked(): void;
}
export const LinkContext = createContext<LinkActions | null>(null);

/** Where a file link points. The window hides paths outside the chat folder behind the word «файл», so that word, `..` and
 *  absolute paths outside the folder all mean «outside»; the host checks again and is the only judge of opening. */
export function fileTarget(href: string, folder: string): { path: string; line?: number; outside: boolean } | null {
  const target = /^file:/i.test(href) ? parseFileUrlHref(href)?.path : href;
  const { path, line } = splitFilePathPosition(target ?? '');
  const name = path.split(/[\\/]/).filter(Boolean).at(-1);
  if (!name || /\s/.test(path)) return null;
  const root = folder.replaceAll('\\', '/').replace(/\/$/, '');
  const absolute = path.startsWith('/') || /^[A-Za-z]:[\\/]/.test(path);
  const outside = name === 'файл' || path.split(/[\\/]/).includes('..') || (absolute && !(root && path.replaceAll('\\', '/').startsWith(root + '/')));
  return { path, line, outside };
}

function FileChip({ href }: { href: string }) {
  const actions = useContext(LinkContext);
  const file = fileTarget(href, actions?.folder ?? '');
  if (!file) return <InertLink raw={href} />;
  const name = file.path.split(/[\\/]/).filter(Boolean).at(-1) ?? file.path;
  const label = `${name}${file.line !== undefined ? `:${file.line}` : ''}`;
  if (file.outside) return <button type="button" className="chat-link chat-link-outside" data-markdown-link="file-outside" title="Файл вне рабочей папки" onClick={() => actions?.refuse(href)}>{label}<span className="chat-md-chip-note">вне папки</span></button>;
  return <span className="chat-link-group" data-markdown-link-group>
    <button type="button" className="chat-link" data-markdown-link="file" title={file.path} onClick={() => actions?.open('file', file.path)}>{label}</button>
    <button type="button" className="chat-link-reveal" data-markdown-reveal aria-label={`Показать в папке: ${name}`} title="Показать в папке" onClick={() => actions?.open('reveal', file.path)}>в папке</button>
  </span>;
}

function UrlChip({ href, host, children }: { href: string; host: string; children: ReactNode }) {
  const actions = useContext(LinkContext);
  return <button type="button" className="chat-link" data-markdown-link="url" title={href} onClick={() => actions?.open('url', href)}>{children}<span className="chat-md-chip-note">{host}</span></button>;
}

/** Links are buttons, never anchors: the window itself never navigates; opening goes through the native host. */
function linkChip(link: Token, child: ReactNode[], key: number): ReactNode {
  const token = link as Token & { href: string; text: string; raw: string };
  const host = webHost(token.href);
  if (host) return <UrlChip key={key} href={token.href} host={host}>{child.length ? child : token.text}</UrlChip>;
  if (!/^[a-z][a-z0-9+.-]*:/i.test(token.href) || /^file:/i.test(token.href) || /^[^:/\\\s]+\.[a-z0-9]+:\d+(:\d+)?$/i.test(token.href)) {
    if (fileTarget(token.href, '')) return <FileChip key={key} href={token.href} />;
  }
  return <InertLink key={key} raw={token.raw} />;
}
/** A link that cannot be opened (javascript:, data:, …) stays literal text, and a click says why nothing happens. */
function InertLink({ raw }: { raw: string }) {
  const actions = useContext(LinkContext);
  return <button type="button" className="chat-link chat-link-outside" data-markdown-link="inert" onClick={() => actions?.blocked()}>{raw}</button>;
}
function inline(tokens: Token[] = []): ReactNode[] {
  return tokens.map((token, index) => {
    const child = inline((token as WithInline).tokens);
    switch (token.type) {
      case 'strong': return <strong key={index}>{child}</strong>;
      case 'em': return <em key={index}>{child}</em>;
      case 'del': return <del key={index}>{child}</del>;
      case 'codespan': return <code key={index}>{token.text}</code>;
      case 'br': return <br key={index} />;
      // Remote images never load. Links are buttons that ask the native host; nothing here navigates.
      // Raw HTML, SVG, math, handlers and script URLs never become DOM elements.
      case 'image': return webHost((token as Token & { href: string }).href) ? <span key={index} className="chat-md-chip" data-markdown-image="placeholder" title={webHost(token.href)}>Картинка{token.text ? `: ${token.text}` : ''}</span> : <span key={index}>{token.raw}</span>;
      case 'link': return linkChip(token, child, index);
      case 'html': return <span key={index}>{token.raw}</span>;
      case 'escape':
      case 'text': return <span key={index}>{child.length ? child : token.text}</span>;
      default: return <span key={index}>{token.raw}</span>;
    }
  });
}
function blocks(tokens: Token[] = []): ReactNode[] {
  return tokens.map((token, index) => <MarkdownBlock key={index} token={token} />);
}
const MarkdownBlock = memo(function MarkdownBlock({ token }: { token: Token }) {
  switch (token.type) {
    case 'space': return null;
    case 'heading': {
      const children = inline(token.tokens);
      // Heading levels are lexer data, never a model-controlled element name.
      switch (token.depth) {
        case 1: return <h2>{children}</h2>;
        case 2: return <h3>{children}</h3>;
        case 3: return <h4>{children}</h4>;
        default: return <h5>{children}</h5>;
      }
    }
    case 'paragraph': return <p>{inline(token.tokens)}</p>;
    case 'text': { const children = inline((token as WithInline).tokens); return <p>{children.length ? children : token.text}</p>; }
    case 'code': return <pre><code>{token.text}</code></pre>;
    case 'blockquote': return <blockquote>{blocks(token.tokens)}</blockquote>;
    case 'hr': return <hr />;
    case 'list': {
      const children = token.items.map((item: ListItem, index: number) => <li key={index}>{item.task && <span aria-label={item.checked ? 'Выполнено' : 'Не выполнено'}>{item.checked ? '☑ ' : '☐ '}</span>}{blocks(item.tokens)}</li>);
      return token.ordered ? <ol start={typeof token.start === 'number' ? token.start : undefined}>{children}</ol> : <ul>{children}</ul>;
    }
    case 'table': return <table><thead><tr>{token.header.map((cell: TableCell, i: number) => <th key={i}>{inline(cell.tokens)}</th>)}</tr></thead><tbody>{token.rows.map((row: TableCell[], i: number) => <tr key={i}>{row.map((cell, j) => <td key={j}>{inline(cell.tokens)}</td>)}</tr>)}</tbody></table>;
    case 'html': return <p data-markdown-html="literal">{token.raw}</p>;
    default: return <p>{token.raw}</p>;
  }
}, (previous, next) => previous.token.type === next.token.type && previous.token.raw === next.token.raw);

export const Markdown = memo(function Markdown({ text }: { text: string }) {
  const tokens = useMemo(() => Lexer.lex(text, { gfm: true }), [text]);
  return <div className="chat-markdown" data-markdown="safe">{blocks(tokens)}</div>;
});
