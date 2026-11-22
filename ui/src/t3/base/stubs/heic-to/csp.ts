// Заглушка окна HoH вместо `heic-to/csp` (T3 Code, MIT): вложения (загрузка) и снимки экрана (snap-shot); офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic). Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent, type Anything } from '../absent';
const value: Anything = absent('heic-to/csp', 'default');
type value<A = any, B = any, C = any, D = any> = any;
export default value;
