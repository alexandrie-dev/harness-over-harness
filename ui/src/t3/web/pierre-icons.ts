// Прослойка окна HoH вместо apps/web/src/pierre-icons.ts (T3 Code, MIT): значков файлов Pierre нет.
export const hasSpecificPierreIconForFileName = (_name: string): boolean => false;
export const syntheticFileNameForLanguageId = (id: string): string => `file.${id}`;
