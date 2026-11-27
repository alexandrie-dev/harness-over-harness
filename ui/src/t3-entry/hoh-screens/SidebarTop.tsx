import { ChartNoAxesColumnIcon, GraduationCapIcon, LayoutDashboardIcon, MessagesSquareIcon, PanelRightIcon, PlusIcon, PuzzleIcon, SquarePenIcon, WrenchIcon } from 'lucide-react';
import { useLocation, useNavigate } from '@tanstack/react-router';
import { useEffect, useState } from 'react';
import { useStore } from 'zustand';

import { SidebarHeaderIconButton, SidebarThreadHeader, type SidebarThreadHeaderProps } from '~/components/sidebar/SidebarThreadHeader';
import { SidebarMenuButton, useSidebar } from '~/components/ui/sidebar';
import { PAGE_TITLES, pages, workspacePage } from './pages';
import { PageLayer, usePage } from './SidebarPages';
import { externalUiError, openExternalUiPanel, useExternalUiCatalog } from './external-ui-sdk';
import { HOH_ENVIRONMENT_ID } from '../../hoh-host/config';
import { toastManager } from '~/components/ui/toast';
import { desktopUpdate } from '../desktop-update';

let announcedUpdate: string | undefined;

const WORKSPACE_PAGES = [
  ['learning', GraduationCapIcon],
  ['plugins', PuzzleIcon],
  ['plugin-creator', WrenchIcon],
  ['usage', ChartNoAxesColumnIcon],
] as const satisfies ReadonlyArray<readonly [keyof typeof PAGE_TITLES, typeof PuzzleIcon]>;

export function SidebarTop(props: SidebarThreadHeaderProps) {
  const { onNewThread, newThreadDisabled, newThreadShortcutLabel } = props;
  const page = usePage();
  const { catalog } = useExternalUiCatalog();
  const navigate = useNavigate();
  const onUsage = useLocation({ select: (location) => location.pathname === '/usage' });
  const { isMobile, setOpen, setOpenMobile } = useSidebar();
  const [teamComingSoon, setTeamComingSoon] = useState(false);
  const updateVersion = useStore(desktopUpdate, state => state.kind === 'available' ? state.version : undefined);
  useEffect(() => {
    if (!updateVersion || updateVersion === announcedUpdate) return;
    announcedUpdate = updateVersion;
    toastManager.add({
      type: 'info',
      title: `HoH ${updateVersion} is available`,
      description: 'Use Update next to Settings to install.',
    });
  }, [updateVersion]);
  return (
    <div className="hoh-side" data-hoh-sidebar-top="">
      <PageLayer workspace={catalog?.packages.find(item => item.workspaceLabel && page === workspacePage(item.id))} />
      <SidebarMenuButton
        type="button"
        className="hoh-side__action hoh-side__new"
        aria-label="New chat"
        tooltip="New chat"
        disabled={newThreadDisabled}
        onClick={(event) => { pages.close(); onNewThread(event); }}
      >
        <SquarePenIcon aria-hidden />
        <span data-hoh-rail-label="">New chat</span>
        {newThreadShortcutLabel ? <kbd className="hoh-side__kbd" data-hoh-rail-label="">{newThreadShortcutLabel}</kbd> : null}
      </SidebarMenuButton>
      <div className="hoh-side__section">
        <div className="hoh-side__group" data-hoh-hide-in-rail="">Workspace</div>
        <div className="hoh-side__tray">
          {WORKSPACE_PAGES.map(([key, Icon]) => {
            const label = PAGE_TITLES[key];
            const active = key === 'usage' ? !page && onUsage : page === key;
            return (
              <SidebarMenuButton
                key={key}
                type="button"
                className="hoh-side__action"
                aria-label={label}
                tooltip={label}
                isActive={active}
                aria-current={active ? 'page' : undefined}
                data-hoh-page-item={key}
                onClick={() => {
                  if (isMobile) setOpenMobile(false);
                  if (key === 'usage') {
                    pages.close();
                    void navigate({ to: '/usage' });
                  } else {
                    pages.open(key);
                  }
                }}
              >
                <span className="hoh-side__glyph" aria-hidden><Icon size={16} /></span>
                <span data-hoh-rail-label="">{label}</span>
              </SidebarMenuButton>
            );
          })}
          {catalog?.packages.map((item) => {
            const key = workspacePage(item.id);
            const active = page === key;
            if (item.workspaceLabel) return (
              <SidebarMenuButton
                key={item.id}
                type="button"
                className="hoh-side__action"
                aria-label={item.workspaceLabel}
                tooltip={item.workspaceLabel}
                isActive={active}
                aria-current={active ? 'page' : undefined}
                data-hoh-page-item={key}
                onClick={() => {
                  if (isMobile) setOpenMobile(false);
                  pages.open(key);
                }}
              >
                <span className="hoh-side__glyph" aria-hidden><LayoutDashboardIcon size={16} /></span>
                <span data-hoh-rail-label="">{item.workspaceLabel}</span>
              </SidebarMenuButton>
            );
            const panel = item.chatPanel;
            if (!panel) return null;
            return (
              <SidebarMenuButton
                key={item.id}
                type="button"
                className="hoh-side__action"
                aria-label={panel.sidebarLabel}
                tooltip={panel.sidebarLabel}
                onClick={() => {
                  if (isMobile) setOpenMobile(false);
                  pages.close();
                  void openExternalUiPanel(item, (threadId) => {
                    void navigate({ to: '/$environmentId/$threadId',
                      params: { environmentId: HOH_ENVIRONMENT_ID, threadId } });
                  }).catch(error => toastManager.add({
                    type: 'error', title: externalUiError(error, 'The plugin panel could not be opened.'),
                  }));
                }}
              >
                <span className="hoh-side__glyph" aria-hidden><PanelRightIcon size={16} /></span>
                <span data-hoh-rail-label="">{panel.sidebarLabel}</span>
              </SidebarMenuButton>
            );
          })}
        </div>
      </div>
      <div className="hoh-side__section hoh-side__team" data-hoh-hide-in-rail="">
        <div className="hoh-side__group">
          <span>Team</span>
          <SidebarHeaderIconButton label="Add to team" tooltip="Coming Soon" onClick={() => setTeamComingSoon((visible) => !visible)}>
            <PlusIcon />
          </SidebarHeaderIconButton>
        </div>
        {teamComingSoon ? <p className="hoh-side__coming-soon" role="status">Coming Soon</p> : null}
      </div>
      <div className="hoh-side__section hoh-side__recent">
        <div className="hoh-side__group" data-hoh-hide-in-rail="">Recent chats</div>
        <div data-hoh-hide-in-rail="">
          <SidebarThreadHeader
            {...props}
            showNewThread={false}
            onNewProject={() => { pages.close(); props.onNewProject(); }}
          />
        </div>
        <SidebarMenuButton
          type="button"
          className="hoh-side__action hoh-side__recent-trigger"
          aria-label="Show recent chats"
          tooltip="Recent chats"
          onClick={(event) => {
            const sidebar = event.currentTarget.closest('[data-app-sidebar]');
            setOpen(true);
            sidebar?.querySelector<HTMLButtonElement>('[data-hoh-sidebar-toggle]')?.focus();
          }}
        >
          <MessagesSquareIcon aria-hidden />
        </SidebarMenuButton>
      </div>
    </div>
  );
}
