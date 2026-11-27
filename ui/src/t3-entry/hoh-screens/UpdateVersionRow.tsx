import { useStore } from 'zustand';
import { AboutVersionTitle } from '~/components/settings/AboutVersionTitle';
import { SettingsRow } from '~/components/settings/settingsLayout';
import { Button } from '~/components/ui/button';
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
      return `HoH ${state.version} is available. Select Update to install and restart.`;
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

export function UpdateVersionRow({ id }: { id: string }) {
  const state = useStore(desktopUpdate);
  return (
    <SettingsRow
      id={id}
      title={<AboutVersionTitle />}
      description={<span role="status">{updateMessage(state)}</span>}
      control={
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={updateInProgress(state) || state.kind === 'unsupportedInstall'}
          onClick={() => void installUpdate()}
        >
          Update
        </Button>
      }
    />
  );
}
