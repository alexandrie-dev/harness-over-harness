// Шов входа T3 с хостом HoH (ui/src/hoh-host, u3; его README — «Как подставить»). Хост экспортирует
//   index.ts: installHohHost({ engine })  — ставит in-memory WebSocket и fetch для http://hoh.local до запуска T3;
//   tauri.ts: tauriEngine()                — движок через мост Tauri (engine_request, события, approval_decide).
// Сборка входа (vite.ts) подставляет их как `virtual:hoh-host`; пока хоста нет — host-stub.ts.

/** Движок HoH для хоста: ui/src/hoh-host/engine.ts (HohEngine). Здесь — только то, что нужно входу. */
export interface HohEngineLike {
  request(method: string, params: unknown): Promise<unknown>;
  subscribe?(listener: (notification: { method: string; params: any }) => void): Promise<() => void>;
}
export interface HohHostOptions {
  readonly engine: HohEngineLike;
  readonly pluginAuthoringSkill?: string;
}
export type InstallHohHost = (options: HohHostOptions) => unknown;
export type TauriEngine = () => HohEngineLike;

/** Адрес, на который собран T3 (VITE_HTTP_URL): его никто не слушает, отвечает хост в странице. */
export const HOH_HTTP_URL = 'http://hoh.local';
