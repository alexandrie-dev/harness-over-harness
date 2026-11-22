export const FOLDER_FIXED_TEXT: string;
export function hostError(error: unknown): { code: string; message: string };
export function refusalWords(error: unknown, options?: { what?: string; button?: string }): string;
export function hostWords(error: unknown): string;
export function errorText(error: unknown, fallback: string, options?: { what?: string; button?: string }): string;
