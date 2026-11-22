// Заглушка окна HoH вместо `saxes` (T3 Code, MIT): офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic). Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent } from './absent';
export const SaxesParser: any = absent('saxes', 'SaxesParser');
export type SaxesParser<A = any, B = any, C = any, D = any> = any;
