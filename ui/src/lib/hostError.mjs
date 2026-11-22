// Почему запрос к движку не удался — по КОДУ, который отдаёт хост (apps/hoh-desktop/src/errors.rs), а не по словам.
// Команда Tauri отвечает отказом объектом {code, message}; проверки и старые хосты могут отдавать строку или Error.
// Слова человеку пишутся здесь, в одном месте: мастер, проверка Claude, окно защиты и чат берут их отсюда.

export const FOLDER_FIXED_TEXT = 'Чат с Claude работает в одной папке. Чтобы работать в другой, начните новый чат.';

// Строки, которые движок и хост ставили в `message` до появления кодов (engine.rs). Нужны только для хоста без кода.
const UNCODED = new Map([
  ['method-unavailable', 'unavailable'],
  ['Помощник ещё подключается', 'connecting'],
]);

/** @returns {{ code: string, message: string }} код причины (пустой, если хост его не назвал) и слова хоста. */
export function hostError(error) {
  let value = error;
  if (typeof value === 'string' && value.trimStart().startsWith('{')) {
    try { value = JSON.parse(value); } catch { /* остаётся строкой */ }
  }
  let code = ''; let message = '';
  if (typeof value === 'string') message = value;
  else if (value && typeof value === 'object') {
    if (typeof value.code === 'string') code = value.code;
    if (typeof value.message === 'string') message = value.message;
  }
  if (!code) code = UNCODED.get(message.trim()) ?? '';
  return { code, message: message.trim() };
}

/** Слова человеку по коду отказа; пусто, если код неизвестен (тогда вызывающий выбирает свои слова). */
export function refusalWords(error, { what = 'это действие', button = 'Дальше' } = {}) {
  switch (hostError(error).code) {
    case 'rejected': return `Вы не разрешили ${what}. Нажмите «${button}» и выберите «Разрешить один раз».`;
    case 'expired': return `Время на ответ вышло. Нажмите «${button}» ещё раз.`;
    case 'folder-fixed': return FOLDER_FIXED_TEXT;
    default: return '';
  }
}

/** Русские слова самого хоста, если они есть и короткие (чтобы не показывать чужую техническую строку). */
export function hostWords(error) {
  const { message } = hostError(error);
  return /[А-Яа-яЁё]/.test(message) && message.length < 160 ? message : '';
}

/** Общий текст для окна: слова по коду, потом русские слова хоста, потом запасной. */
export function errorText(error, fallback, options) {
  return refusalWords(error, options) || hostWords(error) || fallback;
}
