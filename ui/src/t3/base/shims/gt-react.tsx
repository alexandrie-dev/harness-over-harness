// Прослойка окна HoH вместо gt-react (General Translation, i18n T3 Code). Настоящий пакет не ставим (решение CEO):
// без сети. Язык приходит от GTProvider (T3 держит его в web/i18n/runtime.ts); текст — английский исходник T3
// или строка словаря языка (shims/gt-i18n.ts, registerDictionary). <T> показывает свои дети как есть.
import { createContext, useCallback, useContext, type ReactNode } from 'react';

import {
  DEFAULT_LOCALE, decodeMsg, decodeOptions, formatMessage, gtFallback, lookup, msg, resolveMessage,
  type MFunctionType, type Variables,
} from './gt-i18n';

export { decodeMsg, decodeOptions, gtFallback, msg, type MFunctionType };

type Translations = Record<string, Record<string, unknown>>;
const LocaleContext = createContext<string>(DEFAULT_LOCALE);

export function GTProvider({ locale, children }: {
  readonly locale: string;
  readonly translations?: Translations;
  readonly _reload?: (state: { readonly locale: string }) => void;
  readonly children?: ReactNode;
}): ReactNode {
  return <LocaleContext.Provider value={locale}>{children}</LocaleContext.Provider>;
}

/** Настройка GT для T3: загрузчики переводов окно не вызывает (словарь — registerDictionary). */
export async function initializeGTSPA(_options: {
  readonly defaultLocale: string;
  readonly locales: string[];
  readonly locale: string;
  readonly loadTranslations: (locale: string) => Promise<unknown>;
  readonly cacheExpiryTime?: number | null;
  readonly _reload?: (state: { readonly locale: string }) => void;
}): Promise<void> {}

/** Каталог языка у GT; в окне пуст (runtime.ts T3 тогда показывает английский и не кэширует язык). */
export async function getTranslationsSnapshot(_locale: string): Promise<Translations> {
  return {};
}

export function getReactI18nCache(): {
  lookupTranslation: (locale: string, source: string, options?: Variables) => string | undefined;
} {
  return { lookupTranslation: (locale, source) => lookup(locale, source) };
}

export function useLocale(): string {
  return useContext(LocaleContext);
}

export function useGT(): (source: string, options?: Variables) => string {
  const locale = useLocale();
  return useCallback(
    (source: string, options: Variables = {}) => {
      const target = lookup(locale, source);
      return target === undefined ? formatMessage(source, options) : formatMessage(target, options, locale);
    },
    [locale],
  );
}

export function useMessages(): MFunctionType {
  const locale = useLocale();
  return useCallback(
    ((message: string | null | undefined, variables?: Variables) =>
      typeof message === 'string' ? resolveMessage(locale, message, variables) : message) as MFunctionType,
    [locale],
  );
}

export function T({ children }: { readonly children?: ReactNode; readonly context?: string; readonly $context?: string; readonly id?: string }): ReactNode {
  return children;
}

export function Var({ children }: { readonly children?: ReactNode; readonly name?: string }): ReactNode {
  return children;
}

/** Ветка по значению: свойство с именем значения, иначе дети. */
export function Branch({ branch, children, ...branches }: { readonly branch?: string | null; readonly children?: ReactNode; readonly [key: string]: ReactNode | undefined }): ReactNode {
  return branch != null && branch in branches ? branches[branch] : children;
}
