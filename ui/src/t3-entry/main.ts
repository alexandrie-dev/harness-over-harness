// Вход окна HoH на веб-клиенте T3 (HOH-40). Клиент T3 запускается как у них (apps/web/src/bootstrap.ts → main.tsx:
// startAppServices, AppRoot, router, их index.css, тема и шрифты), но сначала хост HoH ставит свой протокол,
// поэтому клиент говорит с движком HoH, а не с сервером T3. Файлы T3 не правятся.
import { installHohHost, tauriEngine } from 'virtual:hoh-host';
import type { HohEngineLike } from './host';
import { HIDDEN_SELECTORS } from './features';
import './brand.css';
import { startEmblemFit } from './emblem-fit';

declare global {
  // Только снимки и проверки в браузере без Tauri (scripts/snapshot-t3.mjs): подделка движка.
  interface Window { __HOH_T3_TEST_ENGINE__?: HohEngineLike; __TAURI_INTERNALS__?: unknown }
}

const engine = !window.__TAURI_INTERNALS__ && window.__HOH_T3_TEST_ENGINE__ ? window.__HOH_T3_TEST_ENGINE__ : tauriEngine();
installHohHost({ engine });
// Элементы, которые T3 рисует без условия (features.ts, HIDDEN_SELECTORS), скрываются стилем входа.
const hidden = document.createElement('style');
hidden.textContent = HIDDEN_SELECTORS.map((selector) => `${selector} { display: none !important; }`).join('\n');
// Шапка 40 px, как у настольного T3 на Linux/Windows: их окно задаёт полосе заголовка высоту 40 (TITLEBAR_HEIGHT), и класс
// `.wco` берёт её через env(titlebar-area-height); у веб-клиента без этого класса — 52 px (index.css, :root).
hidden.textContent += '\nhtml:root { --workspace-topbar-height: 40px; }';
document.head.append(hidden);
startEmblemFit();
void import('virtual:t3-bootstrap');
