import { useSyncExternalStore } from 'react';
import type { HohEngine } from '../../hoh-host/engine';
import type { WhisperDictationFailure, WhisperDictationParams, WhisperDictationResult } from '../../protocol';

const METHOD = 'hoh/voice/whisper';

// undefined means the candidate is unavailable, null means it has no saved address.
let serverUrl: string | null | undefined;
let change = 0;
let engine: Pick<HohEngine, 'request' | 'subscribeStatus'>;
const listeners = new Set<() => void>();

function publish(url: typeof serverUrl) {
  if (serverUrl === url) return;
  serverUrl = url;
  listeners.forEach(listener => listener());
}

export function installWhisperClient(host: typeof engine) {
  engine = host;
  async function refresh() {
    const request = ++change;
    try {
      const result: WhisperDictationResult = await host.request(METHOD, { action: 'getSettings' });
      if (request === change && result?.kind === 'settings') publish(result.serverUrl ?? null);
    } catch (error) {
      if (request === change && typeof error === 'object' && error !== null &&
          'code' in error && error.code === 'unavailable') publish(undefined);
    }
  }
  // Subscribe before probing so a slow engine startup also refreshes the setting.
  if (host.subscribeStatus) {
    void host.subscribeStatus(status => {
      if (status.state === 'ready') void refresh();
    }).then(refresh, refresh);
  } else {
    void refresh();
  }
}

export function useWhisperServer() {
  return useSyncExternalStore(
    listener => {
      listeners.add(listener);
      return () => { listeners.delete(listener); };
    },
    () => serverUrl,
  );
}

async function request(params: WhisperDictationParams): Promise<WhisperDictationResult> {
  const result: WhisperDictationResult = await engine.request(METHOD, params);
  return result;
}

export async function saveWhisperServer(url: string) {
  const result = await request({ action: 'setSettings', serverUrl: url });
  if (result.kind === 'settings') {
    change++;
    publish(result.serverUrl ?? null);
  }
  return result;
}

export function transcribeWhisperRecording(url: string, wavBase64: string) {
  return request({ action: 'transcribe', serverUrl: url, wavBase64 });
}

export function whisperFailureMessage(failure: WhisperDictationFailure, russian: boolean) {
  switch (failure.code) {
    case 'invalidAddress':
      return russian
        ? 'Укажите HTTP или HTTPS адрес без логина, пароля, параметров и фрагмента.'
        : 'Enter an HTTP or HTTPS address without credentials, a query or a fragment.';
    case 'saveFailed':
      return russian ? 'Не удалось сохранить адрес Whisper.' : 'Could not save the Whisper address.';
    case 'addressChanged':
      return russian ? 'Адрес Whisper изменился. Начните запись заново.' : 'The Whisper address changed. Start a new recording.';
    case 'invalidAudio':
      return russian ? 'Whisper не получил пригодную для распознавания запись.' : 'The recording could not be sent to Whisper.';
    case 'unreachable':
      return russian ? 'Whisper недоступен. Проверьте адрес и запущен ли сервер.' : 'Could not reach Whisper. Check the address and make sure the server is running.';
    case 'timeout':
      return russian ? 'Whisper не ответил вовремя.' : 'Whisper did not respond in time.';
    case 'serverRejected':
      return russian ? `Сервер Whisper ответил HTTP ${failure.status}.` : `Whisper returned HTTP ${failure.status}.`;
    case 'invalidResponse':
      return russian ? 'Whisper вернул некорректный ответ.' : 'Whisper returned an invalid response.';
  }
}
