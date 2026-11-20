export const DEFAULT_ACCESS_MODE: 'confirm' | 'full';
/** `hoh/executor/discover` (the engine's contract, not in the generated protocol yet). */
export interface DiscoverResult {
  executor: 'claude'; installed: boolean; version: string | null;
  versionStatus: 'supported' | 'unsupported' | 'unknown'; auth: 'authenticated' | 'unauthenticated' | 'unknown';
  models: { id: string; displayName?: string; isDefault?: boolean }[]; detail: string | null;
}
export interface DiscoverView {
  ready: boolean; tone: 'ok' | 'warn' | 'bad'; headline: string; hint: string; warning: string; version: string;
  models: { id: string; name: string }[]; defaultModel: string;
}
export function claudeModelName(model: { id?: string; displayName?: string }): string;
export function claudeModelLabel(id: string, models?: { id: string; name?: string; displayName?: string }[]): string;
export function discoverView(result: unknown): DiscoverView;
export const FULL_ACCESS_DESCRIPTION: string;
