// Claude как поставщик: единственное место, где результат `hoh/executor/discover` превращается в слова для человека.
// Окно не читает файлы Claude и не ищет программу само: всё, что оно знает, пришло из discover.

/** Умолчание режима доступа Claude в окне — единственное место. Движок сообщает действующий режим сам (`effective`); это значение берётся, только если он не назвал режим. */
export const DEFAULT_ACCESS_MODE = 'confirm';


/** Подпись модели Claude: displayName движка, иначе id; «default» — «По умолчанию». */
export function claudeModelName(model) {
  const name = typeof model?.displayName === 'string' ? model.displayName.trim() : '';
  if (name) return name;
  const id = typeof model?.id === 'string' ? model.id.trim() : '';
  return id === 'default' ? 'По умолчанию' : id;
}

/** Подпись по id среди известных моделей (для нижней строки поля ввода). */
export function claudeModelLabel(id, models = []) {
  const found = models.find(model => model.id === id);
  return found ? (found.name || claudeModelName(found)) : claudeModelName({ id });
}

/**
 * Что сказать человеку про ответ discover.
 * @returns {{ ready: boolean, tone: 'ok'|'warn'|'bad', headline: string, hint: string, warning: string, version: string,
 *   models: { id: string, name: string }[], defaultModel: string }}
 */
export function discoverView(result) {
  const installed = result?.installed === true;
  const version = installed && typeof result.version === 'string' ? result.version.trim() : '';
  const models = (Array.isArray(result?.models) ? result.models : [])
    .filter(model => model && typeof model.id === 'string' && model.id.trim())
    .map(model => ({ id: model.id.trim(), name: claudeModelName(model), isDefault: model.isDefault === true }));
  const defaultModel = (models.find(model => model.isDefault) ?? models[0])?.id ?? '';
  const base = { version, models: models.map(({ id, name }) => ({ id, name })), defaultModel, warning: '', hint: '' };
  if (!installed) return { ...base, ready: false, tone: 'bad', headline: 'Claude на этом компьютере не установлен. Попросите того, кто настраивал вам компьютер, или выберите другой способ выше.' };
  // The version first: an engine that refuses the version does not look at the sign-in (`auth: unknown`), and the person must be told to update, not to sign in.
  if (result?.versionStatus === 'unsupported') return { ...base, ready: false, tone: 'bad', headline: UNSUPPORTED_VERSION_TEXT };
  // A version the engine could not recognise is refused at the start of a chat, like an unsupported one.
  if (result?.versionStatus !== 'supported') return { ...base, ready: false, tone: 'bad', headline: UNKNOWN_VERSION_TEXT };
  if (result?.auth === 'unauthenticated') return { ...base, ready: false, tone: 'bad', headline: 'Claude установлен, но в него не вошли. Откройте Claude, войдите и нажмите «Проверить ещё раз».' };
  if (result?.auth !== 'authenticated') return { ...base, ready: false, tone: 'bad', headline: 'Claude установлен, но не удалось узнать, вошли ли в него. Откройте Claude, войдите и нажмите «Проверить ещё раз».' };
  return { ...base, ready: true, tone: 'ok', headline: 'Готово. Ничего вводить не нужно.', warning: '' };
}

/** Подпись «Полного доступа» и текст окна первого включения: слова CEO, один источник. */
export const FULL_ACCESS_DESCRIPTION = 'Помощник сам удаляет и меняет файлы в любых папках, куда есть доступ у вас, запускает программы и выходит в интернет, не спрашивая. Сделанное нельзя отменить.';

/** Подпись режима «Спрашивать разрешение»: первая фраза верна всегда. */
export const CONFIRM_ACCESS_LEAD = 'Помощник спрашивает перед изменениями.';
/** Вторая фраза: только когда движок запустил Claude с настройками самого человека (`--setting-sources` назвал `user`). */
export const CONFIRM_PERSONAL_NOTE = 'Действуют и ваши собственные настройки Claude: их разрешающие правила могут пропускать действия без вопроса.';
/** Подпись целиком — для мастера, где режим ещё не запущен и действует умолчание (настройки человека). */
export const CONFIRM_ACCESS_DESCRIPTION = `${CONFIRM_ACCESS_LEAD} ${CONFIRM_PERSONAL_NOTE}`;

/** Подпись режима «Спрашивать разрешение» по тому, как движок запустил Claude на самом деле. */
export function confirmDescription(flags) {
  return personalSettingsApply(flags) ? CONFIRM_ACCESS_DESCRIPTION : CONFIRM_ACCESS_LEAD;
}

/** Отказ для версии, которую движок не распознал (старт чата её отклоняет). */
export const UNKNOWN_VERSION_TEXT = 'Не удалось определить версию Claude, поэтому работа с ним не начнётся. Нужна версия 2.x — обновите Claude и нажмите «Проверить ещё раз».';

/** Отказ для версии Claude, которую движок не запускает (UNSUPPORTED_VERSION). */
export const UNSUPPORTED_VERSION_TEXT = 'Эта версия Claude не поддерживается. Нужна версия 2.x — обновите Claude и нажмите «Проверить ещё раз».';

/** Строка у «Отвечает Claude»: движок не берёт настройки Claude из папки проекта. */
export const PROJECT_SETTINGS_OFF_NOTE = 'Настройки Claude из папки проекта не применяются';

/** Источники настроек, с которыми движок на самом деле запустил Claude (`--setting-sources` в `effective.flags`): массив имён; null, если флага нет — движок ничего не сказал. */
export function settingSources(flags) {
  if (!Array.isArray(flags)) return null;
  const at = flags.findIndex(flag => flag === '--setting-sources' || (typeof flag === 'string' && flag.startsWith('--setting-sources=')));
  if (at < 0) return null;
  const value = flags[at] === '--setting-sources' ? flags[at + 1] : flags[at].slice('--setting-sources='.length);
  if (typeof value !== 'string') return null;
  return value.split(',').map(part => part.trim()).filter(Boolean);
}

/** true, если среди источников нет `project` (флаг есть). */
export function projectSettingsIgnored(flags) {
  const sources = settingSources(flags);
  return sources !== null && !sources.includes('project');
}

/** true, если Claude запущен с настройками самого человека (источник `user`). */
export function personalSettingsApply(flags) {
  return settingSources(flags)?.includes('user') === true;
}

/** Постоянная строка у чипа режима: выбранный режим пойдёт со следующего сообщения, процесс этого разговора пока идёт в прежнем. */
export const ACCESS_PENDING_NOTE = 'Вступит в силу со следующего сообщения';
