// Вход окна HoH на веб-клиенте T3 (HOH-40). Клиент T3 запускается как у них (apps/web/src/bootstrap.ts → main.tsx:
// startAppServices, AppRoot, router, их index.css, тема и шрифты), но сначала хост HoH ставит свой протокол,
// поэтому клиент говорит с движком HoH, а не с сервером T3. Файлы T3 не правятся.
import { installHohHost, tauriEngine } from 'virtual:hoh-host';
import pluginAuthoringGuide from '../../../docs/plugin-authoring-kit.md?raw';
import pluginAuthoringSkill from './skills/hoh-plugin-authoring/SKILL.md?raw';
import type { HohEngineLike } from './host';
import { HIDDEN_SELECTORS } from './features';
import { createHohLocalSettings } from './local-settings';
import { startUpdateChecks } from './desktop-update';
import './brand.css';
import './hoh-screens/sidebar.css';
import './grid/tokens.css';
import './grid/frame.css';
import { installGridCheck } from './grid/check';
import { configureMacProjectPicker } from './macos-project-picker';
import { installWhisperClient } from './whisper/client';

declare global {
  // Только снимки и проверки в браузере без Tauri (scripts/snapshot-t3.mjs): подделка движка.
  interface Window {
    __HOH_T3_TEST_ENGINE__?: HohEngineLike;
    __HOH_T3_TEST_REQUESTS__?: { method: string; payload: unknown }[];
    __TAURI_INTERNALS__?: unknown;
    __HOH_TRUST_ENGINE__?: HohEngineLike;
  }
}

const testEngine = !window.__TAURI_INTERNALS__ ? window.__HOH_T3_TEST_ENGINE__ : undefined;
const engine = testEngine ?? tauriEngine();
const platform = await engine.environmentPlatform?.().catch(() => undefined);
configureMacProjectPicker(platform);
installWhisperClient(engine);
installHohHost({
  engine,
  platform,
  createLocalSettings: createHohLocalSettings,
  pluginAuthoringSkill,
  pluginAuthoringGuide,
  ...(testEngine ? { onRequest: (method: string, payload: unknown) => window.__HOH_T3_TEST_REQUESTS__?.push({ method, payload }) } : {}),
});
startUpdateChecks();
// Элементы, которые T3 рисует без условия (features.ts, HIDDEN_SELECTORS), скрываются стилем входа.
const hidden = document.createElement('style');
hidden.textContent = HIDDEN_SELECTORS.map((selector) => `${selector} { display: none !important; }`).join('\n');
// Шапка 40 px, как у настольного T3 на Linux/Windows: их окно задаёт полосе заголовка высоту 40 (TITLEBAR_HEIGHT), и класс
// `.wco` берёт её через env(titlebar-area-height); у веб-клиента без этого класса — 52 px (index.css, :root).
hidden.textContent += '\nhtml:root { --workspace-topbar-height: 40px; }';
document.head.append(hidden);
installGridCheck();
if (import.meta.env.VITE_HOH_GRID_OVERLAY === '1') void import('./grid/overlay').then(({ installGridOverlay }) => installGridOverlay());
void import('virtual:t3-bootstrap');
