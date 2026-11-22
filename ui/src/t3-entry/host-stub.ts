// Заглушка хоста HoH, пока нет ui/src/hoh-host/index.ts (u3). Сборка входа T3 берёт настоящий хост сама
// (src/t3-entry/vite.ts, `virtual:hoh-host`), как только файл появится. Договор — host.ts.
import type { InstallHohHost, TauriEngine } from './host';

export const installHohHost: InstallHohHost = () => {
  console.warn('HoH: хост данных не подключён (нет ui/src/hoh-host/index.ts) — окно T3 без данных');
};
export const tauriEngine: TauriEngine = () => ({
  request: async () => { throw new Error('HoH: нет хоста данных'); },
});
