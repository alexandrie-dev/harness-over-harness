// Прослойка окна HoH вместо apps/web/src/editorPreferences.ts (T3 Code, MIT): внешних редакторов нет.
export class PreferredEditorEnvironmentRequiredError extends Error { constructor(readonly input?: unknown) { super('A preferred editor is unavailable'); } }
export const usePreferredEditor = (_available?: unknown): [null, (value: unknown) => void] => [null, () => {}];
export const useOpenInPreferredEditor = (..._args: unknown[]) => async (..._input: unknown[]): Promise<{ readonly _tag: 'Failure' }> => ({ _tag: 'Failure' });
