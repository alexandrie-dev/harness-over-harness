// Заглушка окна HoH вместо `@t3tools/secret-provider-definitions/registry` (T3 Code, MIT): секреты и 1Password. Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent, type Anything } from '../../absent';
export const builtInSecretProviderDefinitions: Anything = absent('@t3tools/secret-provider-definitions/registry', 'builtInSecretProviderDefinitions');
export declare namespace builtInSecretProviderDefinitions { type definitions<A = any, B = any, C = any, D = any> = any; type find<A = any, B = any, C = any, D = any> = any; }
export type builtInSecretProviderDefinitions<A = any, B = any, C = any, D = any> = any;
