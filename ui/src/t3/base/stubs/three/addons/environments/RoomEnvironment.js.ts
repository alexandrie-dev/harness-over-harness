// Заглушка окна HoH вместо `three/addons/environments/RoomEnvironment.js` (T3 Code, MIT): облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh. Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent } from '../../../absent';
export const RoomEnvironment: any = absent('three/addons/environments/RoomEnvironment.js', 'RoomEnvironment');
export declare namespace RoomEnvironment { type js<A = any, B = any, C = any, D = any> = any; }
export type RoomEnvironment<A = any, B = any, C = any, D = any> = any;
