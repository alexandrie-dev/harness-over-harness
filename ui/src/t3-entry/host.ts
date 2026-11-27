// Шов входа T3 с хостом HoH (ui/src/hoh-host, u3; его README — «Как подставить»). Хост экспортирует
//   index.ts: installHohHost({ engine })  — ставит in-memory WebSocket и fetch для http://hoh.local до запуска T3;
//   tauri.ts: tauriEngine()                — движок через мост Tauri (engine_request, события, approval_decide).
// Сборка входа (vite.ts) подставляет их как `virtual:hoh-host`; пока хоста нет — host-stub.ts.

/** Движок HoH для хоста: ui/src/hoh-host/engine.ts (HohEngine). Здесь — только то, что нужно входу. */
export interface HohEngineLike {
  request(method: string, params: unknown): Promise<unknown>;
  environmentPlatform?: import('../hoh-host/engine').HohEngine['environmentPlatform'];
  subscribeStatus?(listener: (status: import('../hoh-host/engine').EngineStatus) => void): Promise<() => void>;
  subscribe?(listener: (notification: { method: string; params: any }) => void): Promise<() => void>;
}
export interface HohHostOptions {
  readonly engine: HohEngineLike;
  readonly platform?: import('../hoh-host/config').EnvironmentPlatform;
  readonly createLocalSettings?: import('./local-settings').CreateHohLocalSettings;
  readonly pluginAuthoringSkill?: string;
  readonly pluginAuthoringGuide?: string;
}
export type InstallHohHost = (options: HohHostOptions) => unknown;
export type TauriEngine = () => HohEngineLike;
