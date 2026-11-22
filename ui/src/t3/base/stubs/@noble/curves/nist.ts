// Заглушка окна HoH вместо `@noble/curves/nist` (T3 Code, MIT): скрытые части. Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent, type Anything } from '../../absent';
export const p256: Anything = absent('@noble/curves/nist', 'p256');
export declare namespace p256 { type verify<A = any, B = any, C = any, D = any> = any; }
export type p256<A = any, B = any, C = any, D = any> = any;
