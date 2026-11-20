// Прослойка окна HoH вместо apps/web/src/remoteOpen.ts (T3 Code, MIT): удалённого открытия файлов нет.
export const resolveRemoteOpenState = (..._args: unknown[]): { readonly kind: 'local' } => ({ kind: 'local' });
export const useRemoteOpenResolution = (_environmentId?: unknown): { readonly kind: 'local' } => ({ kind: 'local' });
