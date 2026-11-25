// What a failed engine request means, in T3's English. The Tauri host refuses with a CODE next to its own
// (Russian) words (apps/hoh-desktop/src/errors.rs, read the same way as ui/src/lib/hostError.mjs); T3's
// interface is English, so the words come from the code here, never from the host's text.

const CYRILLIC = /[Ѐ-ӿ]/;

/** `{ code, message }` of a refusal: an object, its JSON, a plain string or an Error. */
export function hostFailure(error: unknown): { code: string; message: string } {
  let value: unknown = error instanceof Error ? error.message : error;
  if (typeof value === 'string' && value.trimStart().startsWith('{')) {
    try { value = JSON.parse(value); } catch { /* stays a string */ }
  }
  if (typeof value === 'string') return { code: '', message: value.trim() };
  if (value && typeof value === 'object') {
    const { code, message } = value as { code?: unknown; message?: unknown };
    return { code: typeof code === 'string' ? code : '', message: typeof message === 'string' ? message.trim() : '' };
  }
  return { code: '', message: '' };
}

const BY_CODE: Readonly<Record<string, string>> = {
  rejected: 'You declined this in the HoH confirmation window.',
  expired: 'The confirmation was not answered in time. Try again.',
  connecting: 'HoH is still starting. Try again in a moment.',
  unavailable: 'Not available in HoH.',
  'folder-fixed': 'A Claude chat works in one folder. Start a new chat to work in another.',
  'folder-changed': 'The folder for this chat has changed. Rebind the chat to this folder to continue.',
  refused: 'HoH refused this request.',
};

/** The English words for a failed request: by its code, else its own text when that is English. */
export function englishError(error: unknown, fallback = 'The request failed.'): string {
  const { code, message } = hostFailure(error);
  if (BY_CODE[code]) return BY_CODE[code];
  return message && !CYRILLIC.test(message) ? message : fallback;
}

/** T3's provider failure (`OrchestrationV2ProviderFailure`) for an error of the engine or the host. */
export function providerFailure(error: unknown, failureClass: string, fallback: string) {
  const { code } = hostFailure(error);
  return {
    class: failureClass,
    message: englishError(error, fallback).slice(0, 4096),
    code: code ? code.slice(0, 128) : null,
    retryable: null,
  };
}
