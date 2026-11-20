// Прослойка окна HoH вместо apps/web/src/i18n/messages.ts (T3 Code, MIT): русский текст из словаря.
import { translate, type Message } from '../../shims/gt-react';

export type MessageResolver = (message: Message, options?: Record<string, unknown>) => string;
export const m: MessageResolver = (message, options) => translate(message, options);
export function getActiveLocale(): string { return 'ru'; }
