import { invoke } from '@tauri-apps/api/core';
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import './html-document.css';

interface Props {
  sessionId: string;
  path: string;
  title: string;
  autoOpen: boolean;
  file: (open: () => void) => ReactNode;
}

export function HtmlDocument({ sessionId, path, title, autoOpen, file }: Props) {
  const [url, setUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const card = useRef<HTMLSpanElement>(null);
  const request = useRef(0);
  const openAutomatically = useRef(true);

  useEffect(() => () => { request.current++; }, []);
  useLayoutEffect(() => {
    if (expanded) card.current?.showPopover();
  }, [expanded]);

  const load = useCallback(async () => {
    const current = ++request.current;
    setBusy(true);
    setError(null);
    try {
      const next = await invoke<string>('preview_html', { sessionId, path });
      if (!/^hoh-preview:\/\/localhost\/[0-9a-f]{48}$/.test(next)) throw new Error('Invalid preview address.');
      if (request.current === current) setUrl(next);
    } catch (failure) {
      if (request.current === current) {
        const detail = failure instanceof Error ? failure.message : failure;
        setError(typeof detail === 'string' ? detail : 'The HTML document could not be opened.');
      }
    } finally {
      if (request.current === current) setBusy(false);
    }
  }, [sessionId, path]);

  useEffect(() => {
    if (autoOpen && openAutomatically.current) void load();
  }, [autoOpen, load]);

  const open = () => {
    openAutomatically.current = false;
    if (!url && !busy) void load();
  };

  return (
    <span className="hoh-html-document">
      {file(open)}
      {!url && busy && <span className="hoh-html-loading" role="status">Loading HTML…</span>}
      {url && (
        <span
          ref={card}
          className="hoh-html-card"
          popover={expanded ? 'auto' : undefined}
          onToggle={(event) => {
            if (event.newState === 'closed') setExpanded(false);
          }}
        >
          <span className="hoh-html-bar">
            <span className="hoh-html-title" title={title}>{title}</span>
            <button type="button" className="hoh-html-action" disabled={busy} onClick={() => void load()}>Reload</button>
            {typeof HTMLElement.prototype.showPopover === 'function' && (
              <button type="button" className="hoh-html-action" aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>
                {expanded ? 'Shrink' : 'Expand'}
              </button>
            )}
            <button type="button" className="hoh-html-action" onClick={() => {
              openAutomatically.current = false;
              request.current++;
              setUrl(null);
              setBusy(false);
              setError(null);
              setExpanded(false);
            }}>Close</button>
          </span>
          <iframe
            title={`HTML preview: ${title}`}
            src={url}
            sandbox="allow-scripts"
            referrerPolicy="no-referrer"
            className="hoh-html-frame"
          />
        </span>
      )}
      {error && <span className="hoh-html-error" role="status">{error}</span>}
    </span>
  );
}
