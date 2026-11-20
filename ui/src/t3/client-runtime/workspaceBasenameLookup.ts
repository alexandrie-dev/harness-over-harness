// Прослойка окна HoH вместо packages/client-runtime/src/workspaceBasenameLookup.ts (T3 Code, MIT): поиска файлов по имени в рабочей папке через хост T3 нет.
export const WORKSPACE_BASENAME_LOOKUP_LIMIT = 0;
export function needsWorkspaceBasenameLookup(..._args: unknown[]): boolean { return false; }
export function workspaceLinkLookupQuery(..._args: unknown[]): string { return ''; }
export function findAgentDirectoryMatch(..._args: unknown[]): null { return null; }
export function pickProjectLinkMatch(..._args: unknown[]): null { return null; }
export function pickWorkspaceLinkMatch(..._args: unknown[]): null { return null; }
