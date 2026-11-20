// Прослойка окна HoH вместо apps/web/src/browser/openFileInPreview.ts (T3 Code, MIT): встроенного просмотра нет.
export const isBrowserPreviewFile = (_path: string): boolean => false;
export const openFileInPreview = async (..._args: unknown[]): Promise<void> => {};
export const openUrlInPreview = async (..._args: unknown[]): Promise<void> => {};
export class BrowserPreviewUnavailableError extends Error { constructor(readonly input?: unknown) { super('Browser preview is unavailable'); } }
export const isBrowserSettingsReadError = (_error: unknown): boolean => false;
