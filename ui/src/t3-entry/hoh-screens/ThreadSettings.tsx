import { useGT, useMessages } from 'gt-react';
import { resolveRuntimeModeOptions, runtimeModeConfig, runtimeModeOptions } from '~/components/chat/runtimeModeConfig';
import { SnoozePresetsRow } from '~/components/settings/SnoozePresetsSettings';
import { SettingsPageContainer, SettingsRow, SettingsSection } from '~/components/settings/settingsLayout';
import { useSearchableSetting } from '~/components/settings/settingsSearch';
import { useScopedSettings, useUpdateScopedSettings } from '~/components/settings/useScopedSettings';
import { Select, SelectItem, SelectPopup, SelectTrigger, SelectValue } from '~/components/ui/select';
import { usePrimaryEnvironment } from '~/state/environments';

export function ThreadsSettingsPanel() {
  const gt = useGT();
  const messages = useMessages();
  const setting = useSearchableSetting();
  const settings = useScopedSettings();
  const updateSettings = useUpdateScopedSettings();
  const config = usePrimaryEnvironment()?.serverConfig;
  const provider = config?.providers.find(item => item.instanceId === settings.defaultModelSelection?.instanceId);
  const modes = runtimeModeOptions.filter(mode => provider?.supportedRuntimeModes?.includes(mode));
  const options = resolveRuntimeModeOptions(modes, config?.managedPolicy, messages);
  const chosen = runtimeModeConfig[settings.defaultRuntimeMode];

  return (
    <SettingsPageContainer>
      <SettingsSection id="project-defaults" title={gt('New chats')}>
        <SettingsRow
          serverScoped
          settingKeys={['defaultRuntimeMode']}
          {...setting('default-permissions')}
          description={gt('Starting permissions for new chats with the default harness.')}
          control={
            <Select
              value={settings.defaultRuntimeMode}
              disabled={options.length === 0}
              onValueChange={mode => { if (mode) updateSettings({ defaultRuntimeMode: mode }); }}
            >
              <SelectTrigger size="sm" aria-label={gt('Default permissions')}>
                <chosen.icon className="size-3.5 shrink-0 text-muted-foreground" />
                <SelectValue>{messages(chosen.label)}</SelectValue>
              </SelectTrigger>
              <SelectPopup align="end" alignItemWithTrigger={false}>
                {options.map(option => (
                  <SelectItem key={option.mode} value={option.mode} disabled={option.disabledReason !== undefined}>
                    <span className="grid gap-0.5">
                      <span className="inline-flex items-center gap-1.5">
                        <option.icon className="size-3.5 shrink-0 text-muted-foreground" />
                        {option.label}
                      </span>
                      <span className="text-xs text-muted-foreground">{option.disabledReason ?? option.description}</span>
                    </span>
                  </SelectItem>
                ))}
              </SelectPopup>
            </Select>
          }
        />
      </SettingsSection>
      <SettingsSection id="snooze" title={gt('Snooze')}>
        <SnoozePresetsRow />
      </SettingsSection>
    </SettingsPageContainer>
  );
}
