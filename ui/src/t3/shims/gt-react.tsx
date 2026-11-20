// Прослойка окна HoH вместо gt-react (i18n T3 Code, MIT): тексты T3 написаны по-английски,
// окно показывает русский. Функции возвращают русский текст из словаря; незнакомая строка
// остаётся как есть (так её видно при проверке и можно добавить в словарь).
import type { ReactNode } from 'react';
import { RU } from './ru';

type Values = Record<string, unknown>;
type Options = Values & { $context?: string; $id?: string };
/** Результат msg(): в T3 — помеченная строка, у нас — сама строка. */
export type Message = string;

function fill(text: string, values?: Values): string {
  return values ? text.replace(/\{(\w+)\}/g, (whole, name: string) => (name in values ? String(values[name]) : whole)) : text;
}
export function translate(text: string, options?: Options): string {
  return fill(RU[text] ?? text, options);
}
export function msg(text: string, _options?: Options): Message { return text; }
export function useGT(): (text: string, options?: Options) => string { return translate; }
export function getGT(): (text: string, options?: Options) => string { return translate; }
export function useMessages(): (message: Message, options?: Options) => string { return (message, options) => translate(message, options); }
export function useTranslations(): (text: string, options?: Options) => string { return translate; }
export function T({ children }: { children?: ReactNode; context?: string; id?: string }): ReactNode { return children; }
export function Var({ children }: { children?: ReactNode }): ReactNode { return children; }
export const Num = Var; export const Currency = Var; export const DateTime = Var;
export function Plural({ children }: { children?: ReactNode; n?: number; [form: string]: unknown }): ReactNode { return children; }
export function Branch({ children }: { children?: ReactNode; branch?: string; [key: string]: unknown }): ReactNode { return children; }
