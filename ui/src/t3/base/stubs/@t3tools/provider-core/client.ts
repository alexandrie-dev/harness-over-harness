// Заглушка окна HoH вместо `@t3tools/provider-core/client` (T3 Code, MIT): провайдеры T3 (установка, вход, ACP-реестр). Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent, type Anything } from '../../absent';
export const ProviderClientDefinition: Anything = absent('@t3tools/provider-core/client', 'ProviderClientDefinition');
export type ProviderClientDefinition<A = any, B = any, C = any, D = any> = any;
export const ProviderEnvironmentField: Anything = absent('@t3tools/provider-core/client', 'ProviderEnvironmentField');
export type ProviderEnvironmentField<A = any, B = any, C = any, D = any> = any;
export const ProviderIcon: Anything = absent('@t3tools/provider-core/client', 'ProviderIcon');
export type ProviderIcon<A = any, B = any, C = any, D = any> = any;
export const makeProviderClientRegistry: Anything = absent('@t3tools/provider-core/client', 'makeProviderClientRegistry');
export type makeProviderClientRegistry<A = any, B = any, C = any, D = any> = any;
