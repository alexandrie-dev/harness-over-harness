import { useLocale } from 'gt-react';
import {
  VoiceInputController,
  VoiceTranscriptionError,
  voiceInputBlocksSubmission,
  type VoiceInputState,
} from '@t3tools/client-runtime/voice-input';
import { useNavigate } from '@tanstack/react-router';
import { useLayoutEffect, useState } from 'react';
import { useStableCallback } from '~/hooks/useStableCallback';
import { m } from '~/i18n/messages';
import { createBrowserPcmCapture } from '~/speech/browserPcmCapture';
import { useEnvironmentSpeechInput } from '~/speech/useEnvironmentSpeechInput';
import { encodeWhisperWav } from '../../whisper/Wav';
import { transcribeWhisperRecording, useWhisperServer, whisperFailureMessage } from '../../whisper/client';

type SpeechInputOptions = Parameters<typeof useEnvironmentSpeechInput>[0];
const idle: VoiceInputState = { phase: 'idle', error: null, errorAction: null };

export function useHohComposerSpeechInput(input: SpeechInputOptions) {
  const fallback = useEnvironmentSpeechInput(input);
  const navigate = useNavigate();
  const serverUrl = useWhisperServer();
  const russian = useLocale() === 'ru';
  const active = !input.disabled && input.presented !== false;
  const [state, setState] = useState(idle);
  const [level, setLevel] = useState(0);
  const [recordingStartedAt, setRecordingStartedAt] = useState<number | null>(null);
  const readDraft = useStableCallback(() => {
    const draft = active && input.readDraft();
    if (!draft) return null;
    // Dictation appends without replacing a selection in the draft.
    const end = draft.text.length;
    return { ownerKey: input.ownerKey, text: draft.text, selection: { start: end, end } };
  });
  const commitDraft = useStableCallback(input.commitDraft);
  const settings = useStableCallback(() => ({ active, serverUrl, russian }));

  const [controller] = useState(() => {
    const platform = createBrowserPcmCapture({
      getMicrophoneId: () => '',
      onLevel: setLevel,
      onDurationLimit: () => void voice.stop(),
      onError: (message, code) => void voice.interruptRecording(message, null, code),
    }, async () => {
      const { serverUrl, russian } = settings();
      if (!serverUrl || !navigator.mediaDevices?.getUserMedia || typeof AudioWorkletNode === 'undefined') {
        throw new VoiceTranscriptionError({
          code: 'preparation-failed',
          message: russian ? 'Микрофон недоступен в этом окне.' : 'Microphone capture is unavailable in this window.',
        });
      }
      const chunks: Float32Array[] = [];
      const finish = async ({ signal }: { signal: AbortSignal }) => {
        signal.throwIfAborted();
        if (!chunks.length) return '';
        try {
          const result = await transcribeWhisperRecording(serverUrl, encodeWhisperWav(chunks));
          signal.throwIfAborted();
          if (result.kind === 'error') throw new Error(whisperFailureMessage(result.failure, russian));
          if (result.kind === 'transcript') return result.text;
        } catch (cause) {
          signal.throwIfAborted();
          const message = cause instanceof Error ? cause.message
            : russian ? 'Не удалось получить ответ Whisper.' : 'Could not get a response from Whisper.';
          throw new VoiceTranscriptionError({ code: 'transcription-failed', message });
        } finally {
          chunks.length = 0;
        }
        throw new VoiceTranscriptionError({
          code: 'transcription-failed',
          message: russian ? 'Whisper вернул некорректный ответ.' : 'Whisper returned an invalid response.',
        });
      };
      // Keep PCM in memory until Stop. The daemon accepts a complete WAV in one request.
      return {
        locale: 'und',
        transcribe: (_uri, options) => finish(options),
        streaming: { feed: pcm => { chunks.push(pcm); }, finish },
      };
    });
    const voice = new VoiceInputController({
      resolve: m,
      recorder: platform.recorder,
      getTranscriber: () => platform.transcriber,
      requestPermission: async () => ({ granted: true, canAskAgain: true }),
      configureRecording: async () => undefined,
      releaseRecording: async () => platform.cancelRecording(),
      deleteRecording: platform.deleteRecording,
      readDraft,
      commitDraft,
      onStateChange: next => {
        setState(next);
        setRecordingStartedAt(next.phase === 'recording' ? Date.now() : null);
      },
    });
    return voice;
  });

  // Leaving the originating chat or changing the destination abandons the active take.
  useLayoutEffect(() => () => controller.dispose(), [active, controller, input.ownerKey, serverUrl]);
  const start = useStableCallback(async () => {
    const options = settings();
    if (!options.active) return;
    if (options.serverUrl === null) {
      await navigate({ to: '/settings/chat', hash: 'hoh-whisper-settings' });
    } else if (options.serverUrl) {
      await controller.start();
    }
  });
  const stop = useStableCallback(() => controller.stop());
  const cancel = useStableCallback(() => controller.cancel());
  const finishWithoutSending = useStableCallback(() => { void controller.stop(); });
  const skipPostProcessing = useStableCallback(() => {});

  if (serverUrl === undefined) return { ...fallback, available: false, whisperError: null };
  const busy = voiceInputBlocksSubmission(state);
  return {
    ...fallback,
    setup: { ...fallback.setup, open: false },
    available: true,
    state,
    preview: null,
    whisperError: state.phase === 'error' ? state.error : null,
    level,
    recordingStartedAt,
    preparing: state.phase === 'preparing',
    blocksSubmission: busy,
    freezesEditor: busy,
    start,
    stop,
    cancel,
    submitAfterDictation: finishWithoutSending,
    skipPostProcessing,
  };
}
