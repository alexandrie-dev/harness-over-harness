// Страницы боковой панели: пустая «Learning» и живой раунд дизайна «Plugins» (HOH-42).
// Это не маршруты T3 (дерево маршрутов не трогаем): страница — слой поверх области справа от боковой панели (SidebarPages.tsx).
export type PageKey = 'learning' | 'plugins' | 'plugin-creator' | 'usage';
export const PAGE_TITLES: Readonly<Record<PageKey, string>> = {
  learning: 'Learning', plugins: 'Plugins', 'plugin-creator': 'Plugin Creator', usage: 'Usage',
};

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
