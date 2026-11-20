import { createContext, type ReactNode, useContext } from 'react';
import { parseFileUrlHref, splitFilePathPosition } from '@t3tools/shared/fileLinks';

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
export function fileTarget(rawHref: string, folder: string): { path: string; line?: number; outside: boolean } | null {
  // The T3 pipeline percent-encodes link destinations; the path is read decoded.
  let href = rawHref;
  try { href = decodeURI(rawHref); } catch { /* keep as written */ }
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

/** A link that cannot be opened (javascript:, data:, …) stays literal text, and a click says why nothing happens. */
function InertLink({ raw }: { raw: string }) {
  const actions = useContext(LinkContext);
  return <button type="button" className="chat-link chat-link-outside" data-markdown-link="inert" onClick={() => actions?.blocked()}>{raw}</button>;
}

/** A link of an answer. Links are buttons, never anchors: the window itself never navigates; opening goes through the native host.
 *  `raw` is the authored source, shown as it is when the link can not be opened. */
export function ChatLink({ href, raw, children }: { href: string; raw: string; children: ReactNode }) {
  const host = webHost(href);
  if (host) return <UrlChip href={href} host={host}>{children}</UrlChip>;
  if (!/^[a-z][a-z0-9+.-]*:/i.test(href) || /^file:/i.test(href) || /^[^:/\\\s]+\.[a-z0-9]+:\d+(:\d+)?$/i.test(href)) {
    if (fileTarget(href, '')) return <FileChip href={href} />;
  }
  return <InertLink raw={raw} />;
}

/** Remote images never load here: an address shows a chip with the description, anything else stays literal. */
export function ChatImage({ src, alt, raw }: { src: string; alt: string; raw: string }) {
  const host = webHost(src);
  return host ? <span className="chat-md-chip" data-markdown-image="placeholder" title={host}>Картинка{alt ? `: ${alt}` : ''}</span> : <span>{raw}</span>;
}
