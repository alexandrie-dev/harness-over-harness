// Объявления web/components/files/office/word-text.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { type DisplayPage } from "@betteroffice/docx/layout/render";
import { type WordSpan } from "./word-protocol";
/** Measures text in the layer's font. */
export type TextMeasure = {
    /** The natural width of `text` at a font size of 1. */
    readonly width: (text: string) => number;
    /** The font's ascent and descent, at a font size of 1. */
    readonly ascent: number;
    readonly descent: number;
};
/**
 * Builds a page's text layer from its display list: its header, body, notes and footer in reading order. Each glyph
 * run's text sits over its painted glyphs, merged with its neighbours on the line while their text runs on, so
 * selection, find and screen readers see whole words and lines.
 */
export declare function pageText(page: DisplayPage, measure: TextMeasure): WordSpan[];
/**
 * The page and height of the first body text at or after a document position, such as a bookmark's: the heading a
 * table of contents entry points to. Undefined when no text follows it. A page whose content is not built yet is
 * built through `build` only when its body reaches the position.
 */
export declare function positionTarget(pages: readonly DisplayPage[], position: number, build: (index: number) => DisplayPage | undefined): {
    index: number;
    y: any;
} | undefined;
