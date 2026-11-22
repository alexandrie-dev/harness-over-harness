// Верх боковой панели окна HoH (HOH-41, состав раунда 12: design/round12/shots/home.png), нарисованный компонентами и видом T3:
// «New chat» с сочетанием клавиш T3, группа «Team» с «+» (строк нет: движка сотрудников нет), заголовок «Recent chats» над настоящим
// списком разговоров T3. «Обучение» и «Виджеты» не рисуются. Подставляется вместо SidebarThreadHeader при сборке (vite.ts, sidebarList);
// получает те же свойства, что получал T3 (SidebarThreadHeaderProps).
import { PlusIcon, SquarePenIcon } from 'lucide-react';
import type { MouseEvent } from 'react';

import { SidebarHeaderIconButton } from '~/components/sidebar/SidebarThreadHeader';
import { SidebarMenuButton } from '~/components/ui/sidebar';
import './sidebar.css';

interface Props {
  onNewThread: (event: MouseEvent) => void;
  newThreadDisabled: boolean;
  newThreadShortcutLabel: string | null | undefined;
}

export function SidebarTop({ onNewThread, newThreadDisabled, newThreadShortcutLabel }: Props) {
  return (
    <div className="hoh-side" data-hoh-sidebar-top="">
      <SidebarMenuButton type="button" className="hoh-side__new" disabled={newThreadDisabled} onClick={onNewThread}>
        <SquarePenIcon aria-hidden />
        <span>New chat</span>
        {newThreadShortcutLabel ? <kbd className="hoh-side__kbd">{newThreadShortcutLabel}</kbd> : null}
      </SidebarMenuButton>
      <div className="hoh-side__group">
        <span>Team</span>
        {/* Движка сотрудников у HoH нет: «+» ничего не создаёт и выключена, пока он не появится. */}
        <SidebarHeaderIconButton label="Add to team" tooltip="Not available yet" disabled>
          <PlusIcon />
        </SidebarHeaderIconButton>
      </div>
      <div className="hoh-side__group hoh-side__group--list"><span>Recent chats</span></div>
    </div>
  );
}
