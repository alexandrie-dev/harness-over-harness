// Прослойка окна HoH вместо apps/web/src/state/use-atom-command.ts (T3 Code, MIT): команд хоста T3 нет: команда возвращает отказ.
export const useAtomCommand = (..._args: unknown[]) => async (..._input: unknown[]): Promise<{ readonly _tag: 'Failure' }> => ({ _tag: 'Failure' });
