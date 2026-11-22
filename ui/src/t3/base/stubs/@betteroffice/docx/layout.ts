// Заглушка окна HoH вместо `@betteroffice/docx/layout` (T3 Code, MIT): офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic). Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent } from '../../absent';
export const BundledFontProvider: any = absent('@betteroffice/docx/layout', 'BundledFontProvider');
export type BundledFontProvider<A = any, B = any, C = any, D = any> = any;
export const createRustMeasureSource: any = absent('@betteroffice/docx/layout', 'createRustMeasureSource');
export type createRustMeasureSource<A = any, B = any, C = any, D = any> = any;
