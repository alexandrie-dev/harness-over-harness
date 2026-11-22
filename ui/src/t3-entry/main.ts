// Вход окна HoH на веб-клиенте T3 (HOH-40). Клиент T3 запускается как у них (apps/web/src/bootstrap.ts → main.tsx:
// startAppServices, AppRoot, router, их index.css, тема и шрифты), но сначала хост HoH ставит свой протокол,
// поэтому клиент говорит с движком HoH, а не с сервером T3. Файлы T3 не правятся.
import { installHohHost } from 'virtual:hoh-host';
import { HIDDEN_AREAS } from './features';

installHohHost({ hidden: HIDDEN_AREAS });
void import('virtual:t3-bootstrap');
