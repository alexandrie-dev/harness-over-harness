// Объявления web/components/files/officeArchive.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import JSZip from "jszip";
import { type OfficeWorkerRequest } from "./officePreviewProtocol";
export declare const WORD_LIMITS: {
    /** document.xml read and kept, about 700 pages of prose; the engine opens it in about 1 s. */
    bodyBytes: number;
    bodyElements: number;
    bodyTags: number;
    /** A table is laid out in one go: 100,000 tags is a 4,000-row table, about 2 s. */
    elementTags: number;
    /** Line breaking grows with the square of a paragraph's text: 40K characters take about 2 s. */
    paragraphText: number;
    styles: number;
    styleBytes: number;
    styleUnits: number;
    styleVisits: number;
    stylePropertyVisits: number;
    numberingReferences: number;
    numberingBytes: number;
    numberingMarkup: number;
    auxiliaryPartBytes: number;
    auxiliaryMarkup: number;
    /** Body, styles, lists and notes, with headers and footers counted once per page. */
    expandedBytes: number;
    expandedMarkup: number;
};
export declare function boundedArchive(input: Extract<OfficeWorkerRequest, {
    type: "open";
}>): Promise<{
    archive: JSZip;
    word: {
        documents: string[];
        truncated: boolean;
    } | null;
    workbook: {
        sheets: {
            truncated: boolean;
            gridlines: boolean;
        }[];
        themeFonts: {
            major: string;
            minor: string;
        };
    } | undefined;
}>;
export declare function prepareOffice(bytes: ArrayBuffer, kind: Extract<OfficeWorkerRequest, {
    type: "open";
}>["kind"]): Promise<{
    bytes: any;
    truncated: boolean;
    view: {
        hiddenGridlines: number[];
        themeFonts: {
            major: string;
            minor: string;
        };
    } | undefined;
}>;
