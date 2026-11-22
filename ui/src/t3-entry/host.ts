// Шов входа T3 с хостом HoH (ui/src/hoh-host, u3). Хост экспортирует из ui/src/hoh-host/index.ts:
//   export const installHohHost: InstallHohHost
// Вход вызывает его один раз, до загрузки кода T3 (src/t3-entry/main.ts): хост ставит свой протокол
// (in-memory WebSocket в globalThis, см. hoh-host/socket.ts) и сообщает в конфиге сервера только видимые области.
import type { HiddenArea } from './features';

export interface HohHostOptions {
  /** Скрытые области (features.ts): хост не сообщает их в конфиге/возможностях сервера. */
  readonly hidden: readonly HiddenArea[];
}
export type InstallHohHost = (options: HohHostOptions) => void;
