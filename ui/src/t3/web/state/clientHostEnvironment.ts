// Прослойка окна HoH вместо apps/web/src/state/clientHostEnvironment.ts (T3 Code, MIT): окно всегда на машине пользователя.
export const clientHostEnvironmentAtom = (_environmentId: string | null): boolean => true;
