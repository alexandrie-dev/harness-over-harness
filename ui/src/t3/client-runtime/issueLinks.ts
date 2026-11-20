// Прослойка окна HoH вместо packages/client-runtime/src/issueLinks.ts (T3 Code, MIT): трекеров задач в окне нет.
export function mayContainStandaloneIssueLink(_text: string): boolean { return false; }
export function resolveIssueLinkTarget(..._args: unknown[]): null { return null; }
