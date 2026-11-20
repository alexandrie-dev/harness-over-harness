// Прослойка окна HoH вместо packages/client-runtime/src/state/runtime.ts (T3 Code, MIT): команд хоста T3 (атомы) в окне нет.
export function isAtomCommandInterrupted(_result: unknown): boolean { return false; }
export function squashAtomCommandFailure(result: unknown): unknown { return result; }
