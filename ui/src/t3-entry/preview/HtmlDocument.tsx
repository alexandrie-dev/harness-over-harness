import { invoke } from '@tauri-apps/api/core';
import { useEffect, useRef, useState, type ReactNode } from 'react';
import './html-document.css';

interface Props {
  sessionId: string;
  path: string;
  title: string;
  file: ReactNode;
}

export function HtmlDocument({ sessionId, path, title, file }: Props) {
  const [url, setUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const request = useRef(0);

  useEffect(() => () => { request.current++; }, []);
  useEffect(() => {
    if (!expanded) return;
    const close = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setExpanded(false);
    };
    window.addEventListener('keydown', close);
    return () => window.removeEventListener('keydown', close);
  }, [expanded]);

  const load = async () => {
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
  };

  return (
    <span className="hoh-html-document">
      {file}
      {!url && (
        <button type="button" className="hoh-html-action" disabled={busy} onClick={() => void load()}>
          {busy ? 'Loading HTML…' : 'Preview HTML'}
        </button>
      )}
      {url && (
        <span className="hoh-html-card" data-expanded={expanded || undefined}>
          <span className="hoh-html-bar">
            <span className="hoh-html-title" title={title}>{title}</span>
            <button type="button" className="hoh-html-action" disabled={busy} onClick={() => void load()}>Reload</button>
            <button type="button" className="hoh-html-action" aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>
              {expanded ? 'Shrink' : 'Expand'}
            </button>
            <button type="button" className="hoh-html-action" onClick={() => {
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
