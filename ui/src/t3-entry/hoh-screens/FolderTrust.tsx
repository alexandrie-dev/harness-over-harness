import { useCallback, useEffect, useRef, useState } from 'react';
import { folderTrustView, isKnownAbsoluteFolder } from './folder-trust-view.mjs';
import type { HohEngineLike } from '../host';
import { readAppliedTrust, writeAppliedTrust } from './folder-trust-record.mjs';

declare global { interface Window { __HOH_TRUST_ENGINE__?: HohEngineLike } }

type Trust = { cwd: string; executor: string; trusted: boolean; changed: boolean; isolated?: boolean; running?: boolean; applied?: boolean };

/** The approval itself is provided by the native host, not by this page. */
export function FolderTrust({ threadId, fallbackCwd, isServerThread }: { threadId: string; fallbackCwd: string | null; isServerThread: boolean }) {
  const [state, setState] = useState<Trust | null>(null);
  const [busy, setBusy] = useState(false);
  const [folderUnknown, setFolderUnknown] = useState(false);
  const [changedNotice, setChangedNotice] = useState(false);
  const generation = useRef(0);
  const refresh = useCallback(async () => {
    const current = ++generation.current;
    const update = (value: Trust | null) => { if (current === generation.current) { setState(value); if (value) setFolderUnknown(false); } };
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
      // A placeholder such as ~/Documents/HoH is not an authoritative engine folder.
      if (!isKnownAbsoluteFolder(cwd)) { update(null); if (current === generation.current) setFolderUnknown(true); return; }
      const result = await engine.request('hoh/executor/trust/get', { executor, cwd }) as Trust;
      const flags = access.effective?.flags ?? [];
      const sources = flags.indexOf('--setting-sources');
      const isolated = sources >= 0 && flags[sources + 1] === '';
      const running = !!access.session?.running;
      const selectedSources = sources >= 0 ? flags[sources + 1]?.split(',') ?? [] : [];
      const liveApplied = running && (executor === 'codex' ? flags.includes('project-trust=trusted') : selectedSources.includes('user') && selectedSources.includes('project') && selectedSources.includes('local'));
      let applied = liveApplied;
      if (isServerThread) try {
        const saved = readAppliedTrust(window.localStorage, threadId);
        if (!result.trusted || result.changed || isolated) {
          if (saved) writeAppliedTrust(window.localStorage, threadId, null);
        } else if (liveApplied) {
          if (saved?.cwd !== cwd || saved.executor !== executor) writeAppliedTrust(window.localStorage, threadId, { cwd, executor });
        } else if (!running && saved?.cwd === cwd && saved.executor === executor) {
          applied = true;
        }
      } catch { /* Storage is optional; never claim a prior launch without evidence. */ }
      if (result.changed && current === generation.current) setChangedNotice(true);
      update({ ...result, trusted: isolated ? false : result.trusted, isolated, running, applied });
    } catch { update(null); }
  }, [threadId, fallbackCwd, isServerThread]);
  useEffect(() => {
    setState(null);
    setFolderUnknown(false);
    setChangedNotice(false);
    void refresh();
    const timer = window.setInterval(() => void refresh(), 5000);
    window.addEventListener('focus', refresh);
    let stopped = false;
    let unsubscribe: (() => void) | undefined;
    void window.__HOH_TRUST_ENGINE__?.subscribe?.(event => {
      if (event.method === 'session.event' && event.params?.sessionId === threadId) {
        if (event.params?.event?.type === 'plugin:external-harness/folder-trust-changed') { setChangedNotice(true); void refresh(); }
        if (event.params?.event?.type === 'plugin:external-harness/launch') void refresh();
      }
    }).then(off => { if (stopped) off(); else unsubscribe = off; }).catch(() => {});
    return () => { stopped = true; unsubscribe?.(); generation.current++; window.clearInterval(timer); window.removeEventListener('focus', refresh); };
  }, [refresh]);
  if (!state) return folderUnknown ? <span data-hoh-folder-trust className="text-xs text-muted-foreground">Project settings in this folder: unknown</span> : null;
  const view = folderTrustView({ running: state.running ?? false, applied: state.applied ?? false, trusted: state.trusted, isolated: state.isolated ?? false, changed: changedNotice });
  return <span data-hoh-folder-trust className="inline-flex items-center gap-1 text-xs text-muted-foreground">
    {view.text}
    {view.canTrust && <button type="button" disabled={busy} className="underline hover:text-foreground" onClick={async () => {
      const engine = window.__HOH_TRUST_ENGINE__;
      if (!engine) return;
      setBusy(true);
      try { if (isServerThread) { try { writeAppliedTrust(window.localStorage, threadId, null); } catch { /* Storage is optional. */ } } await engine.request('hoh/executor/trust/set', { executor: state.executor, cwd: state.cwd, trust: true }); setChangedNotice(false); }
      catch { /* The native host rejected or could not confirm the request. */ }
      finally { setBusy(false); void refresh(); }
    }}>Trust project settings in this folder (Claude and Codex)…</button>}
  </span>;
}
