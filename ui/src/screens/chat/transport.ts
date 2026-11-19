import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { MethodMap, Notification } from '../../protocol';

export type ChatMethod = Extract<keyof MethodMap,
  'hoh/employee/open' | 'hoh/security/summary' | 'session/resume' | 'session/events' |
  'session/working-directory/get' | 'session/prompt' | 'session/cancel'>;
export type EngineStatus = { state: 'starting' | 'ready' | 'unavailable' | 'restarting' | 'stopped' };
export interface ChatTransport {
  request<M extends ChatMethod>(method: M, params: MethodMap[M]['params']): Promise<MethodMap[M]['result']>;
  subscribe(listener: (event: Notification) => void): Promise<() => void>;
  subscribeStatus(listener: (status: EngineStatus) => void): Promise<() => void>;
}
declare global {
  interface Window { __HOH_CHAT_TEST_TRANSPORT__?: ChatTransport; }
}

export function chatTransport(): ChatTransport {
  // Browser-only adapter, installed by the headless harness; never used in Tauri.
  if (!isTauri() && window.__HOH_CHAT_TEST_TRANSPORT__) return window.__HOH_CHAT_TEST_TRANSPORT__;
  // Native bridge delivers protocol frames and process lifecycle separately.
  return {
    request<M extends ChatMethod>(method: M, params: MethodMap[M]['params']) {
      return invoke<MethodMap[M]['result']>('engine_request', { method, params });
    },
    subscribe: listener => listen<Notification>('engine-notification', event => listener(event.payload)),
    subscribeStatus: listener => listen<EngineStatus>('engine-status', event => listener(event.payload)),
  };
}
