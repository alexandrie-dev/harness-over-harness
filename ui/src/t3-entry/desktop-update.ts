import { Channel, invoke, isTauri } from '@tauri-apps/api/core';
import { createStore } from 'zustand/vanilla';

type UpdateProgress = { kind: 'downloading' | 'installing'; version: string };
type UpdateResult =
  | { kind: 'unsupportedInstall' | 'noRelease' | 'upToDate' | 'rolledBack' }
  | { kind: 'available' | 'noCompatiblePackage' | 'restarting'; version: string };

export type UpdateState = UpdateResult | UpdateProgress
  | { kind: 'idle' | 'checking' }
  | { kind: 'error'; message: string };

export const desktopUpdate = createStore<UpdateState>(() => ({ kind: 'idle' }));

export function updateInProgress(state: UpdateState): boolean {
  return state.kind === 'checking' || state.kind === 'downloading'
    || state.kind === 'installing' || state.kind === 'restarting';
}

async function requestUpdate(install: boolean): Promise<UpdateState | undefined> {
  const current = desktopUpdate.getState();
  if (updateInProgress(current) || current.kind === 'unsupportedInstall') return;
  if (install && current.kind !== 'available') return;
  desktopUpdate.setState({ kind: 'checking' }, true);
  try {
    const result = install && current.kind === 'available'
      ? await invoke<UpdateResult>('auto_update', {
          expectedVersion: current.version,
          onProgress: new Channel<UpdateProgress>(progress => desktopUpdate.setState(progress, true)),
        })
      : await invoke<UpdateResult>('check_for_update');
    desktopUpdate.setState(result, true);
    return result;
  } catch (error) {
    const message = typeof error === 'string' ? error : 'HoH could not check or install updates.';
    const result = { kind: 'error', message } as const;
    desktopUpdate.setState(result, true);
    return result;
  }
}

export function installUpdate() {
  return requestUpdate(true);
}

export function checkForUpdate() {
  return requestUpdate(false);
}

export function startUpdateChecks(): void {
  if (!isTauri()) return;
  void checkForUpdate();
  window.setInterval(() => void checkForUpdate(), 6 * 60 * 60 * 1000);
}
