// Вход окна HoH на веб-клиенте T3 (HOH-40). Клиент T3 запускается как у них (apps/web/src/bootstrap.ts → main.tsx:
// startAppServices, AppRoot, router, их index.css, тема и шрифты), но сначала хост HoH ставит свой протокол,
// поэтому клиент говорит с движком HoH, а не с сервером T3. Файлы T3 не правятся.
import { installHohHost, tauriEngine } from 'virtual:hoh-host';
import type { HohEngineLike } from './host';

declare global {
  // Только снимки и проверки в браузере без Tauri (scripts/snapshot-t3.mjs): подделка движка.
  interface Window { __HOH_T3_TEST_ENGINE__?: HohEngineLike; __TAURI_INTERNALS__?: unknown }
}

const engine = !window.__TAURI_INTERNALS__ && window.__HOH_T3_TEST_ENGINE__ ? window.__HOH_T3_TEST_ENGINE__ : tauriEngine();
installHohHost({ engine });
void import('virtual:t3-bootstrap');
