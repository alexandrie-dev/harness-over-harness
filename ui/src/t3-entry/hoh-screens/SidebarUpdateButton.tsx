import { isTauri } from '@tauri-apps/api/core';
import { RefreshCwIcon } from 'lucide-react';
import { useStore } from 'zustand';
import { SidebarMenuButton, SidebarMenuItem } from '~/components/ui/sidebar';
import { toastManager } from '~/components/ui/toast';
import { Tooltip, TooltipPopup, TooltipTrigger } from '~/components/ui/tooltip';
import { desktopUpdate, installUpdate, updateInProgress, type UpdateState } from '../desktop-update';

function updateMessage(state: UpdateState): string {
  switch (state.kind) {
    case 'idle':
      return 'HoH checks GitHub for updates automatically.';
    case 'checking':
      return 'Checking GitHub for updates…';
    case 'unsupportedInstall':
      return 'Install HoH from a release package to use updates.';
    case 'noRelease':
      return 'No stable release has been published yet.';
    case 'upToDate':
      return 'HoH is up to date.';
    case 'noCompatiblePackage':
      return `HoH ${state.version} has no package for this system.`;
    case 'available':
      return `Install HoH ${state.version} and restart.`;
    case 'downloading':
      return `Downloading HoH ${state.version}…`;
    case 'installing':
      return `Installing HoH ${state.version}…`;
    case 'restarting':
      return `Restarting in HoH ${state.version}…`;
    case 'error':
      return state.message;
  }
}

export function SidebarUpdateButton() {
  const state = useStore(desktopUpdate);
  if (!isTauri()) return null;

  const busy = updateInProgress(state);
  const message = updateMessage(state);

  async function handleUpdate() {
    const result = await installUpdate();
    if (result && !updateInProgress(result)) {
      toastManager.add({
        type: result.kind === 'error' ? 'error' : 'info',
        title: updateMessage(result),
      });
    }
  }

  return (
    <SidebarMenuItem className="ms-auto shrink-0">
      <Tooltip>
        <TooltipTrigger
          render={
            <SidebarMenuButton
              type="button"
              className="hoh-side__action"
              aria-label="Update"
              aria-busy={busy}
              isActive={state.kind === 'available'}
              disabled={busy || state.kind === 'unsupportedInstall'}
              onClick={() => void handleUpdate()}
            >
              <RefreshCwIcon
                aria-hidden
                data-hoh-update-icon=""
                className={busy ? 'motion-safe:animate-spin' : undefined}
              />
              <span data-hoh-rail-label="">Update</span>
            </SidebarMenuButton>
          }
        />
        <TooltipPopup side="top">{message}</TooltipPopup>
      </Tooltip>
      <span className="sr-only" role="status">{message}</span>
    </SidebarMenuItem>
  );
}
