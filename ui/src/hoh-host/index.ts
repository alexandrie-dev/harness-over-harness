// The HoH host for T3's web client: one call before the T3 bundle starts.
//
//   import { installHohHost } from '../hoh-host';
//   installHohHost();             // Tauri engine, address http://hoh.local
//   await import('./t3-main');    // T3's own entry, unchanged, built with VITE_HTTP_URL=http://hoh.local
//
// T3 then opens `ws://hoh.local/ws?...` through `globalThis.WebSocket` and reads `/api/auth/session`
// and `/.well-known/t3/environment` through `globalThis.fetch`; both are answered here, in memory.
import type { HohEngine } from './engine.ts';
import type { EnvironmentPlatform } from './config.ts';
import { HohHost } from './host.ts';
import { pluginInventory } from './plugin-inventory.ts';
import { installHohFetch, installHohXhr } from './http.ts';
import { installHohWebSocket, socketMatcher } from './socket.ts';
import { WireServer, type WireServerOptions } from './wire.ts';
import type { CreateHohLocalSettings } from '../t3-entry/local-settings.ts';

export { HohHost } from './host.ts';
export { WireServer, RpcFailure } from './wire.ts';
export { HohWebSocket, installHohWebSocket, socketMatcher } from './socket.ts';
export { installHohFetch, installHohXhr, hohHttpResponse } from './http.ts';
export type { HohEngine } from './engine.ts';

/** The address T3 is built to reach (`VITE_HTTP_URL`); nothing listens there, the page answers it. */
export const HOH_HOSTNAME = 'hoh.local';

export interface InstallOptions extends WireServerOptions {
  readonly engine: HohEngine;
  readonly platform?: EnvironmentPlatform;
  readonly createLocalSettings?: CreateHohLocalSettings;
  readonly hostname?: string;
  readonly pluginAuthoringSkill?: string;
  readonly pluginAuthoringGuide?: string;
  readonly target?: any;
}

/** Start the host and put its socket and fetch in place. Returns the host and a function that undoes it. */
export function installHohHost(options: InstallOptions) {
  const host = new HohHost({ engine: options.engine, platform: options.platform, createLocalSettings: options.createLocalSettings });
  const server = new WireServer(host.handlers(), options);
  const matches = socketMatcher(options.hostname ?? HOH_HOSTNAME);
  const restoreSocket = installHohWebSocket(server, matches, options.target);
  const restoreFetch = installHohFetch(matches, options.target, (threadId, cursor) => host.readTimeline(threadId, cursor), options.platform);
  // T3 uploads a picture's bytes with XMLHttpRequest to the address it was given; the host keeps them.
  const restoreXhr = installHohXhr(matches, (method, pathname, contentType, body) =>
    method === 'POST' ? host.receiveUpload(pathname, contentType, body) : Promise.resolve({ status: 404 }), options.target);
  // The home screen (src/t3-entry/hoh-screens) reads the host through this handle.
  (options.target ?? globalThis).__HOH_HOME__ = host.home;
  (options.target ?? globalThis).__HOH_SECRETARY_CHAT__ = host.secretaryChat;
  (options.target ?? globalThis).__HOH_CHAT_TITLES__ = host.chatTitles;
  (options.target ?? globalThis).__HOH_PLUGINS__ = pluginInventory(options.engine);
  if (options.pluginAuthoringSkill) {
    (options.target ?? globalThis).__HOH_PLUGIN_CREATOR__ = host.pluginCreator(options.pluginAuthoringSkill, options.pluginAuthoringGuide);
  }
  (options.target ?? globalThis).__HOH_TRUST_ENGINE__ = {
    request: (method: string, params: unknown) => host.trustRequest(method, params),
    subscribe: (listener: any) => options.engine.subscribe(listener),
  };
  (options.target ?? globalThis).__HOH_EXTERNAL_IMAGE__ = (threadId: string, path: string) => host.showExternalImage(threadId, path);
  (options.target ?? globalThis).__HOH_MESSAGE_EDIT__ = {
    canEdit: (threadId: string, messageId: string) => host.canEditUserMessage(threadId, messageId),
    send: (threadId: string, messageId: string, text: string) => host.editUserMessage(threadId, messageId, text),
  };
  void host.start();
  return {
    host,
    server,
    uninstall: () => {
      delete (options.target ?? globalThis).__HOH_PLUGINS__;
      delete (options.target ?? globalThis).__HOH_SECRETARY_CHAT__;
      delete (options.target ?? globalThis).__HOH_CHAT_TITLES__;
      delete (options.target ?? globalThis).__HOH_PLUGIN_CREATOR__;
      delete (options.target ?? globalThis).__HOH_EXTERNAL_IMAGE__;
      delete (options.target ?? globalThis).__HOH_MESSAGE_EDIT__;
      restoreSocket(); restoreFetch(); restoreXhr(); host.stop();
    },
  };
}
