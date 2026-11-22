// Заглушка окна HoH вместо `tiff` (T3 Code, MIT): офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic). Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent } from './absent';
export const decode: any = absent('tiff', 'decode');
export type decode<A = any, B = any, C = any, D = any> = any;
const value: any = absent('tiff', 'default');
type value<A = any, B = any, C = any, D = any> = any;
export default value;
