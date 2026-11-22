// Заглушка окна HoH вместо `@t3tools/provider-muse/client` (T3 Code, MIT): провайдеры T3 (установка, вход, ACP-реестр). Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent, type Anything } from '../../absent';
export const museClient: Anything = absent('@t3tools/provider-muse/client', 'museClient');
export type museClient<A = any, B = any, C = any, D = any> = any;
