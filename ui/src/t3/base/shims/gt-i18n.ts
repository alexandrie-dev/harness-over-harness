// Прослойка окна HoH вместо gt-i18n (General Translation, i18n T3 Code). Настоящий пакет не ставим (решение CEO):
// без сети и без облака GT. Тексты T3 — английские исходники с ICU-разметкой ({n, plural, ...}); прослойка
// подставляет переменные сама. Перевод — по словарю «английский исходник -> текст» на язык (registerDictionary);
// по умолчанию словарей нет, и окно показывает строки T3 как есть. Пути подключения: src/t3/base/aliases.json и
// tsconfig.json (scripts/t3-sync.py, FORCE_STUB в scripts/t3graph.py).

export type Variables = Record<string, unknown>;
/** Сообщение после msg(): английская строка (с подставленными переменными, если они были). */
export type MFunctionType = <M extends string | null | undefined>(message: M, variables?: Variables) => M extends string ? string : M;

export const DEFAULT_LOCALE = 'en';
const dictionaries = new Map<string, Readonly<Record<string, string>>>();

/** Словарь языка: английский исходник T3 -> текст на этом языке (например, собранный из ru.json T3). */
export function registerDictionary(locale: string, dictionary: Readonly<Record<string, string>>): void {
  dictionaries.set(locale, dictionary);
}

export function lookup(locale: string, source: string): string | undefined {
  return locale === DEFAULT_LOCALE ? undefined : dictionaries.get(locale)?.[source];
}

// ───── ICU MessageFormat: аргументы, plural/selectordinal, select, number, date/time ─────

function variable(values: Variables, name: string): unknown {
  return Object.prototype.hasOwnProperty.call(values, name) ? values[name] : undefined;
}

/** Разбирает текст до закрывающей скобки уровня (или до конца); возвращает строку и позицию после неё. */
function formatPart(text: string, start: number, values: Variables, locale: string, pound: string | null): [string, number] {
  let out = '';
  let i = start;
  while (i < text.length) {
    const c = text[i]!;
    if (c === '}') return [out, i];
    if (c === "'") {
      const next = text[i + 1];
      if (next === "'") { out += "'"; i += 2; continue; }
      if (next === '{' || next === '}' || (pound !== null && next === '#')) {
        const end = text.indexOf("'", i + 1);
        out += text.slice(i + 1, end < 0 ? text.length : end);
        i = end < 0 ? text.length : end + 1;
        continue;
      }
      out += c; i += 1; continue;
    }
    if (c === '#' && pound !== null) { out += pound; i += 1; continue; }
    if (c === '{') {
      const [piece, after] = formatArgument(text, i + 1, values, locale, pound);
      out += piece; i = after; continue;
    }
    out += c; i += 1;
  }
  return [out, i];
}

/** Пропускает вложенный блок {…}, начиная после открывающей скобки; возвращает позицию после закрывающей. */
function skipBlock(text: string, start: number): number {
  let depth = 1;
  for (let i = start; i < text.length; i += 1) {
    if (text[i] === '{') depth += 1;
    else if (text[i] === '}' && --depth === 0) return i + 1;
  }
  throw new Error('unbalanced');
}

