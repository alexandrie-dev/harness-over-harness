// Слой страниц боковой панели над областью справа от панели: «Learning» и дизайн-раунд «Plugins».
// Закрывается, когда человек открывает что-то другое: «New chat», разговор из «Recent chats», другой маршрут.
import { useNavigate, useRouterState } from '@tanstack/react-router';
import { useEffect, useState, useSyncExternalStore } from 'react';
import { createPortal } from 'react-dom';

import { PAGE_TITLES, pages } from './pages';
import { PluginsPage } from './PluginsPage';
import { UsagePage } from './UsagePage';
import { PluginCreatorPage } from './PluginCreatorPage';
import { openCommandPalette } from '~/commandPaletteBus';
import { HOH_ENVIRONMENT_ID } from '../../hoh-host/config';

export function usePage() { return useSyncExternalStore(pages.subscribe, pages.current, () => null); }

export function PageLayer() {
  const page = usePage();
  const navigate = useNavigate();
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
      {page === 'plugins' ? <PluginsPage /> : page === 'usage' ? <UsagePage onClose={pages.close} /> : page === 'plugin-creator' ? (
        <PluginCreatorPage
          onAddProject={() => openCommandPalette({ open: 'add-project' })}
          onOpenThread={(threadId) => {
            void navigate({ to: '/$environmentId/$threadId', params: { environmentId: HOH_ENVIRONMENT_ID, threadId } });
          }}
        />
      ) : (
        <div className="hoh-page__column">
          <h1>{PAGE_TITLES[page]}</h1>
          <p>Coming Soon</p>
        </div>
      )}
    </div>,
    host,
  );
}
