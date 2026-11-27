import { ChevronLeftIcon, ChevronRightIcon } from 'lucide-react';
import { useEffect, useRef } from 'react';
import { useGT } from 'gt-react';
import { Button } from '~/components/ui/button';
import { useSidebar, useSidebarVisibility } from '~/components/ui/sidebar';
import { Tooltip, TooltipPopup, TooltipTrigger } from '~/components/ui/tooltip';

export function SidebarToggle() {
  const { isMobile, onCollapse, toggleSidebar } = useSidebar();
  const expanded = useSidebarVisibility();
  const trigger = useRef<HTMLButtonElement>(null);
  const gt = useGT();
  const label = isMobile ? gt('Close sidebar') : expanded ? gt('Collapse sidebar') : gt('Expand sidebar');
  const Icon = expanded ? ChevronLeftIcon : ChevronRightIcon;

  useEffect(() => onCollapse(() => {
    const hidden = document.activeElement?.closest('[data-hoh-hide-in-rail]');
    const sidebar = trigger.current?.closest('[data-app-sidebar]');
    if (hidden && sidebar?.contains(hidden)) trigger.current?.focus({ preventScroll: true });
  }), [onCollapse]);

  return (
    <Tooltip>
      <TooltipTrigger render={
        <Button
          ref={trigger}
          className="hoh-sidebar-toggle"
          data-hoh-sidebar-toggle=""
          data-sidebar="trigger"
          data-slot="sidebar-trigger"
          aria-label={label}
          aria-expanded={expanded}
          size="icon"
          variant="ghost"
          onClick={toggleSidebar}
        >
          <Icon aria-hidden />
        </Button>
      } />
      <TooltipPopup side={expanded ? 'bottom' : 'right'}>{label}</TooltipPopup>
    </Tooltip>
  );
}
