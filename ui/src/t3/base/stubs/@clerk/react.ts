// Заглушка окна HoH вместо `@clerk/react` (T3 Code, MIT): облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh. Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent, type Anything } from '../absent';
export const ClerkProvider: Anything = absent('@clerk/react', 'ClerkProvider');
export type ClerkProvider<A = any, B = any, C = any, D = any> = any;
export const ClerkProviderProps: Anything = absent('@clerk/react', 'ClerkProviderProps');
export type ClerkProviderProps<A = any, B = any, C = any, D = any> = any;
export const UserButton: Anything = absent('@clerk/react', 'UserButton');
export declare namespace UserButton { type UserProfilePage<A = any, B = any, C = any, D = any> = any; }
export type UserButton<A = any, B = any, C = any, D = any> = any;
export const useAuth: Anything = absent('@clerk/react', 'useAuth');
export type useAuth<A = any, B = any, C = any, D = any> = any;
export const useClerk: Anything = absent('@clerk/react', 'useClerk');
export type useClerk<A = any, B = any, C = any, D = any> = any;
