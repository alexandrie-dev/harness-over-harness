// Умолчания окна HoH для веб-клиента T3 (HOH-40 fix2, решения CEO 22.11 20:00). Выполняется до сценария темы T3
// (boot-theme.js) и пишет в localStorage только то, чего человек ещё не выбирал; дальше всё меняется их же настройками.
(() => {
  try {
    const storage = window.localStorage;
    // Тёмная тема: те же ключи, что T3 пишет при выборе Dark в Appearance (web/hooks/useTheme.ts, setTheme).
    if (storage.getItem('t3code:theme') === null && storage.getItem('t3code:theme-appearance-mode') === null) {
      storage.setItem('t3code:theme', 'dark');
      storage.setItem('t3code:theme-appearance-mode', 'dark');
    }
    // Боковая панель 208 px, как в образце T3 0.1.10: у них начальная ширина — min(256, max(208, ширина окна − 640))
    // при запуске, у нашей основы T3 — всегда 256. Перетаскивание и сброс двойным нажатием — их (AppSidebarLayout.tsx).
    if (storage.getItem('chat_thread_sidebar_width') === null) storage.setItem('chat_thread_sidebar_width', '208');
  } catch {}
})();
