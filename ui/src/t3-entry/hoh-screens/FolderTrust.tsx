import { useCallback, useEffect, useRef, useState } from 'react';
import { folderTrustView } from './folder-trust-view.mjs';

type Trust = { cwd: string; executor: string; trusted: boolean; changed: boolean; isolated?: boolean; running?: boolean; applied?: boolean };

/** The approval itself is provided by the native host, not by this page. */
export function FolderTrust({ threadId, fallbackCwd, isServerThread }: { threadId: string; fallbackCwd: string | null; isServerThread: boolean }) {
  const [state, setState] = useState<Trust | null>(null);
  const [busy, setBusy] = useState(false);
  const [changedNotice, setChangedNotice] = useState(false);
  const generation = useRef(0);
  const refresh = useCallback(async () => {
    const current = ++generation.current;
    const update = (value: Trust | null) => { if (current === generation.current) setState(value); };
    const engine = window.__HOH_TRUST_ENGINE__;
    if (!engine) return;
    try {
      let access = await engine.request('hoh/executor/access/get', isServerThread
        ? { executor: 'claude', sessionId: threadId }
        : { executor: 'claude' }) as { session?: { executor?: string | null; running?: string | null }; selected?: { executor?: string | null }; effective?: { flags?: string[] } };
      const executor = isServerThread ? access.session?.executor : access.selected?.executor;
      if (executor !== 'claude' && executor !== 'codex') { update(null); return; }
      if (executor === 'codex') {
        access = await engine.request('hoh/executor/access/get', isServerThread
          ? { executor, sessionId: threadId } : { executor }) as typeof access;
      }
      let cwd = fallbackCwd;
      if (isServerThread) try {
        const directory = await engine.request('session/working-directory/get', { sessionId: threadId }) as { cwd?: string };
        cwd = directory.cwd || cwd;
      } catch { /* Use the project until an established session is available. */ }
      if (!cwd) { update(null); return; }
      const result = await engine.request('hoh/executor/trust/get', { executor, cwd }) as Trust;
      const flags = access.effective?.flags ?? [];
      const sources = flags.indexOf('--setting-sources');
      const isolated = sources >= 0 && flags[sources + 1] === '';
      const running = !!access.session?.running;
      const selectedSources = sources >= 0 ? flags[sources + 1]?.split(',') ?? [] : [];
      const applied = running && (executor === 'codex' ? flags.includes('project-trust=trusted') : selectedSources.includes('project') && selectedSources.includes('local'));
      if (result.changed && current === generation.current) setChangedNotice(true);
      update({ ...result, trusted: isolated ? false : result.trusted, isolated, running, applied });
    } catch { update(null); }
  }, [threadId, fallbackCwd, isServerThread]);
  useEffect(() => {
    setState(null);
    setChangedNotice(false);
    void refresh();
    const timer = window.setInterval(() => void refresh(), 5000);
    window.addEventListener('focus', refresh);
    let stopped = false;
    let unsubscribe: (() => void) | undefined;
    void window.__HOH_TRUST_ENGINE__?.subscribe?.(event => {
      if (event.method === 'session.event' && event.params?.sessionId === threadId && event.params?.event?.type === 'plugin:external-harness/folder-trust-changed') {
        setChangedNotice(true);
        void refresh();
      }
    }).then(off => { if (stopped) off(); else unsubscribe = off; }).catch(() => {});
    return () => { stopped = true; unsubscribe?.(); generation.current++; window.clearInterval(timer); window.removeEventListener('focus', refresh); };
  }, [refresh]);
  if (!state) return null;
  const view = folderTrustView({ running: state.running, applied: state.applied, trusted: state.trusted, isolated: state.isolated, changed: changedNotice });
  return <span className="inline-flex shrink-0 items-center gap-1 text-xs text-muted-foreground">
    {view.text}
    {view.canTrust && <button type="button" disabled={busy} className="underline hover:text-foreground" onClick={async () => {
      const engine = window.__HOH_TRUST_ENGINE__;
      if (!engine) return;
      setBusy(true);
      try { await engine.request('hoh/executor/trust/set', { executor: state.executor, cwd: state.cwd, trust: true }); setChangedNotice(false); }
      catch { /* The native host rejected or could not confirm the request. */ }
      finally { setBusy(false); void refresh(); }
    }}>Доверять настройкам проекта в этой папке (Claude и Codex)…</button>}
  </span>;
}
