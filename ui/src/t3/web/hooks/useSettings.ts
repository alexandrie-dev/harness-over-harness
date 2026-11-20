// Прослойка окна HoH вместо apps/web/src/hooks/useSettings.ts (T3 Code, MIT): настройки вида окна HoH: перенос строк в коде выключен, формулы включены, ссылки открывает система.
const SETTINGS = { wordWrap: false, mathRenderingEnabled: true, browserLinkTarget: 'external' } as const;
export function useClientSettings<T>(select: (settings: typeof SETTINGS) => T): T { return select(SETTINGS); }
