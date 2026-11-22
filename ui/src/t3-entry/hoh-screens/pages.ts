// Пустые страницы «Learning» и «Widgets» (HOH-41, ответ основателя: «пустыми да, позже доделаем»): какая открыта сейчас.
// Это не маршруты T3 (дерево маршрутов не трогаем): страница — слой поверх области справа от боковой панели (SidebarPages.tsx).
export type PageKey = 'learning' | 'widgets';
export const PAGE_TITLES: Readonly<Record<PageKey, string>> = { learning: 'Learning', widgets: 'Widgets' };

export interface PageStore {
  current(): PageKey | null;
  open(page: PageKey): void;
  close(): void;
  subscribe(listener: () => void): () => void;
}
export function createPageStore(): PageStore {
  let page: PageKey | null = null;
  const listeners = new Set<() => void>();
  const set = (next: PageKey | null) => { if (next !== page) { page = next; for (const listener of listeners) listener(); } };
  return { current: () => page, open: (next) => set(next), close: () => set(null), subscribe: (listener) => { listeners.add(listener); return () => listeners.delete(listener); } };
}
export const pages = createPageStore();
