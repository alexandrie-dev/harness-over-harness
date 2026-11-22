// Заглушка окна HoH вместо `@t3tools/secret-provider-onepassword/shared/definition` (T3 Code, MIT): секреты и 1Password. Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent, type Anything } from '../../../absent';
export const OnePasswordAccount: Anything = absent('@t3tools/secret-provider-onepassword/shared/definition', 'OnePasswordAccount');
export type OnePasswordAccount<A = any, B = any, C = any, D = any> = any;
export const OnePasswordSecretProviderConfig: Anything = absent('@t3tools/secret-provider-onepassword/shared/definition', 'OnePasswordSecretProviderConfig');
export declare namespace OnePasswordSecretProviderConfig { type fields<A = any, B = any, C = any, D = any> = any; }
export type OnePasswordSecretProviderConfig<A = any, B = any, C = any, D = any> = any;
