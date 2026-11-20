// Прослойка окна HoH вместо apps/web/src/state/use-atom-query-runner.ts (T3 Code, MIT): запросов хоста T3 нет.
export const useAtomQueryRunner = (..._args: unknown[]) => async (..._input: unknown[]): Promise<{ readonly _tag: 'Failure' }> => ({ _tag: 'Failure' });
