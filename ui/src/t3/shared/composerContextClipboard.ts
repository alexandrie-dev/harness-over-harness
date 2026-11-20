// Прослойка окна HoH вместо packages/shared/src/composerContextClipboard.ts (T3 Code, MIT): копирование с контекстом поля ввода T3 не нужно; копируется обычный текст.
export const COMPOSER_CONTEXT_CLIPBOARD_MIME = 'application/x-hoh-composer-context';
export function encodeComposerContextClipboardHtml(_text: string, _fragment: string, html: string): string { return html; }
