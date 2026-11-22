// Заглушка окна HoH вместо `@clerk/electron/react` (T3 Code, MIT): облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh. Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent, type Anything } from '../../absent';
export const ClerkProvider: Anything = absent('@clerk/electron/react', 'ClerkProvider');
export type ClerkProvider<A = any, B = any, C = any, D = any> = any;
