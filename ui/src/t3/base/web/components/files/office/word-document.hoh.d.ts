// Объявления web/components/files/office/word-document.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { type DisplayPage } from "@betteroffice/docx/layout/render";
import { type VectorBitmap } from "./vector-pictures";
import { type WordPageSize, type WordSpan } from "./word-protocol";
/**
 * Opens the document and lays out its first pages; the rest lay out a `step` at a time. `fonts` is the font set the
 * painting canvas draws text with, if it has one.
 */
export declare function openWordDocument(bytes: Uint8Array, fonts: FontFaceSet | undefined): Promise<{
    pages: () => any;
    loading: () => boolean;
    built: (index: number) => any;
    /**
     * Lays out about `stepBudget` milliseconds more of the body. Blocks vary widely, and sizing a step by the last one
     * alone overshoots after a run of cheap blocks.
     */
    step: () => {
        done: true;
        pages: any;
        kept: any;
    } | {
        done: false;
        pages?: never;
        kept?: never;
    };
    paint: (index: number, scale: number, ratio: number, signal: AbortSignal) => Promise<{
        bitmap: ImageBitmap;
        vectors: import("./vector-pictures").VectorRequest[];
    } | undefined>;
    vectors: (bitmaps: readonly VectorBitmap[]) => void;
    text: (index: number) => readonly WordSpan[];
    target: (name: string) => {
        index: number;
        y: any;
    } | undefined;
}>;
export declare function pageSize(page: DisplayPage): WordPageSize;
