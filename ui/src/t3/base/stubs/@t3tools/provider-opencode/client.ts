// Заглушка окна HoH вместо `@t3tools/provider-opencode/client` (T3 Code, MIT): провайдеры T3 (установка, вход, ACP-реестр). Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent, type Anything } from '../../absent';
export const openCodeClient: Anything = absent('@t3tools/provider-opencode/client', 'openCodeClient');
export type openCodeClient<A = any, B = any, C = any, D = any> = any;