function formatArgument(text: string, start: number, values: Variables, locale: string, pound: string | null): [string, number] {
  const head = /^\s*([^\s,{}]+)\s*(?:,\s*(\w+)\s*(?:,)?)?/.exec(text.slice(start));
  if (!head) throw new Error('bad argument');
  const [whole, name = '', type] = head;
  let i = start + whole.length;
  const value = variable(values, name);
  if (!type) {
    const end = text.indexOf('}', i);
    if (end < 0) throw new Error('unbalanced');
    return [value === undefined ? `{${name}}` : String(value), end + 1];
  }
  if (type === 'plural' || type === 'selectordinal' || type === 'select') {
    const options = new Map<string, number>();
    const offsetMatch = /^\s*offset:\s*(\d+)/.exec(text.slice(i));
    const offset = offsetMatch ? Number(offsetMatch[1]) : 0;
    if (offsetMatch) i += offsetMatch[0].length;
    for (;;) {
      const m = /^\s*([^\s{}]+)\s*\{/.exec(text.slice(i));
      if (!m) break;
      options.set(m[1]!, i + m[0].length);
      i = skipBlock(text, i + m[0].length);
    }
    const end = text.indexOf('}', i);
    if (end < 0) throw new Error('unbalanced');
    let chosen: number | undefined;
    let nextPound = pound;
    if (type === 'select') {
      chosen = options.get(String(value)) ?? options.get('other');
    } else {
      const n = Number(value) - offset;
      nextPound = Number.isFinite(n) ? new Intl.NumberFormat(locale).format(n) : String(value);
      const rule = new Intl.PluralRules(locale, { type: type === 'plural' ? 'cardinal' : 'ordinal' }).select(n);
      chosen = options.get(`=${Number(value)}`) ?? options.get(rule) ?? options.get('other');
    }
    if (chosen === undefined) return ['', end + 1];
    return [formatPart(text, chosen, values, locale, nextPound)[0], end + 1];
  }
  const end = skipBlock(text, i) - 1; // стиль (`number, percent`) до закрывающей скобки
  const style = text.slice(i, end).trim();
  if (value === undefined) return [`{${name}}`, end + 1];
  if (type === 'number') {
    const options: Intl.NumberFormatOptions = style === 'percent' ? { style: 'percent' } : style === 'integer' ? { maximumFractionDigits: 0 } : {};
    return [new Intl.NumberFormat(locale, options).format(Number(value)), end + 1];
  }
  if (type === 'date' || type === 'time') {
    const when = value instanceof Date ? value : new Date(value as string | number);
    const size = (['short', 'medium', 'long', 'full'] as const).find((s) => s === style) ?? 'medium';
    const options: Intl.DateTimeFormatOptions = type === 'date' ? { dateStyle: size } : { timeStyle: size };
    return [new Intl.DateTimeFormat(locale, options).format(when), end + 1];
  }
  return [String(value), end + 1];
}

/** Подставляет переменные в ICU-строку. Нераспознанная разметка — строка как есть. */
export function formatMessage(text: string, values: Variables = {}, locale: string = DEFAULT_LOCALE): string {
  if (!text.includes('{') && !text.includes("'")) return text;
  try {
    let out = '';
    let i = 0;
    while (i < text.length) {
      const [piece, end] = formatPart(text, i, values, locale, null);
      out += piece;
      if (end >= text.length) break;
      out += '}'; i = end + 1; // лишняя закрывающая скобка — как текст
    }
    return out;
  } catch {
    return text;
  }
}

// ───── msg(): исходник с переменными помнится, чтобы decodeOptions/decodeMsg работали, как у GT ─────

const sources = new Map<string, Variables & { $_source: string }>();

function hasVariables(options: Variables): boolean {
  return Object.keys(options).some((key) => !key.startsWith('$'));
}

/** Помечает исходник для перевода; с переменными — сразу английский текст с подстановкой (исходник помнится). */
export function msg<S extends string>(source: S, options?: Variables): S {
  if (!options || !hasVariables(options)) return source;
  const english = formatMessage(source, options, DEFAULT_LOCALE);
  sources.set(english, { ...options, $_source: source });
  return english as S;
}

export function decodeOptions(message: string): (Variables & { $_source: string }) | null {
  return sources.get(message) ?? null;
}

/** Английский текст сообщения. */
export function decodeMsg(message: string): string {
  return message;
}

/** Текст сообщения на языке `$locale` (по умолчанию английский) с подстановкой переменных. */
export function gtFallback(message: string, options: Variables = {}): string {
  const locale = typeof options.$locale === 'string' ? options.$locale : DEFAULT_LOCALE;
  const fallback = typeof options.$_fallback === 'string' ? options.$_fallback : message;
  return formatMessage(message, options, locale) || fallback;
}

export function resolveMessage(locale: string, message: string, variables: Variables = {}): string {
  const decoded = decodeOptions(message);
  const source = decoded?.$_source ?? message;
  const values = decoded ?? variables;
  const target = lookup(locale, source);
  return target === undefined ? formatMessage(source, values, DEFAULT_LOCALE) : formatMessage(target, values, locale);
}

/** Сообщение на английском (у GT — на языке по умолчанию). */
export const mFallback = ((message: string | null | undefined, variables?: Variables) =>
  typeof message === 'string' ? resolveMessage(DEFAULT_LOCALE, message, variables) : message) as MFunctionType;
