// Заглушка окна HoH вместо `@clerk/react` (T3 Code, MIT): облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh. Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent } from '../absent';
export const ClerkProvider: any = absent('@clerk/react', 'ClerkProvider');
export type ClerkProvider<A = any, B = any, C = any, D = any> = any;
export const ClerkProviderProps: any = absent('@clerk/react', 'ClerkProviderProps');
export type ClerkProviderProps<A = any, B = any, C = any, D = any> = any;
export const UserButton: any = absent('@clerk/react', 'UserButton');
export declare namespace UserButton { type UserProfilePage<A = any, B = any, C = any, D = any> = any; }
export type UserButton<A = any, B = any, C = any, D = any> = any;
export const useAuth: any = absent('@clerk/react', 'useAuth');
export type useAuth<A = any, B = any, C = any, D = any> = any;
export const useClerk: any = absent('@clerk/react', 'useClerk');
export type useClerk<A = any, B = any, C = any, D = any> = any;
