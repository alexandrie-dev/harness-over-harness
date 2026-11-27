import { Channel, invoke } from '@tauri-apps/api/core';
import { useEffect, useSyncExternalStore } from 'react';
import type { HomeEntry } from '../../hoh-host/threads';

export interface PagePlugin {
  id: string;
  name: string;
  version: string;
  digest: string;
  chatPanel?: { title: string; sidebarLabel: string };
  workspaceLabel?: string;
}

interface Catalog {
  directory: string;
  packages: PagePlugin[];
  rejected: { id: string; reason: string }[];
  prepared: PagePlugin | null;
}

export function externalUiError(error: unknown, fallback: string) {
  if (typeof error === 'string') return error;
  return error instanceof Error ? error.message : fallback;
}

let catalogState: { catalog: Catalog | null; error: string } = { catalog: null, error: '' };
const listeners = new Set<() => void>();
const readCatalog = () => catalogState;
function subscribeCatalog(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}
let catalogRequest: Promise<void> | undefined;
function publishCatalog(catalog: Catalog) {
  catalogState = { catalog, error: '' };
  for (const listener of listeners) listener();
}

export function refreshExternalUiCatalog() {
  if (catalogRequest) return catalogRequest;
  const request: Promise<void> = invoke<Catalog>('list_external_ui_plugins')
    .then(catalog => {
      if (catalogRequest === request) publishCatalog(catalog);
    })
    .catch(error => {
      if (catalogRequest !== request) return;
      catalogState = { ...catalogState, error: externalUiError(error, 'The plugin catalog could not be loaded.') };
      for (const listener of listeners) listener();
    })
    .finally(() => {
      if (catalogRequest === request) catalogRequest = undefined;
    });
  catalogRequest = request;
  return request;
}

export async function installExternalUiPlugin(prepared?: PagePlugin) {
  const installed = await invoke<{ package: PagePlugin; catalog: Catalog } | null>(
    'install_external_ui_plugin',
    { prepared: prepared ?? null },
  );
  if (!installed) return null;
  catalogRequest = undefined;
  publishCatalog(installed.catalog);
  return installed.package;
}

export function useExternalUiCatalog() {
  const desktop = Boolean(globalThis.window?.__TAURI_INTERNALS__);
  const state = useSyncExternalStore(subscribeCatalog, readCatalog, readCatalog);
  useEffect(() => {
    if (desktop && !readCatalog().catalog && !readCatalog().error) void refreshExternalUiCatalog();
  }, [desktop]);
  return { desktop, ...state };
}

interface QueueHost {
  entries(): HomeEntry[];
  projectTitle(id: string): string;
  subscribe(listener: () => void): () => void;
}

type QueueState = 'question' | 'approval' | 'review';

export function reviewQueue(host: Pick<QueueHost, 'entries' | 'projectTitle'>) {
  return host.entries()
    .filter(entry => entry.state === 'waiting' && (entry.request || entry.canComplete))
    .sort((left, right) => left.startedAt - right.startedAt || left.id.localeCompare(right.id))
    .slice(0, 512)
    .map(entry => ({
      threadId: entry.id,
      title: Array.from(entry.title).slice(0, 240).join(''),
      project: Array.from(host.projectTitle(entry.projectId)).slice(0, 120).join(''),
      startedAt: entry.startedAt,
      state: (entry.request?.kind ?? 'review') satisfies QueueState,
    }));
}

type PanelEvent = { type: 'closed' } | { type: 'openChat'; threadId: string };

let nextPanelId = 0;
let activePanelId: number | undefined;
let disposeActivePanel: (() => void) | undefined;
let opening = false;

export async function openExternalUiPanel(item: PagePlugin, openChat: (id: string) => void) {
  if (opening) return;
  const host = (globalThis as { __HOH_HOME__?: QueueHost }).__HOH_HOME__;
  if (!host) throw new Error('The current profile is still loading.');
  opening = true;
  const panelId = ++nextPanelId;
  const channel = new Channel<PanelEvent>();
  let closed = false;
  channel.onmessage = (event) => {
    if (event.type === 'closed') {
      closed = true;
      if (activePanelId === panelId) {
        disposeActivePanel?.();
        disposeActivePanel = undefined;
        activePanelId = undefined;
      }
      return;
    }
    if (activePanelId === panelId && reviewQueue(host).some(item => item.threadId === event.threadId)) {
      openChat(event.threadId);
    }
  };

  try {
    await invoke('open_external_ui_plugin', { package: item, panelId, onEvent: channel });
    if (closed) return;
    disposeActivePanel?.();
    activePanelId = panelId;
    let previous = '';
    let revision = 0;
    const publish = () => {
      const queue = reviewQueue(host);
      const serialized = JSON.stringify(queue);
      if (serialized === previous) return;
      previous = serialized;
      void invoke('update_external_ui_panel', { panelId, revision: ++revision, queue }).catch(error => {
        if (activePanelId === panelId) console.error('Could not update the external plugin queue', error);
      });
    };
    disposeActivePanel = host.subscribe(publish);
    publish();
  } finally {
    opening = false;
  }
}
