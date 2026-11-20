// Прослойка окна HoH вместо apps/web/src/hooks/usePullRequestLinking.ts (T3 Code, MIT): запросов на слияние нет.
export const usePullRequestLinking = (_environmentId?: string) => ({ isLinked: (..._args: unknown[]): boolean => false, link: async (..._args: unknown[]): Promise<void> => {} });
