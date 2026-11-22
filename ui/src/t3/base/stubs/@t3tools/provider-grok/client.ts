// Заглушка окна HoH вместо `@t3tools/provider-grok/client` (T3 Code, MIT): провайдеры T3 (установка, вход, ACP-реестр). Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent, type Anything } from '../../absent';
export const grokClient: Anything = absent('@t3tools/provider-grok/client', 'grokClient');
export type grokClient<A = any, B = any, C = any, D = any> = any;
