// Прослойка окна HoH вместо apps/web/src/rpc/atomRegistry.ts (T3 Code, MIT): реестра атомов нет.
export const appAtomRegistry = { get: (_atom: unknown): unknown => null };
