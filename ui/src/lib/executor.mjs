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
  if (result?.auth === 'unauthenticated') return { ...base, ready: false, tone: 'bad', headline: 'Claude установлен, но в него не вошли. Откройте Claude, войдите и нажмите «Проверить ещё раз».' };
  if (result?.auth !== 'authenticated') return { ...base, ready: false, tone: 'bad', headline: 'Claude установлен, но не удалось узнать, вошли ли в него. Откройте Claude, войдите и нажмите «Проверить ещё раз».' };
  const warning = result?.versionStatus === 'supported' ? ''
    : result?.versionStatus === 'unsupported' ? 'Эта версия Claude не поддерживается: что-то может не работать. Лучше обновить Claude.'
      : 'Не удалось проверить, подходит ли эта версия Claude. Работа возможна, но без гарантий.';
  return { ...base, ready: true, tone: warning ? 'warn' : 'ok', headline: 'Готово. Ничего вводить не нужно.', warning };
}

/** Подпись «Полного доступа» и текст окна первого включения: слова CEO, один источник. */
export const FULL_ACCESS_DESCRIPTION = 'Помощник сам удаляет и меняет файлы в любых папках, куда есть доступ у вас, запускает программы и выходит в интернет, не спрашивая. Сделанное нельзя отменить.';
