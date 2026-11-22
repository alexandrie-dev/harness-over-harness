// Заглушка окна HoH вместо `entities` (T3 Code, MIT): офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic). Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent, type Anything } from './absent';
export const decodeHTMLAttribute: Anything = absent('entities', 'decodeHTMLAttribute');
export type decodeHTMLAttribute<A = any, B = any, C = any, D = any> = any;
export const decodeHTMLStrict: Anything = absent('entities', 'decodeHTMLStrict');
export type decodeHTMLStrict<A = any, B = any, C = any, D = any> = any;
