// Слой пустой страницы «Learning» / «Widgets» над областью справа от боковой панели: заголовок и одна строка «Coming soon».
// Закрывается, когда человек открывает что-то другое: «New chat», разговор из «Recent chats», другой маршрут.
import { useRouterState } from '@tanstack/react-router';
import { useEffect, useState, useSyncExternalStore } from 'react';
import { createPortal } from 'react-dom';

import { PAGE_TITLES, pages } from './pages';

export function usePage() { return useSyncExternalStore(pages.subscribe, pages.current, () => null); }

export function PageLayer() {
  const page = usePage();
  const path = useRouterState({ select: (state) => state.location.pathname });
  const [host, setHost] = useState<Element | null>(null);
  useEffect(() => { setHost(document.querySelector('[data-slot="sidebar-inset"]')); }, [page]);
  useEffect(() => { pages.close(); }, [path]);
  useEffect(() => {
    const onClick = (event: MouseEvent) => { if ((event.target as Element | null)?.closest?.('[data-thread-item]')) pages.close(); };
    document.addEventListener('click', onClick, true);
    return () => document.removeEventListener('click', onClick, true);
  }, []);
  if (!page || !host) return null;
  return createPortal(
    <div className="hoh-page" data-hoh-page={page}>
      <div className="hoh-page__column">
        <h1>{PAGE_TITLES[page]}</h1>
        <p>Coming soon</p>
      </div>
    </div>,
    host,
  );
}
