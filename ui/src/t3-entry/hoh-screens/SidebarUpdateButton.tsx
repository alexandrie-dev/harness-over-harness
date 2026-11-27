import { invoke, isTauri } from '@tauri-apps/api/core';
import { ArrowUpRightIcon, RefreshCwIcon } from 'lucide-react';
import { useRef, useState } from 'react';
import { useStore } from 'zustand';
import { Button } from '~/components/ui/button';
import {
  Dialog, DialogDescription, DialogFooter, DialogHeader, DialogPopup, DialogTitle, DialogTrigger,
} from '~/components/ui/dialog';
import { SidebarMenuButton, SidebarMenuItem } from '~/components/ui/sidebar';
import { checkForUpdate, desktopUpdate, installUpdate, updateInProgress, type UpdateState } from '../desktop-update';

function updateMessage(state: UpdateState): string {
  switch (state.kind) {
    case 'idle':
      return 'Check GitHub for the latest stable release.';
    case 'checking':
      return 'Checking GitHub for updates…';
    case 'unsupportedInstall':
      return 'Download a release package from GitHub to enable updates.';
    case 'noRelease':
      return 'No stable release has been published yet.';
    case 'upToDate':
      return 'HoH is up to date.';
    case 'rolledBack':
      return 'The update could not start. HoH restored the previous version.';
    case 'noCompatiblePackage':
      return `HoH ${state.version} has no package for this system.`;
    case 'available':
      return `HoH ${state.version} is available. Installing it restarts HoH and interrupts running tasks.`;
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
  const trigger = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const [linkFailed, setLinkFailed] = useState(false);
  if (!isTauri()) return null;

  const busy = updateInProgress(state);

  function openReleases() {
    setLinkFailed(false);
    void invoke('open_update_releases')
      .catch(() => setLinkFailed(true));
  }

  return (
    <SidebarMenuItem className="ms-auto shrink-0">
      <Dialog
        open={open}
        onOpenChange={(next) => { setOpen(next); setLinkFailed(false); }}
      >
        <DialogTrigger
          render={
            <SidebarMenuButton
              ref={trigger}
              className="hoh-side__action"
              aria-label="Update"
              tooltip="Update"
              isActive={state.kind === 'available'}
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
        <DialogPopup finalFocus={trigger}>
          <DialogHeader>
            <DialogTitle>Update HoH</DialogTitle>
            <DialogDescription aria-live="polite">{updateMessage(state)}</DialogDescription>
            {linkFailed ? <p className="text-sm" role="alert">Could not open GitHub in your browser.</p> : null}
          </DialogHeader>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={openReleases}>
              GitHub Releases <ArrowUpRightIcon aria-hidden />
            </Button>
            {state.kind === 'available' ? (
              <Button type="button" onClick={() => void installUpdate()}>
                Install and restart
              </Button>
            ) : state.kind === 'unsupportedInstall' ? null : (
              <Button type="button" disabled={busy} onClick={() => void checkForUpdate()}>
                Check for updates
              </Button>
            )}
          </DialogFooter>
        </DialogPopup>
      </Dialog>
    </SidebarMenuItem>
  );
}
