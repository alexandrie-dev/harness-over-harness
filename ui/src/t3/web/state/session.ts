// Прослойка окна HoH вместо apps/web/src/state/session.ts (T3 Code, MIT): сессий подключения T3 нет; прав на операции хоста нет.
export const readEnvironmentScope = (_environmentId: string | null | undefined, _scope: string): boolean => false;
export const useEnvironmentScope = (_environmentId: string | null | undefined, _scope: string): boolean => false;
export const readPreparedConnection = (_environmentId: string): null => null;
