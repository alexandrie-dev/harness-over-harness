// Заглушка окна HoH вместо `@t3tools/source-control-definitions/registry` (T3 Code, MIT): git и source control. Пакет не ставится; файл пишет scripts/t3-sync.py.
import { absent, type Anything } from '../../absent';
export const builtInSourceControlDefinitions: Anything = absent('@t3tools/source-control-definitions/registry', 'builtInSourceControlDefinitions');
export type builtInSourceControlDefinitions<A = any, B = any, C = any, D = any> = any;
export const canonicalizeGitCafePullRequestUrl: Anything = absent('@t3tools/source-control-definitions/registry', 'canonicalizeGitCafePullRequestUrl');
export type canonicalizeGitCafePullRequestUrl<A = any, B = any, C = any, D = any> = any;
