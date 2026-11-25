// Объявления web/openVsxThemes.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import type { ThemeDefinition } from "./themePalette";
export type OpenVsxThemeSort = "downloadCount" | "rating" | "timestamp" | "relevance";
export type OpenVsxThemeExtension = {
    id: string;
    collectionId: string;
    name: string;
    publisher: string;
    description: string;
    downloadCount: number;
    iconUrl: string | null;
    sourceUrl: string | null;
    manifestUrl: string;
    sha256Url: string;
    vsixUrl: string;
    version: string;
    license: string;
};
export type OpenVsxThemeSearchOptions = {
    signal?: AbortSignal;
    sortBy?: OpenVsxThemeSort;
};
export declare function searchOpenVsxThemes(query: string, { signal, sortBy }?: OpenVsxThemeSearchOptions): Promise<OpenVsxThemeExtension[]>;
export declare function importOpenVsxThemeExtension(extension: OpenVsxThemeExtension, signal?: AbortSignal): Promise<ReadonlyArray<ThemeDefinition>>;
