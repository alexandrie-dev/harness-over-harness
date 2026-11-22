// Заглушка окна HoH вместо `@daypicker/react` (T3 Code, MIT): скрытые части. Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent, type Anything } from '../absent';
export const DayPicker: Anything = absent('@daypicker/react', 'DayPicker');
export type DayPicker<A = any, B = any, C = any, D = any> = any;
