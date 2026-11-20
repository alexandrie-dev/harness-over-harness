// Прослойка окна HoH вместо apps/web/src/localApi.ts (T3 Code, MIT): моста настольного приложения T3 нет; системные действия окно просит у ядра Tauri отдельно.
export const readLocalApi = (): { readonly shell: { readonly openExternal: (href: string) => Promise<void> } } | undefined => undefined;
