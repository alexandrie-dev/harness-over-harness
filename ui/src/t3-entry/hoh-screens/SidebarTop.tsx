// Верх боковой панели окна HoH (HOH-41, состав раунда 12: design/round12/shots/home.png), нарисованный компонентами и видом T3:
// «New chat» с сочетанием клавиш T3, группа «Team» с «+» (строк нет: движка сотрудников нет), заголовок «Recent chats» над настоящим
// списком разговоров T3. Под «New chat» находятся Learning и Plugins. Подставляется вместо SidebarThreadHeader при сборке (vite.ts, sidebarList);
// получает те же свойства, что получал T3 (SidebarThreadHeaderProps).
import { ChartNoAxesColumnIcon, GraduationCapIcon, PlusIcon, PuzzleIcon, SquarePenIcon, WrenchIcon } from 'lucide-react';
import { useNavigate } from '@tanstack/react-router';
import { useState, type MouseEvent } from 'react';

import { SidebarHeaderIconButton } from '~/components/sidebar/SidebarThreadHeader';
import { SidebarMenuButton } from '~/components/ui/sidebar';
import { PAGE_TITLES, pages, type PageKey } from './pages';
import { PageLayer, usePage } from './SidebarPages';
import './sidebar.css';

const PAGE_ICONS = {
  learning: GraduationCapIcon, plugins: PuzzleIcon, 'plugin-creator': WrenchIcon, usage: ChartNoAxesColumnIcon,
} as const;

interface Props {
  onNewThread: (event: MouseEvent) => void;
  newThreadDisabled: boolean;
  newThreadShortcutLabel: string | null | undefined;
}

export function SidebarTop({ onNewThread, newThreadDisabled, newThreadShortcutLabel }: Props) {
  const page = usePage();
  const navigate = useNavigate();
  const [teamComingSoon, setTeamComingSoon] = useState(false);
  return (
    <div className="hoh-side" data-hoh-sidebar-top="">
      <PageLayer />
      <SidebarMenuButton type="button" className="hoh-side__new" disabled={newThreadDisabled} onClick={(event) => { pages.close(); onNewThread(event); }}>
        <SquarePenIcon aria-hidden />
        <span>New chat</span>
        {newThreadShortcutLabel ? <kbd className="hoh-side__kbd">{newThreadShortcutLabel}</kbd> : null}
      </SidebarMenuButton>
      {(['learning', 'plugins', 'plugin-creator', 'usage'] as PageKey[]).map((key) => {
        const Icon = PAGE_ICONS[key];
        return (
          <SidebarMenuButton key={key} type="button" className="hoh-side__new" isActive={page === key} aria-current={page === key ? 'page' : undefined} data-hoh-page-item={key} onClick={() => {
            if (key === 'usage') { pages.close(); void navigate({ to: '/usage' }); }
            else pages.open(key);
          }}>
            <Icon aria-hidden />
            <span>{PAGE_TITLES[key]}</span>
          </SidebarMenuButton>
        );
      })}
      <div className="hoh-side__group">
        <span>Team</span>
        {/* Движка сотрудников у HoH нет: «+» ничего не создаёт и выключена, пока он не появится. */}
        <SidebarHeaderIconButton label="Add to team" tooltip="Coming Soon" onClick={() => setTeamComingSoon((visible) => !visible)}>
          <PlusIcon />
        </SidebarHeaderIconButton>
      </div>
      {teamComingSoon ? <p className="hoh-side__coming-soon" role="status">Coming Soon</p> : null}
      <div className="hoh-side__group hoh-side__group--list"><span>Recent chats</span></div>
    </div>
  );
}
