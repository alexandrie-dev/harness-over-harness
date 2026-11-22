// Заглушка окна HoH вместо `@clerk/electron/passkeys` (T3 Code, MIT): облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh. Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent, type Anything } from '../../absent';
export const passkeys: Anything = absent('@clerk/electron/passkeys', 'passkeys');
export type passkeys<A = any, B = any, C = any, D = any> = any;
