// Прослойка окна HoH вместо packages/shared/src/threadLinks.ts (T3 Code, MIT): в окне нет связанных потоков, ссылки на них не разбираются.
export const THREAD_LINK_PROTOCOL = 't3-thread:';
export function parseThreadLinkHref(_href: string): null { return null; }
