import type { ServerProvider, ServerSettings } from '@t3tools/contracts';
import { useGT } from 'gt-react';
import { SettingsRow, SettingsSection } from '~/components/settings/settingsLayout';
import {
  resolveOnboardingProviderInstallCommand,
  resolveOnboardingProviderLoginCommand,
} from '~/onboarding/providerReadiness.logic';
import { usePrimaryEnvironment } from '~/state/environments';

export function ProviderSetup({ provider }: { readonly provider: ServerProvider | undefined }) {
  const gt = useGT();
  const environment = usePrimaryEnvironment();
  const platform = environment?.serverConfig?.environment.platform.os ?? 'linux';
  const settings: ServerSettings | undefined = environment?.serverConfig?.settings;
  if (!provider || !settings || provider.status === 'ready') return null;
  const { driver } = provider;
  if (driver !== 'claudeAgent' && driver !== 'codex' && driver !== 'pi') return null;

  let command: string;
  let title: string;
  if (!provider.installed) {
    command = driver === 'pi'
      ? 'npm install -g @earendil-works/pi-coding-agent@1.6'
      : resolveOnboardingProviderInstallCommand(driver === 'codex' ? 'codex' : 'claudeAgent', platform);
    title = gt('Install the CLI');
  } else if (driver === 'pi') {
    command = 'pi';
    title = gt('Open Pi and configure a model');
  } else if (provider.auth.status === 'unauthenticated') {
    command = resolveOnboardingProviderLoginCommand(provider, settings, platform);
    title = gt('Sign in to the CLI');
  } else {
    return null;
  }

  return (
    <SettingsSection title={gt('Setup')}>
      <SettingsRow
        title={title}
        description={gt('Run this in a terminal, then refresh provider status.')}
        control={
          <code className="min-w-0 select-text break-all rounded-md border border-border bg-muted/40 px-2 py-1 text-xs">
            {command}
          </code>
        }
      />
    </SettingsSection>
  );
}
