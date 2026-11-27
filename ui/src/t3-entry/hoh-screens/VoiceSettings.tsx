import type { VoiceShortcutMode } from '@t3tools/contracts';
import { useGT } from 'gt-react';
import { RefreshCwIcon } from 'lucide-react';
import { useEffect, useState } from 'react';
import { MicrophoneOrderList } from '~/components/settings/MicrophoneOrderList';
import { MicrophoneTest } from '~/components/settings/MicrophoneTest';
import { SettingsPageContainer, SettingsRow, SettingsSection } from '~/components/settings/settingsLayout';
import { useSearchableSetting } from '~/components/settings/settingsSearch';
import { Button } from '~/components/ui/button';
import { Select, SelectItem, SelectPopup, SelectTrigger, SelectValue } from '~/components/ui/select';
import { updateClientSettings, useClientSettings } from '~/hooks/useSettings';
import { useStableCallback } from '~/hooks/useStableCallback';
import { listAudioInputs, updateMicrophoneOrder, type BrowserMicrophone } from '~/speech/browserMicrophones';
import { microphoneOrder, preferredMicrophoneId, rememberMicrophones } from '~/speech/microphoneOrder';
import { WhisperSetting } from './whisper/WhisperSetting';

export function VoiceSettingsPanel() {
  const gt = useGT();
  const setting = useSearchableSetting();
  const settings = useClientSettings();
  const [microphones, setMicrophones] = useState<BrowserMicrophone[]>([]);
  const refreshMicrophones = useStableCallback(async () => {
    try {
      const devices = await listAudioInputs();
      setMicrophones(devices);
      updateMicrophoneOrder(order => rememberMicrophones(order, devices));
    } catch {
      setMicrophones([]);
    }
  });
  useEffect(() => {
    const mediaDevices = navigator.mediaDevices;
    void refreshMicrophones();
    mediaDevices?.addEventListener('devicechange', refreshMicrophones);
    return () => mediaDevices?.removeEventListener('devicechange', refreshMicrophones);
  }, [refreshMicrophones]);

  const order = microphoneOrder(settings);
  const microphoneId = preferredMicrophoneId(order, microphones);
  const shortcutModes: Record<VoiceShortcutMode, string> = {
    auto: gt('Automatic'),
    hold: gt('Hold to record'),
    toggle: gt('Press to start and stop'),
  };
  return (
    <SettingsPageContainer>
      <SettingsSection id="voice-input" title={gt('Input')}>
        <SettingsRow
          {...setting('voice-microphone')}
          description={gt('Voice input uses the first connected microphone in this list.')}
          control={
            <Button size="icon-sm" variant="outline" onClick={refreshMicrophones} aria-label={gt('Refresh microphones')}>
              <RefreshCwIcon />
            </Button>
          }
        >
          <div className="mt-3 pb-2">
            <MicrophoneOrderList order={order} devices={microphones} />
          </div>
        </SettingsRow>
        <MicrophoneTest key={microphoneId} microphoneId={microphoneId} onMicrophoneAccess={refreshMicrophones} />
        <SettingsRow
          {...setting('dictation-shortcut-mode')}
          control={
            <Select
              value={settings.voiceShortcutMode}
              onValueChange={value => { if (value) void updateClientSettings({ voiceShortcutMode: value }); }}
            >
              <SelectTrigger size="sm" aria-label={gt('Dictation shortcut mode')}>
                <SelectValue>{shortcutModes[settings.voiceShortcutMode]}</SelectValue>
              </SelectTrigger>
              <SelectPopup align="end" alignItemWithTrigger={false}>
                {(Object.keys(shortcutModes) as VoiceShortcutMode[]).map(mode => (
                  <SelectItem key={mode} value={mode}>{shortcutModes[mode]}</SelectItem>
                ))}
              </SelectPopup>
            </Select>
          }
        />
      </SettingsSection>
      <WhisperSetting />
    </SettingsPageContainer>
  );
}
