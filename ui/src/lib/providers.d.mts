export type ProviderScope = 'loopback' | 'private' | 'cloud' | 'unknown';
export interface DetectedProvider { id: string; name: string; icon: string | null; scope: ProviderScope; label: string }
export function addressScope(hostname: string): 'loopback' | 'private' | '';
export function detectProvider(baseUrl: unknown, model?: unknown): DetectedProvider;
