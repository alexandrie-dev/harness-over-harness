import { Channel, invoke, isTauri } from '@tauri-apps/api/core';
import { createStore } from 'zustand/vanilla';

type UpdateProgress = { kind: 'downloading' | 'installing'; version: string };
type UpdateResult =
  | { kind: 'unsupportedInstall' | 'noRelease' | 'upToDate' }
  | { kind: 'available' | 'noCompatiblePackage' | 'restarting'; version: string };

export type UpdateState = UpdateResult | UpdateProgress
  | { kind: 'idle' | 'checking' }
  | { kind: 'error'; message: string };

export const desktopUpdate = createStore<UpdateState>(() => ({ kind: 'idle' }));

export function updateInProgress(state: UpdateState): boolean {
  return state.kind === 'checking' || state.kind === 'downloading'
    || state.kind === 'installing' || state.kind === 'restarting';
}

async function requestUpdate(install: boolean): Promise<void> {
  const current = desktopUpdate.getState();
  if (updateInProgress(current) || current.kind === 'unsupportedInstall') return;
  desktopUpdate.setState({ kind: 'checking' }, true);
  try {
    const result = install
      ? await invoke<UpdateResult>('auto_update', {
          onProgress: new Channel<UpdateProgress>(progress => desktopUpdate.setState(progress, true)),
        })
      : await invoke<UpdateResult>('check_for_update');
    desktopUpdate.setState(result, true);
  } catch (error) {
    const message = typeof error === 'string' ? error : 'HoH could not check or install updates.';
    desktopUpdate.setState({ kind: 'error', message }, true);
  }
}

export function installUpdate(): Promise<void> {
  return requestUpdate(true);
}

export function startUpdateChecks(): void {
  if (!isTauri()) return;
  void requestUpdate(false);
  window.setInterval(() => void requestUpdate(false), 6 * 60 * 60 * 1000);
}
