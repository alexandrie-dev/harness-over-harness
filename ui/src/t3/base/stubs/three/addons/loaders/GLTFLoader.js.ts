// Заглушка окна HoH вместо `three/addons/loaders/GLTFLoader.js` (T3 Code, MIT): облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh. Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent } from '../../../absent';
export const GLTFLoader: any = absent('three/addons/loaders/GLTFLoader.js', 'GLTFLoader');
export declare namespace GLTFLoader { type js<A = any, B = any, C = any, D = any> = any; }
export type GLTFLoader<A = any, B = any, C = any, D = any> = any;
