// Объявления web/components/files/office/fonts.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import type { BundledFontProvider } from "@betteroffice/docx/layout";
import { type BundledFontFace } from "@betteroffice/fonts";
/** The faces Word substitutes for a document's fonts: metric-compatible ones first, then Word's own fallbacks. */
export declare function wordFonts(): BundledFontProvider;
/** The face PowerPoint draws a family with: its metric-compatible or bundled face, else PowerPoint's substitute. */
export declare function presentationFace(family: string, bold: boolean, italic: boolean): BundledFontFace;
/** The face Excel's cells draw a family with when it is missing: its metric-compatible or bundled face, if any. */
export declare function sheetFace(family: string, bold: boolean, italic: boolean): any;
/**
 * A bundled face's bytes. The font package caches them per worker, so a face loads once per open file; the browser's
 * caches, which every worker shares, serve the files to the next.
 */
export declare function loadFace(face: BundledFontFace): any;
