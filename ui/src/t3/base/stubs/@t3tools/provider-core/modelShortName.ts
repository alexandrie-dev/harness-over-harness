// Заглушка окна HoH вместо `@t3tools/provider-core/modelShortName` (T3 Code, MIT): провайдеры T3 (установка, вход, ACP-реестр). Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent, type Anything } from '../../absent';
export const modelShortName: Anything = absent('@t3tools/provider-core/modelShortName', 'modelShortName');
export type modelShortName<A = any, B = any, C = any, D = any> = any;
