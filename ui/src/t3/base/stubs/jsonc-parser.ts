// Заглушка окна HoH вместо `jsonc-parser` (T3 Code, MIT): импорт и экспорт тем VS Code / Open VSX. Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent } from './absent';
export const ParseError: any = absent('jsonc-parser', 'ParseError');
export type ParseError<A = any, B = any, C = any, D = any> = any;
export const parse: any = absent('jsonc-parser', 'parse');
export type parse<A = any, B = any, C = any, D = any> = any;
