// Прослойка окна HoH вместо apps/web/src/browser/useOpenLink.ts (T3 Code, MIT): ссылки открывает система.
export const useOpenLink = (_threadRef?: unknown) => (href: string): void => { window.open(href, '_blank', 'noopener'); };
