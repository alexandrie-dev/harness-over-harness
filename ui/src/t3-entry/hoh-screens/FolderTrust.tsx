import { useCallback, useEffect, useRef, useState } from 'react';

type Trust = { cwd: string; trusted: boolean; changed: boolean; isolated?: boolean };

/** The approval itself is provided by the native host, not by this page. */
export function FolderTrust({ threadId, fallbackCwd, isServerThread }: { threadId: string; fallbackCwd: string | null; isServerThread: boolean }) {
  const [state, setState] = useState<Trust | null>(null);
  const [busy, setBusy] = useState(false);
  const generation = useRef(0);
  const refresh = useCallback(async () => {
    const current = ++generation.current;
    const update = (value: Trust | null) => { if (current === generation.current) setState(value); };
    const engine = window.__HOH_TRUST_ENGINE__;
    if (!engine) return;
    try {
      const access = await engine.request('hoh/executor/access/get', isServerThread
        ? { executor: 'claude', sessionId: threadId }
        : { executor: 'claude' }) as { session?: { executor?: string | null }; selected?: { executor?: string | null }; effective?: { flags?: string[] } };
      const executor = isServerThread ? access.session?.executor : access.selected?.executor;
      if (executor !== 'claude') { update(null); return; }
      let cwd = fallbackCwd;
      if (isServerThread) try {
        const directory = await engine.request('session/working-directory/get', { sessionId: threadId }) as { cwd?: string };
        cwd = directory.cwd || cwd;
      } catch { /* Use the project until an established session is available. */ }
      if (!cwd) { update(null); return; }
      const result = await engine.request('hoh/executor/trust/get', { executor: 'claude', cwd }) as Trust;
      const flags = access.effective?.flags ?? [];
      const sources = flags.indexOf('--setting-sources');
      const isolated = sources >= 0 && flags[sources + 1] === '';
      update(isolated ? { ...result, trusted: false, isolated } : result);
    } catch { update(null); }
  }, [threadId, fallbackCwd, isServerThread]);
  useEffect(() => {
    setState(null);
    void refresh();
    const timer = window.setInterval(() => void refresh(), 5000);
    window.addEventListener('focus', refresh);
    return () => { generation.current++; window.clearInterval(timer); window.removeEventListener('focus', refresh); };
  }, [refresh]);
  if (!state) return null;
  return <span className="inline-flex shrink-0 items-center gap-1 text-xs text-muted-foreground">
    {state.trusted ? 'Настройки проекта применяются' : 'Настройки проекта не применяются'}
    {!state.trusted && !state.isolated && <button type="button" disabled={busy} className="underline hover:text-foreground" onClick={async () => {
      const engine = window.__HOH_TRUST_ENGINE__;
      if (!engine) return;
      setBusy(true);
      try { await engine.request('hoh/executor/trust/set', { executor: 'claude', cwd: state.cwd, trust: true }); }
      catch { /* The native host rejected or could not confirm the request. */ }
      finally { setBusy(false); void refresh(); }
    }}>Доверять…</button>}
  </span>;
}
