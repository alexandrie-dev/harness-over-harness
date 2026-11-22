// Заглушка окна HoH вместо `sql.js` (T3 Code, MIT): офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic). Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent, type Anything } from './absent';
export const Database: Anything = absent('sql.js', 'Database');
export type Database<A = any, B = any, C = any, D = any> = any;
const value: Anything = absent('sql.js', 'default');
type value<A = any, B = any, C = any, D = any> = any;
export default value;
