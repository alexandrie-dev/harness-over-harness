// Объявления web/vscodeThemeImport.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { type ThemeDefinition } from "./themePalette";
export declare function isVsCodeThemeFile(value: unknown): boolean;
export declare function parseThemeImport(source: string): ThemeDefinition;
export declare function humanizeThemeName(raw: string): string;
export declare function parseVsCodeThemeFile(value: unknown): ThemeDefinition;
export declare function pairVsCodeThemes(themes: ReadonlyArray<ThemeDefinition>, options?: {
    pairedId?: (light: ThemeDefinition, dark: ThemeDefinition) => string;
}): ReadonlyArray<ThemeDefinition>;
export declare function resolveThemeLabelCollisions(entries: ReadonlyArray<{
    theme: ThemeDefinition;
    sourceName?: string | undefined;
}>): ReadonlyArray<ThemeDefinition>;
