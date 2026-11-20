// Прослойка окна HoH вместо apps/web/src/browser/browserLinkTarget.ts (T3 Code, MIT): встроенного браузера нет.
export const resolveLinkTarget = (..._args: unknown[]): 'external' => 'external';
