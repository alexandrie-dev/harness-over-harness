// Прослойка окна HoH вместо packages/client-runtime/src/errors/index.ts (T3 Code, MIT): 
export function thrownErrorMessage(error: unknown, _resolve?: unknown): string | null { return error instanceof Error && error.message ? error.message : null; }
