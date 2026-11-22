// Заглушка окна HoH вместо `fast-xml-parser` (T3 Code, MIT): офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic). Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent } from './absent';
export const XMLParser: any = absent('fast-xml-parser', 'XMLParser');
export type XMLParser<A = any, B = any, C = any, D = any> = any;
export const XMLValidator: any = absent('fast-xml-parser', 'XMLValidator');
export declare namespace XMLValidator { type validate<A = any, B = any, C = any, D = any> = any; }
export type XMLValidator<A = any, B = any, C = any, D = any> = any;
